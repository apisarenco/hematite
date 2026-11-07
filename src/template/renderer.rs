use std::sync::Arc;

use crate::markdown::ast::{BlockNode, DocAst, InlineNode, ListItem};
use crate::template::TemplateEngine;

/// Renders a DocAst into HTML.
/// If an element template exists (e.g. `elements/heading.html` or `elements/admonition.html`),
/// it delegates rendering to Minijinja. Otherwise, it uses high-speed hardcoded Rust emission.
pub struct ElementRenderer {
    template_engine: Arc<TemplateEngine>,
}

impl ElementRenderer {
    pub fn new(template_engine: Arc<TemplateEngine>) -> Self {
        Self { template_engine }
    }

    /// Render an entire AST document to an HTML string
    pub fn render_doc(&self, ast: &DocAst) -> String {
        let mut out = String::with_capacity(1024);
        for block in &ast.blocks {
            self.render_block(block, ast, &mut out);
        }
        out
    }

    /// Render a single block node
    pub fn render_block(&self, block: &BlockNode, ast: &DocAst, out: &mut String) {
        let tmpl_name = match block {
            BlockNode::Heading { .. } => "elements/heading.html",
            BlockNode::Paragraph { .. } => "elements/paragraph.html",
            BlockNode::Blockquote { .. } => "elements/blockquote.html",
            BlockNode::CodeBlock { .. } => "elements/code_block.html",
            BlockNode::List { .. } => "elements/list.html",
            BlockNode::Table { .. } => "elements/table.html",
            BlockNode::Admonition { .. } => "elements/admonition.html",
            BlockNode::ThematicBreak => "elements/thematic_break.html",
            BlockNode::HtmlBlock { .. } => "elements/html_block.html",
            BlockNode::CustomBlock { name, .. } => {
                // Check for custom template or fallback
                let custom_tmpl = format!("elements/{}.html", name);
                if self.template_engine.env().get_template(&custom_tmpl).is_ok() {
                    return self.render_template_block(&custom_tmpl, block, ast, out);
                }
                "elements/custom_block.html"
            }
        };

        if self.template_engine.env().get_template(tmpl_name).is_ok() {
            self.render_template_block(tmpl_name, block, ast, out);
        } else {
            self.render_hardcoded_block(block, ast, out);
        }
    }

    fn render_template_block(&self, tmpl_name: &str, block: &BlockNode, ast: &DocAst, out: &mut String) {
        if let Ok(tmpl) = self.template_engine.env().get_template(tmpl_name) {
            // Provide rendered default HTML so the template can choose to wrap or replace it
            let mut inner_html = String::new();
            self.render_hardcoded_block(block, ast, &mut inner_html);

            let block_val = serde_json::to_value(block).unwrap_or(serde_json::Value::Null);
            let ctx = minijinja::context! {
                block => block_val,
                rendered_html => inner_html,
                ast_title => ast.title.clone(),
                frontmatter => ast.frontmatter.clone(),
            };

            if let Ok(rendered) = tmpl.render(ctx) {
                out.push_str(&rendered);
                return;
            }
        }
        // Fallback if template render failed
        self.render_hardcoded_block(block, ast, out);
    }

