pub mod ast;
pub mod builder;

use gray_matter::engine::YAML;
use gray_matter::Matter;
use pulldown_cmark::{html, Event, Options, Parser};
use serde_json::Value;
use std::sync::Arc;

use crate::plugin::PluginRegistry;

#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub title: String,
    pub title_source: String,
    pub frontmatter_json: String,
    pub html: String,
    pub raw_content: String,
    pub links: Vec<String>,
    pub toc_html: String,
    pub ast: ast::DocAst,
    pub ast_blob: Vec<u8>,
}

pub struct MarkdownEngine {
    matter: Matter<YAML>,
    registry: Arc<PluginRegistry>,
}

impl Clone for MarkdownEngine {
    fn clone(&self) -> Self {
        Self {
            matter: Matter::<YAML>::new(),
            registry: self.registry.clone(),
        }
    }
}

impl MarkdownEngine {
    pub fn new(registry: Arc<PluginRegistry>) -> Self {
        Self {
            matter: Matter::<YAML>::new(),
            registry,
        }
    }

    pub fn parse(&self, raw: &str, default_title: &str) -> ParsedDoc {
        self.parse_with_context(raw, default_title, None, None, None)
    }

    pub fn parse_with_context(
        &self,
        raw: &str,
        default_title: &str,
        current_url: Option<&str>,
        link_resolver: Option<&(dyn Fn(&str, &str) -> (String, Option<String>) + Send + Sync)>,
        media_resolver: Option<&(dyn Fn(&str) -> Option<String> + Send + Sync)>,
    ) -> ParsedDoc {
        // 1. Parse frontmatter
        let parsed_matter = self.matter.parse(raw);
        let content = parsed_matter.content;

        let frontmatter_val: Value = if let Some(data) = parsed_matter.data {
            data.deserialize().unwrap_or(Value::Object(Default::default()))
        } else {
            Value::Object(Default::default())
        };

        // Extract title and source from frontmatter or first H1, or fallback
        let (title, title_source_str) = extract_title_with_source(&frontmatter_val, &content, default_title);
        let title_source = title_source_str.to_string();

        let frontmatter_json = serde_json::to_string(&frontmatter_val).unwrap_or_else(|_| "{}".to_string());

        // 2. Pulldown-cmark parsing & Plugins
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TASKLISTS);
        options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

        let parser = Parser::new_ext(&content, options);

        let mut collapsed_events = Vec::new();
        let mut text_buf = String::new();

        for event in parser {
            match event {
                Event::Text(t) => {
                    text_buf.push_str(&t);
                }
                other => {
                    if !text_buf.is_empty() {
                        collapsed_events.push(Event::Text(text_buf.clone().into()));
                        text_buf.clear();
                    }
                    collapsed_events.push(other);
                }
            }
        }
        if !text_buf.is_empty() {
            collapsed_events.push(Event::Text(text_buf.into()));
        }

        let mut transformed_events = Vec::new();
        let mut links = Vec::new();
        let mut headings = Vec::new();

        let mut in_heading = false;
        let mut current_heading_text = String::new();
        let mut current_heading_level = 0;
        let mut skipped_first_h1 = false;
        let mut in_skipped_h1 = false;

