use crate::plugin::MarkdownPlugin;
use pulldown_cmark::{Event, Tag, TagEnd};

pub struct PluginInstance;

impl MarkdownPlugin for PluginInstance {
    fn name(&self) -> &'static str {
        "admonitions"
    }
}

pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Information about a parsed Obsidian callout header
#[derive(Debug, Clone)]
pub struct CalloutHeader {
    pub callout_type: String,
    pub is_collapsible: bool,
    pub is_open: bool,
    pub title: String,
}

/// Helper to parse Obsidian callout header from text
/// E.g.:
/// `[!note]` -> type="note", collapsible=false, title="Note"
/// `[!example]- Stats, Abilities, Skills` -> type="example", collapsible=true, is_open=false, title="Stats, Abilities, Skills"
/// `[!danger]+ Expanded Warning` -> type="danger", collapsible=true, is_open=true, title="Expanded Warning"
pub fn parse_callout_header(text: &str) -> Option<(CalloutHeader, &str)> {
    let trimmed_start = text.trim_start();
    if !trimmed_start.starts_with("[!") {
        return None;
    }

    let end_bracket = trimmed_start.find(']')?;
    let inner = &trimmed_start[2..end_bracket];
    let after_bracket = &trimmed_start[end_bracket + 1..];

    // Check for collapsible modifier '-' or '+' right after ']'
    let (is_collapsible, is_open, title_part) = if after_bracket.starts_with('-') {
        (true, false, &after_bracket[1..])
    } else if after_bracket.starts_with('+') {
        (true, true, &after_bracket[1..])
    } else {
        (false, true, after_bracket)
    };

    let (callout_type, _metadata) = match inner.split_once('|') {
        Some((t, m)) => (t.trim(), Some(m.trim())),
        None => (inner.trim(), None),
    };

    let callout_type = callout_type.to_lowercase();
    if callout_type.is_empty() {
        return None;
    }

    // Next comes optional whitespace and optional custom title up to newline (or end of text)
    let (first_line_rest, remaining_text) = match title_part.find('\n') {
        Some(idx) => (&title_part[..idx], &title_part[idx + 1..]),
        None => (title_part, ""),
    };

    let title_trimmed = first_line_rest.trim();
    let title = if title_trimmed.is_empty() {
        // Default title: capitalize first letter of callout_type
        let mut chars = callout_type.chars();
        match chars.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        }
    } else {
        title_trimmed.to_string()
    };

    Some((
        CalloutHeader {
            callout_type,
            is_collapsible,
            is_open,
            title,
        },
        remaining_text,
    ))
}

pub fn callout_icon_svg(callout_type: &str) -> &'static str {
    match callout_type {
        "note" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>"#,
        "info" | "todo" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line></svg>"#,
        "tip" | "hint" | "important" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18h6"></path><path d="M10 22h4"></path><path d="M12 2a7 7 0 0 0-4 12.7V17h8v-2.3A7 7 0 0 0 12 2z"></path></svg>"#,
        "success" | "check" | "done" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path><polyline points="22 4 12 14.01 9 11.01"></polyline></svg>"#,
        "warning" | "caution" | "attention" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path><line x1="12" y1="9" x2="12" y2="13"></line><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>"#,
        "danger" | "error" | "failure" | "fail" | "missing" | "bug" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polygon points="7.86 2 16.14 2 22 7.86 22 16.14 16.14 22 7.86 22 2 16.14 2 7.86 7.86 2"></polygon><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>"#,
        "question" | "help" | "faq" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"></path><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>"#,
        "quote" | "cite" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M3 21c3 0 7-1 7-8V5c0-1.25-.756-2.017-2-2H4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V20c0 1 0 1 1 1z"></path><path d="M15 21c3 0 7-1 7-8V5c0-1.25-.757-2.017-2-2h-4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V20c0 1 0 1 1 1z"></path></svg>"#,
        "example" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"></line><line x1="8" y1="12" x2="21" y2="12"></line><line x1="8" y1="18" x2="21" y2="18"></line><line x1="3" y1="6" x2="3.01" y2="6"></line><line x1="3" y1="12" x2="3.01" y2="12"></line><line x1="3" y1="18" x2="3.01" y2="18"></line></svg>"#,
        "tldr" => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><polyline points="10 9 9 9 8 9"></polyline></svg>"#,
        _ => r#"<svg class="admonition-icon" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>"#,
    }
}

