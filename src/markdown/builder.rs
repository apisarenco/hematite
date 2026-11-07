use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};

use super::ast::{slugify_heading, BlockNode, DocAst, InlineNode, ListItem};

/// Builds a DocAst from pulldown_cmark events
pub fn events_to_ast(
    events: Vec<Event>,
    title: String,
    title_source: String,
    frontmatter: serde_json::Value,
    links: Vec<String>,
) -> DocAst {
    let mut ast = DocAst::new(title, title_source, frontmatter);
    ast.links = links;

    let mut block_stack: Vec<BlockBuilder> = Vec::new();

    for event in events {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, id, .. } => {
                    let lvl = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    block_stack.push(BlockBuilder::Heading {
                        level: lvl,
                        id: id.map(|s| s.to_string()).unwrap_or_default(),
                        inlines: Vec::new(),
                    });
                }
                Tag::Paragraph => {
                    block_stack.push(BlockBuilder::Paragraph {
                        inlines: Vec::new(),
                    });
                }
                Tag::BlockQuote(_) => {
                    block_stack.push(BlockBuilder::Blockquote {
                        children: Vec::new(),
                    });
                }
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(l) => {
                            let s = l.trim().to_string();
                            if s.is_empty() {
                                None
                            } else {
                                Some(s)
                            }
                        }
                        pulldown_cmark::CodeBlockKind::Indented => None,
                    };
                    block_stack.push(BlockBuilder::CodeBlock {
                        language: lang,
                        code: String::new(),
                    });
                }
                Tag::List(first_num) => {
                    let ordered = first_num.is_some();
                    block_stack.push(BlockBuilder::List {
                        ordered,
                        start: first_num,
                        items: Vec::new(),
                    });
                }
                Tag::Item => {
                    block_stack.push(BlockBuilder::ListItem {
                        task: None,
                        children: Vec::new(),
                    });
                }
                Tag::Table(_) => {
                    block_stack.push(BlockBuilder::Table {
                        headers: Vec::new(),
                        rows: Vec::new(),
                        in_head: false,
                        current_row: Vec::new(),
                        current_cell: Vec::new(),
                    });
                }
                Tag::TableHead => {
                    if let Some(BlockBuilder::Table { in_head, .. }) = block_stack.last_mut() {
                        *in_head = true;
                    }
                }
                Tag::TableRow => {
                    if let Some(BlockBuilder::Table { current_row, .. }) = block_stack.last_mut() {
                        current_row.clear();
                    }
                }
                Tag::TableCell => {
                    if let Some(BlockBuilder::Table { current_cell, .. }) = block_stack.last_mut() {
                        current_cell.clear();
                    }
                }
                Tag::Emphasis => {
                    push_inline_modifier(&mut block_stack, InlineModifier::Italic);
                }
                Tag::Strong => {
                    push_inline_modifier(&mut block_stack, InlineModifier::Bold);
                }
                Tag::Strikethrough => {
                    push_inline_modifier(&mut block_stack, InlineModifier::Strikethrough);
                }
                Tag::Link { dest_url, title, .. } => {
                    push_inline_modifier(
                        &mut block_stack,
                        InlineModifier::Link {
                            url: dest_url.to_string(),
                            title: if title.is_empty() {
                                None
                            } else {
                                Some(title.to_string())
                            },
                        },
                    );
                }
                Tag::Image { dest_url, title, .. } => {
                    push_inline_modifier(
                        &mut block_stack,
                        InlineModifier::Image {
                            url: dest_url.to_string(),
                            title: if title.is_empty() {
                                None
                            } else {
                                Some(title.to_string())
                            },
                        },
                    );
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    if let Some(BlockBuilder::Heading { level, id, inlines }) = block_stack.pop() {
                        let final_id = if id.is_empty() {
                            let text = extract_inlines_text_simple(&inlines);
                            slugify_heading(&text)
                        } else {
                            id
                        };
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::Heading {
                                level,
                                id: final_id,
                                children: inlines,
                            },
                        );
                    }
                }
                TagEnd::Paragraph => {
                    if let Some(BlockBuilder::Paragraph { inlines }) = block_stack.pop() {
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::Paragraph { children: inlines },
                        );
                    }
                }
                TagEnd::BlockQuote => {
                    if let Some(BlockBuilder::Blockquote { children }) = block_stack.pop() {
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::Blockquote { children },
                        );
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some(BlockBuilder::CodeBlock { language, code }) = block_stack.pop() {
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::CodeBlock { language, code },
                        );
                    }
                }
                TagEnd::Item => {
                    if let Some(BlockBuilder::ListItem { task, children }) = block_stack.pop() {
                        if let Some(BlockBuilder::List { items, .. }) = block_stack.last_mut() {
                            items.push(ListItem { task, children });
                        }
                    }
                }
                TagEnd::List(_) => {
                    if let Some(BlockBuilder::List { ordered, start, items }) = block_stack.pop() {
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::List { ordered, start, items },
                        );
                    }
                }
                TagEnd::TableCell => {
                    if let Some(BlockBuilder::Table { current_cell, current_row, .. }) = block_stack.last_mut() {
                        current_row.push(std::mem::take(current_cell));
                    }
                }
                TagEnd::TableRow => {
                    if let Some(BlockBuilder::Table { in_head, headers, rows, current_row, .. }) = block_stack.last_mut() {
                        let row = std::mem::take(current_row);
                        if *in_head {
                            *headers = row;
                        } else {
                            rows.push(row);
                        }
                    }
                }
                TagEnd::TableHead => {
                    if let Some(BlockBuilder::Table { in_head, .. }) = block_stack.last_mut() {
                        *in_head = false;
                    }
                }
                TagEnd::Table => {
                    if let Some(BlockBuilder::Table { headers, rows, .. }) = block_stack.pop() {
                        append_block(
                            &mut block_stack,
                            &mut ast.blocks,
                            BlockNode::Table { headers, rows },
                        );
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => {
                    pop_inline_modifier(&mut block_stack);
                }
                _ => {}
            },
            Event::Text(t) => {
                append_inline(&mut block_stack, InlineNode::Text(t.to_string()));
            }
            Event::Code(c) => {
                append_inline(&mut block_stack, InlineNode::Code(c.to_string()));
            }
            Event::Html(h) => {
                // If we are at the top level or inside a container block, check if this is an admonition wrapper
                let html_str = h.to_string();
                if block_stack.is_empty() {
                    ast.blocks.push(BlockNode::HtmlBlock { html: html_str });
                } else if let Some(builder) = block_stack.last_mut() {
                    match builder {
                        BlockBuilder::CodeBlock { code, .. } => code.push_str(&html_str),
                        BlockBuilder::Paragraph { inlines } | BlockBuilder::Heading { inlines, .. } => {
                            inlines.push(InlineNode::HtmlInline(html_str));
                        }
                        BlockBuilder::Table { current_cell, .. } => {
                            current_cell.push(InlineNode::HtmlInline(html_str));
                        }
                        _ => {
                            // Insert as inline if applicable
                            append_inline(&mut block_stack, InlineNode::HtmlInline(html_str));
                        }
                    }
                }
            }
            Event::TaskListMarker(checked) => {
                if let Some(BlockBuilder::ListItem { task, .. }) = block_stack.last_mut() {
                    *task = Some(checked);
                }
            }
            Event::Rule => {
                append_block(&mut block_stack, &mut ast.blocks, BlockNode::ThematicBreak);
            }
            Event::SoftBreak => {
                append_inline(&mut block_stack, InlineNode::SoftBreak);
            }
            Event::HardBreak => {
                append_inline(&mut block_stack, InlineNode::LineBreak);
            }
            _ => {}
        }
    }

    // Fold any raw admonition containers from pulldown callouts if applicable
    post_process_admonitions(&mut ast.blocks);

    ast
}

