use crate::plugin::{MarkdownPlugin, TransformContext};
use pulldown_cmark::{Event, Tag, TagEnd};

pub struct PluginInstance;

fn is_image_file(target: &str) -> bool {
    let lower = target.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".gif")
        || lower.ends_with(".svg")
        || lower.ends_with(".webp")
        || lower.ends_with(".bmp")
        || lower.ends_with(".ico")
}

fn parse_obsidian_image_options(
    inner: &str,
) -> (String, Option<String>, Option<u32>, Option<u32>, Vec<String>) {
    let (target, options) = match inner.split_once('|') {
        Some((t, opts)) => (t.trim(), Some(opts.trim())),
        None => (inner.trim(), None),
    };

    let mut alt = None;
    let mut width = None;
    let mut height = None;
    let mut classes = Vec::new();

    if let Some(opts) = options {
        let parts: Vec<&str> = opts
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        for part in parts {
            // Check WxH dimension (e.g. 200x300)
            if let Some((w_str, h_str)) = part.split_once('x') {
                if let (Ok(w), Ok(h)) = (w_str.trim().parse::<u32>(), h_str.trim().parse::<u32>()) {
                    width = Some(w);
                    height = Some(h);
                    continue;
                }
            }
            // Check width dimension (e.g. 400)
            if let Ok(w) = part.parse::<u32>() {
                width = Some(w);
                continue;
            }

            // Check formatting tokens (e.g. "right lp")
            let sub_tokens: Vec<&str> = part.split_whitespace().collect();
            let mut recognized = false;
            for token in &sub_tokens {
                match *token {
                    "right" => {
                        classes.push("align-right".to_string());
                        recognized = true;
                    }
                    "left" => {
                        classes.push("align-left".to_string());
                        recognized = true;
                    }
                    "center" => {
                        classes.push("align-center".to_string());
                        recognized = true;
                    }
                    "lp" => {
                        classes.push("lp".to_string());
                        recognized = true;
                    }
                    _ => {}
                }
            }

            if !recognized {
                if alt.is_none() {
                    alt = Some(part.to_string());
                } else {
                    classes.push(part.to_string());
                }
            }
        }
    }

    if alt.is_none() {
        alt = Some(target.to_string());
    }

    (target.to_string(), alt, width, height, classes)
}

fn build_image_html(
    src: &str,
    alt: &str,
    width: Option<u32>,
    height: Option<u32>,
    classes: &[String],
) -> String {
    let mut tag = format!("<img src=\"{}\" alt=\"{}\"", src, alt);
    if let Some(w) = width {
        tag.push_str(&format!(" width=\"{}\"", w));
    }
    if let Some(h) = height {
        tag.push_str(&format!(" height=\"{}\"", h));
    }
    if !classes.is_empty() {
        tag.push_str(&format!(" class=\"{}\"", classes.join(" ")));
    }
    tag.push_str(" />");
    tag
}

impl MarkdownPlugin for PluginInstance {
    fn name(&self) -> &'static str {
        "obsidian_wikilinks"
    }

    fn trigger_byte(&self) -> Option<u8> {
        Some(b'[')
    }

    fn transform_with_context<'a>(
        &self,
        event: Event<'a>,
        ctx: &TransformContext<'a>,
    ) -> Option<Vec<Event<'a>>> {
        if let Event::Text(ref text) = event {
            if text.contains("[[") && text.contains("]]") {
                let mut events = Vec::new();
                let mut remainder = text.as_ref();

                while let Some(start_idx) = remainder.find("[[") {
                    let is_embed = start_idx > 0 && remainder.as_bytes()[start_idx - 1] == b'!';
                    let prefix = if is_embed {
                        &remainder[..start_idx - 1]
                    } else {
                        &remainder[..start_idx]
                    };

                    if !prefix.is_empty() {
                        events.push(Event::Text(prefix.to_string().into()));
                    }

                    if let Some(end_idx) = remainder[start_idx + 2..].find("]]") {
                        let actual_end = start_idx + 2 + end_idx;
                        let inner = &remainder[start_idx + 2..actual_end];

                        if is_embed {
                            let (target, alt, width, height, classes) =
                                parse_obsidian_image_options(inner);

                            if is_image_file(&target) {
                                let media_url = if let Some(media_resolver) = ctx.media_resolver {
                                    media_resolver(&target).unwrap_or_else(|| {
                                        format!("/_media/{}", urlencoding::encode(&target))
                                    })
                                } else {
                                    format!("/_media/{}", urlencoding::encode(&target))
                                };

                                let img_html = build_image_html(
                                    &media_url,
                                    alt.as_deref().unwrap_or(""),
                                    width,
                                    height,
                                    &classes,
                                );
                                events.push(Event::Html(img_html.into()));
                            } else {
                                // Non-image embed: fallback or link
                                let (link_url, resolved_title) = if let Some(resolver) = ctx.link_resolver {
                                    let curr = ctx.current_url.unwrap_or("/");
                                    resolver(curr, &target)
                                } else {
                                    (crate::crawler::normalize_slug_path(&target), None)
                                };

                                let display_text = alt.unwrap_or_else(|| resolved_title.unwrap_or(target));

                                events.push(Event::Start(Tag::Link {
                                    link_type: pulldown_cmark::LinkType::Inline,
                                    dest_url: link_url.into(),
                                    title: "".into(),
                                    id: "".into(),
                                }));
                                events.push(Event::Text(display_text.into()));
                                events.push(Event::End(TagEnd::Link));
                            }
                        } else {
                            // Standard Wikilink [[Target|Display]] or [[Target#Heading|Display]] or [[#Heading|Display]]
                            let (target_full, display) = match inner.split_once('|') {
                                Some((t, d)) => (t.trim(), Some(d.trim())),
                                None => (inner.trim(), None),
                            };

                            let (target_doc, anchor) = match target_full.split_once('#') {
                                Some((doc, heading)) => (doc.trim(), Some(heading.trim())),
                                None => (target_full, None),
                            };

                            let (mut link_url, resolved_title) = if target_doc.is_empty() {
                                // Anchor on the current page, e.g. [[#Heading]]
                                ("".to_string(), None)
                            } else if let Some(resolver) = ctx.link_resolver {
                                let curr = ctx.current_url.unwrap_or("/");
                                resolver(curr, target_doc)
                            } else {
                                (crate::crawler::normalize_slug_path(target_doc), None)
                            };

                            if let Some(h) = anchor {
                                let anchor_slug = crate::markdown::ast::slugify_heading(h);
                                if !anchor_slug.is_empty() {
                                    link_url.push('#');
                                    link_url.push_str(&anchor_slug);
                                }
                            }

                            let display_text = display
                                .map(|d| d.to_string())
                                .or(resolved_title)
                                .unwrap_or_else(|| target_full.to_string());

                            events.push(Event::Start(Tag::Link {
                                link_type: pulldown_cmark::LinkType::Inline,
                                dest_url: link_url.into(),
                                title: "".into(),
                                id: "".into(),
                            }));
                            events.push(Event::Text(display_text.into()));
                            events.push(Event::End(TagEnd::Link));
                        }

                        remainder = &remainder[actual_end + 2..];
                    } else {
                        // Unclosed [[
                        break;
                    }
                }

                if !remainder.is_empty() {
                    events.push(Event::Text(remainder.to_string().into()));
                }

                return Some(events);
            }
        }
        None
    }
}