/// Transform events to replace Obsidian callout blockquotes with styled admonition containers
pub fn process_callouts<'a>(events: Vec<Event<'a>>) -> Vec<Event<'a>> {
    let mut result = Vec::with_capacity(events.len());
    let mut i = 0;

    // Stack tracking nested callouts: true if current blockquote is a callout (and whether it's collapsible)
    let mut callout_stack: Vec<Option<bool>> = Vec::new();

    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::BlockQuote(kind)) => {
                // Check if this blockquote starts with a callout header
                // The pattern in pulldown-cmark is:
                // Event::Start(Tag::BlockQuote)
                // Event::Start(Tag::Paragraph)
                // Event::Text(t) where t starts with "[!"
                if i + 2 < events.len() {
                    if let (Event::Start(Tag::Paragraph), Event::Text(text)) = (&events[i + 1], &events[i + 2]) {
                        if let Some((header, remaining)) = parse_callout_header(text) {
                            callout_stack.push(Some(header.is_collapsible));

                            let icon = callout_icon_svg(&header.callout_type);
                            let title_escaped = escape_html(&header.title);

                            if header.is_collapsible {
                                let open_attr = if header.is_open { " open" } else { "" };
                                result.push(Event::Html(
                                    format!(
                                        r#"<details class="admonition admonition-{}"{open_attr}><summary class="admonition-title"><span class="admonition-title-icon">{icon}</span><span class="admonition-title-text">{title_escaped}</span></summary><div class="admonition-content">"#,
                                        header.callout_type
                                    ).into()
                                ));
                            } else {
                                result.push(Event::Html(
                                    format!(
                                        r#"<div class="admonition admonition-{}"><div class="admonition-title"><span class="admonition-title-icon">{icon}</span><span class="admonition-title-text">{title_escaped}</span></div><div class="admonition-content">"#,
                                        header.callout_type
                                    ).into()
                                ));
                            }

                            // Now handle what to do with Event::Start(Paragraph) and Event::Text
                            let remaining_trimmed = remaining.trim_start();
                            if remaining_trimmed.is_empty() {
                                // First line was just the callout marker / title
                                // Consume Event::Start(Paragraph) and Event::Text
                                i += 3;
                                // If the next event is SoftBreak or HardBreak, skip it because the title line ended
                                if i < events.len() {
                                    match events[i] {
                                        Event::SoftBreak | Event::HardBreak => {
                                            i += 1;
                                        }
                                        _ => {}
                                    }
                                }
                                // If the next event is End(Paragraph), skip it so we don't leave an empty <p></p>
                                if i < events.len() {
                                    if let Event::End(TagEnd::Paragraph) = events[i] {
                                        i += 1;
                                    } else {
                                        // The paragraph actually continues with subsequent events (e.g. text on next line)
                                        // Re-emit Start(Paragraph) so following text is wrapped in <p>...</p>
                                        result.push(Event::Start(Tag::Paragraph));
                                    }
                                }
                            } else {
                                // Keep Paragraph start, replace text with remainder
                                result.push(Event::Start(Tag::Paragraph));
                                result.push(Event::Text(remaining_trimmed.to_string().into()));
                                i += 3;
                            }
                            continue;
                        }
                    }
                }

                callout_stack.push(None);
                result.push(Event::Start(Tag::BlockQuote(*kind)));
                i += 1;
            }
            Event::End(TagEnd::BlockQuote) => {
                match callout_stack.pop() {
                    Some(Some(is_collapsible)) => {
                        if is_collapsible {
                            result.push(Event::Html("</div></details>".into()));
                        } else {
                            result.push(Event::Html("</div></div>".into()));
                        }
                    }
                    _ => {
                        result.push(Event::End(TagEnd::BlockQuote));
                    }
                }
                i += 1;
            }
            _ => {
                result.push(events[i].clone());
                i += 1;
            }
        }
    }

    result
}