enum InlineModifier {
    Bold,
    Italic,
    Strikethrough,
    Link { url: String, title: Option<String> },
    Image { url: String, title: Option<String> },
}

enum BlockBuilder {
    Heading {
        level: u8,
        id: String,
        inlines: Vec<InlineNode>,
    },
    Paragraph {
        inlines: Vec<InlineNode>,
    },
    Blockquote {
        children: Vec<BlockNode>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    List {
        ordered: bool,
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    ListItem {
        task: Option<bool>,
        children: Vec<BlockNode>,
    },
    Table {
        headers: Vec<Vec<InlineNode>>,
        rows: Vec<Vec<Vec<InlineNode>>>,
        in_head: bool,
        current_row: Vec<Vec<InlineNode>>,
        current_cell: Vec<InlineNode>,
    },
    InlineWrap {
        modifier: InlineModifier,
        inlines: Vec<InlineNode>,
    },
}

fn push_inline_modifier(stack: &mut Vec<BlockBuilder>, modifier: InlineModifier) {
    stack.push(BlockBuilder::InlineWrap {
        modifier,
        inlines: Vec::new(),
    });
}

fn pop_inline_modifier(stack: &mut Vec<BlockBuilder>) {
    if let Some(BlockBuilder::InlineWrap { modifier, inlines }) = stack.pop() {
        let node = match modifier {
            InlineModifier::Bold => InlineNode::Bold(inlines),
            InlineModifier::Italic => InlineNode::Italic(inlines),
            InlineModifier::Strikethrough => InlineNode::Strikethrough(inlines),
            InlineModifier::Link { url, title } => InlineNode::Link {
                url,
                title,
                children: inlines,
            },
            InlineModifier::Image { url, title } => {
                let alt = extract_inlines_text_simple(&inlines);
                InlineNode::Image { url, alt, title }
            }
        };
        append_inline(stack, node);
    }
}

fn append_inline(stack: &mut [BlockBuilder], node: InlineNode) {
    if let Some(last) = stack.last_mut() {
        match last {
            BlockBuilder::Heading { inlines, .. }
            | BlockBuilder::Paragraph { inlines }
            | BlockBuilder::InlineWrap { inlines, .. } => {
                inlines.push(node);
            }
            BlockBuilder::Table { current_cell, .. } => {
                current_cell.push(node);
            }
            BlockBuilder::CodeBlock { code, .. } => {
                if let InlineNode::Text(t) = node {
                    code.push_str(&t);
                }
            }
            BlockBuilder::ListItem { children, .. } => {
                // If no active paragraph in list item, wrap in paragraph
                if let Some(BlockNode::Paragraph { children: p_inlines }) = children.last_mut() {
                    p_inlines.push(node);
                } else {
                    children.push(BlockNode::Paragraph {
                        children: vec![node],
                    });
                }
            }
            _ => {}
        }
    }
}

fn append_block(stack: &mut [BlockBuilder], root_blocks: &mut Vec<BlockNode>, block: BlockNode) {
    if let Some(last) = stack.last_mut() {
        match last {
            BlockBuilder::Blockquote { children } => {
                children.push(block);
            }
            BlockBuilder::ListItem { children, .. } => {
                children.push(block);
            }
            _ => {
                root_blocks.push(block);
            }
        }
    } else {
        root_blocks.push(block);
    }
}

fn extract_inlines_text_simple(inlines: &[InlineNode]) -> String {
    let mut out = String::new();
    for i in inlines {
        match i {
            InlineNode::Text(t) | InlineNode::Code(t) => out.push_str(t),
            InlineNode::Bold(c) | InlineNode::Italic(c) | InlineNode::Strikethrough(c) => {
                out.push_str(&extract_inlines_text_simple(c));
            }
            InlineNode::Link { children, .. } => out.push_str(&extract_inlines_text_simple(children)),
            InlineNode::Wikilink { alias, target, .. } => {
                out.push_str(alias.as_deref().unwrap_or(target));
            }
            _ => {}
        }
    }
    out
}

/// Detects blockquotes formatted like Obsidian callouts: `> [!NOTE]` or `> [!WARNING]+`
/// and transforms them into BlockNode::Admonition
fn post_process_admonitions(blocks: &mut Vec<BlockNode>) {
    for block in blocks.iter_mut() {
        match block {
            BlockNode::Blockquote { children } => {
                post_process_admonitions(children);
                if let Some(admonition) = try_convert_callout(children) {
                    *block = admonition;
                }
            }
            BlockNode::List { items, .. } => {
                for item in items {
                    post_process_admonitions(&mut item.children);
                }
            }
            BlockNode::Admonition { children, .. } => {
                post_process_admonitions(children);
            }
            _ => {}
        }
    }
}

fn try_convert_callout(children: &mut Vec<BlockNode>) -> Option<BlockNode> {
    if children.is_empty() {
        return None;
    }

    let (kind, title, collapsible, open, remaining_inlines) = {
        let first = &children[0];
        if let BlockNode::Paragraph { children: inlines } = first {
            if inlines.is_empty() {
                return None;
            }
            if let InlineNode::Text(first_text) = &inlines[0] {
                let trimmed = first_text.trim_start();
                if trimmed.starts_with("[!") {
                    if let Some(closing) = trimmed.find(']') {
                        let inner = &trimmed[2..closing];
                        let mut kind = inner.to_lowercase();
                        let mut collapsible = false;
                        let mut open = false;

                        let after_bracket = trimmed[closing + 1..].trim();
                        let (after_modifier, had_mod) = if after_bracket.starts_with('+') {
                            collapsible = true;
                            open = true;
                            (after_bracket[1..].trim(), true)
                        } else if after_bracket.starts_with('-') {
                            collapsible = true;
                            open = false;
                            (after_bracket[1..].trim(), true)
                        } else {
                            (after_bracket, false)
                        };

                        if !had_mod {
                            if kind.ends_with('+') {
                                collapsible = true;
                                open = true;
                                kind.pop();
                            } else if kind.ends_with('-') {
                                collapsible = true;
                                open = false;
                                kind.pop();
                            }
                        }

                        let custom_title = if !after_modifier.is_empty() {
                            Some(after_modifier.to_string())
                        } else {
                            None
                        };

                        let rest_inlines = if inlines.len() > 1 {
                            inlines[1..].to_vec()
                        } else {
                            Vec::new()
                        };

                        (kind, custom_title, collapsible, open, rest_inlines)
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            return None;
        }
    };

    let mut new_children = Vec::new();
    if !remaining_inlines.is_empty() {
        new_children.push(BlockNode::Paragraph {
            children: remaining_inlines,
        });
    }
    for child in children.drain(1..) {
        new_children.push(child);
    }

    Some(BlockNode::Admonition {
        kind,
        title,
        collapsible,
        open,
        children: new_children,
    })
}