        for event in collapsed_events {
            // If title is taken from H1, skip the first H1 tag so it isn't returned in the content HTML
            if title_source == "h1" && !skipped_first_h1 {
                match &event {
                    Event::Start(pulldown_cmark::Tag::Heading { level: pulldown_cmark::HeadingLevel::H1, .. }) => {
                        in_skipped_h1 = true;
                        continue;
                    }
                    Event::End(pulldown_cmark::TagEnd::Heading(pulldown_cmark::HeadingLevel::H1)) if in_skipped_h1 => {
                        in_skipped_h1 = false;
                        skipped_first_h1 = true;
                        continue;
                    }
                    _ if in_skipped_h1 => {
                        continue;
                    }
                    _ => {}
                }
            }

            // Track headings for TOC
            match &event {
                Event::Start(pulldown_cmark::Tag::Heading { level, .. }) => {
                    in_heading = true;
                    current_heading_level = *level as usize;
                    current_heading_text.clear();
                }
                Event::End(pulldown_cmark::TagEnd::Heading(_)) => {
                    in_heading = false;
                    let slug = slugify(&current_heading_text);
                    headings.push((current_heading_level, current_heading_text.clone(), slug));
                }
                Event::Text(t) if in_heading => {
                    current_heading_text.push_str(t);
                }
                _ => {}
            }

            // Extract internal links from [[Wikilinks]] pattern
            if let Event::Text(ref text) = event {
                let mut remainder = text.as_ref();
                while let Some(start_idx) = remainder.find("[[") {
                    if let Some(end_idx) = remainder[start_idx + 2..].find("]]") {
                        let actual_end = start_idx + 2 + end_idx;
                        let inner = &remainder[start_idx + 2..actual_end];
                        let target = inner.split_once('|').map(|(t, _)| t).unwrap_or(inner).trim();
                        links.push(target.to_string());
                        remainder = &remainder[actual_end + 2..];
                    } else {
                        break;
                    }
                }
            }

            // Plugin pipeline
            let ctx = crate::plugin::TransformContext {
                current_url,
                link_resolver,
                media_resolver,
            };
            let mut applied = false;
            for plugin in self.registry.plugins() {
                if let Some(tb) = plugin.trigger_byte() {
                    let matches_trigger = match &event {
                        Event::Text(t) => t.as_bytes().contains(&tb),
                        _ => false,
                    };
                    if !matches_trigger {
                        continue;
                    }
                }

                if let Some(events) = plugin.transform_with_context(event.clone(), &ctx) {
                    transformed_events.extend(events);
                    applied = true;
                    break;
                }
            }

            if !applied {
                transformed_events.push(event);
            }
        }

        // Build DocAst object model from untransformed callouts (builder handles them directly)
        let ast = builder::events_to_ast(
            transformed_events.clone(),
            title.clone(),
            title_source.clone(),
            frontmatter_val,
            links.clone(),
        );

        let ast_blob = ast.to_cbor().unwrap_or_default();

        // Apply callout/admonition container transformation for fallback HTML
        let transformed_events = crate::plugins::admonitions::process_callouts(transformed_events);

        let mut html_output = String::new();
        html::push_html(&mut html_output, transformed_events.into_iter());

        // Generate TOC HTML
        let toc_html = generate_toc_html(&headings);

        ParsedDoc {
            title,
            title_source,
            frontmatter_json,
            html: html_output,
            raw_content: content,
            links,
            toc_html,
            ast,
            ast_blob,
        }
    }
}

pub fn extract_title_with_source(frontmatter: &Value, markdown: &str, default_title: &str) -> (String, &'static str) {
    if let Some(t) = frontmatter.get("title").and_then(|v| v.as_str()) {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return (trimmed.to_string(), "frontmatter");
        }
    }

    if let Some(h1) = extract_first_h1(markdown) {
        return (h1, "h1");
    }

    (default_title.to_string(), "filename")
}

pub fn extract_title(frontmatter: &Value, markdown: &str, default_title: &str) -> String {
    extract_title_with_source(frontmatter, markdown, default_title).0
}

pub fn extract_title_from_raw(raw: &str, default_title: &str) -> String {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(raw);
    let frontmatter_val: Value = if let Some(data) = parsed.data {
        data.deserialize().unwrap_or(Value::Object(Default::default()))
    } else {
        Value::Object(Default::default())
    };

    extract_title(&frontmatter_val, &parsed.content, default_title)
}

fn extract_first_h1(markdown: &str) -> Option<String> {
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            let h1 = trimmed[2..].trim();
            if !h1.is_empty() {
                return Some(h1.to_string());
            }
        }
    }
    None
}

fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn generate_toc_html(headings: &[(usize, String, String)]) -> String {
    if headings.is_empty() {
        return String::new();
    }
    let mut out = String::from("<nav class=\"toc\"><ul>");
    for (level, text, slug) in headings {
        out.push_str(&format!(
            "<li class=\"toc-level-{level}\"><a href=\"#{slug}\">{text}</a></li>"
        ));
    }
    out.push_str("</ul></nav>");
    out
}