    fn render_hardcoded_block(&self, block: &BlockNode, ast: &DocAst, out: &mut String) {
        match block {
            BlockNode::Heading { level, id, children } => {
                out.push_str(&format!("<h{}", level));
                if !id.is_empty() {
                    out.push_str(&format!(" id=\"{}\"", html_escape(id)));
                }
                out.push('>');
                self.render_inlines(children, out);
                out.push_str(&format!("</h{}>\n", level));
            }
            BlockNode::Paragraph { children } => {
                out.push_str("<p>");
                self.render_inlines(children, out);
                out.push_str("</p>\n");
            }
            BlockNode::Blockquote { children } => {
                out.push_str("<blockquote>\n");
                for child in children {
                    self.render_block(child, ast, out);
                }
                out.push_str("</blockquote>\n");
            }
            BlockNode::CodeBlock { language, code } => {
                out.push_str("<pre><code");
                if let Some(lang) = language {
                    if !lang.is_empty() {
                        out.push_str(&format!(" class=\"language-{}\"", html_escape(lang)));
                    }
                }
                out.push('>');
                out.push_str(&html_escape(code));
                out.push_str("</code></pre>\n");
            }
            BlockNode::List { ordered, start, items } => {
                if *ordered {
                    if let Some(s) = start {
                        out.push_str(&format!("<ol start=\"{}\">\n", s));
                    } else {
                        out.push_str("<ol>\n");
                    }
                } else {
                    out.push_str("<ul>\n");
                }

                for item in items {
                    self.render_list_item(item, ast, out);
                }

                if *ordered {
                    out.push_str("</ol>\n");
                } else {
                    out.push_str("</ul>\n");
                }
            }
            BlockNode::Table { headers, rows } => {
                out.push_str("<table>\n");
                if !headers.is_empty() {
                    out.push_str("<thead>\n<tr>\n");
                    for h in headers {
                        out.push_str("<th>");
                        self.render_inlines(h, out);
                        out.push_str("</th>\n");
                    }
                    out.push_str("</tr>\n</thead>\n");
                }
                if !rows.is_empty() {
                    out.push_str("<tbody>\n");
                    for row in rows {
                        out.push_str("<tr>\n");
                        for cell in row {
                            out.push_str("<td>");
                            self.render_inlines(cell, out);
                            out.push_str("</td>\n");
                        }
                        out.push_str("</tr>\n");
                    }
                    out.push_str("</tbody>\n");
                }
                out.push_str("</table>\n");
            }
            BlockNode::Admonition { kind, title, collapsible, open, children } => {
                let tag = if *collapsible { "details" } else { "div" };
                let open_attr = if *collapsible && *open { " open" } else { "" };
                let kind_clean = html_escape(kind);
                
                out.push_str(&format!(
                    "<{} class=\"admonition admonition-{}\"{}>\n",
                    tag, kind_clean, open_attr
                ));

                if *collapsible {
                    out.push_str("<summary class=\"admonition-title\">");
                } else {
                    out.push_str("<div class=\"admonition-title\">");
                }

                out.push_str(&format!("<span class=\"admonition-icon admonition-icon-{}\"></span>", kind_clean));
                
                let title_text = title.as_deref().unwrap_or(kind.as_str());
                out.push_str(&format!(
                    "<span class=\"admonition-title-text\">{}</span>",
                    html_escape(title_text)
                ));

                if *collapsible {
                    out.push_str("</summary>\n");
                } else {
                    out.push_str("</div>\n");
                }

                out.push_str("<div class=\"admonition-content\">\n");
                for child in children {
                    self.render_block(child, ast, out);
                }
                out.push_str("</div>\n");

                out.push_str(&format!("</{}>\n", tag));
            }
            BlockNode::ThematicBreak => {
                out.push_str("<hr />\n");
            }
            BlockNode::HtmlBlock { html } => {
                out.push_str(html);
                out.push('\n');
            }
            BlockNode::CustomBlock { name, attributes, children } => {
                out.push_str(&format!("<div class=\"custom-block custom-block-{}\"", html_escape(name)));
                for (k, v) in attributes {
                    out.push_str(&format!(" data-{}=\"{}\"", html_escape(k), html_escape(v)));
                }
                out.push_str(">\n");
                for child in children {
                    self.render_block(child, ast, out);
                }
                out.push_str("</div>\n");
            }
        }
    }

    fn render_list_item(&self, item: &ListItem, ast: &DocAst, out: &mut String) {
        out.push_str("<li>");
        if let Some(checked) = item.task {
            let checked_attr = if checked { " checked=\"\"" } else { "" };
            out.push_str(&format!(
                "<input disabled=\"\" type=\"checkbox\"{}/> ",
                checked_attr
            ));
        }

        for child in &item.children {
            if let BlockNode::Paragraph { children } = child {
                // If it's a tight single paragraph in list item, omit <p>
                if item.children.len() == 1 {
                    self.render_inlines(children, out);
                    continue;
                }
            }
            self.render_block(child, ast, out);
        }
        out.push_str("</li>\n");
    }

    pub fn render_inlines(&self, inlines: &[InlineNode], out: &mut String) {
        for inline in inlines {
            match inline {
                InlineNode::Text(t) => out.push_str(&html_escape(t)),
                InlineNode::Bold(children) => {
                    out.push_str("<strong>");
                    self.render_inlines(children, out);
                    out.push_str("</strong>");
                }
                InlineNode::Italic(children) => {
                    out.push_str("<em>");
                    self.render_inlines(children, out);
                    out.push_str("</em>");
                }
                InlineNode::Strikethrough(children) => {
                    out.push_str("<del>");
                    self.render_inlines(children, out);
                    out.push_str("</del>");
                }
                InlineNode::Code(c) => {
                    out.push_str("<code>");
                    out.push_str(&html_escape(c));
                    out.push_str("</code>");
                }
                InlineNode::Link { url, title, children } => {
                    out.push_str(&format!("<a href=\"{}\"", html_escape(url)));
                    if let Some(t) = title {
                        out.push_str(&format!(" title=\"{}\"", html_escape(t)));
                    }
                    out.push('>');
                    self.render_inlines(children, out);
                    out.push_str("</a>");
                }
                InlineNode::Image { url, alt, title } => {
                    out.push_str(&format!("<img src=\"{}\" alt=\"{}\"", html_escape(url), html_escape(alt)));
                    if let Some(t) = title {
                        out.push_str(&format!(" title=\"{}\"", html_escape(t)));
                    }
                    out.push_str(" />");
                }
                InlineNode::Wikilink { target, alias, resolved_url } => {
                    let href = resolved_url.as_deref().unwrap_or(target.as_str());
                    let label = alias.as_deref().unwrap_or(target.as_str());
                    out.push_str(&format!(
                        "<a href=\"{}\" class=\"internal-wikilink\">{}</a>",
                        html_escape(href),
                        html_escape(label)
                    ));
                }
                InlineNode::HtmlInline(raw) => {
                    out.push_str(raw);
                }
                InlineNode::LineBreak => {
                    out.push_str("<br />\n");
                }
                InlineNode::SoftBreak => {
                    out.push('\n');
                }
            }
        }
    }
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
