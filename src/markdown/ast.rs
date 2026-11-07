use serde::{Deserialize, Serialize};

/// Document-level Abstract Syntax Tree (Object Model)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocAst {
    pub frontmatter: serde_json::Value,
    pub title: String,
    pub title_source: String,
    pub blocks: Vec<BlockNode>,
    pub links: Vec<String>,
}

impl DocAst {
    pub fn new(title: String, title_source: String, frontmatter: serde_json::Value) -> Self {
        Self {
            frontmatter,
            title,
            title_source,
            blocks: Vec::new(),
            links: Vec::new(),
        }
    }

    /// Serialize this AST to CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)?;
        Ok(bytes)
    }

    /// Deserialize AST from CBOR bytes
    pub fn from_cbor(bytes: &[u8]) -> Result<Self, ciborium::de::Error<std::io::Error>> {
        ciborium::from_reader(bytes)
    }

    /// Extract headings for Table of Contents generation: (level, text, slug)
    pub fn extract_headings(&self) -> Vec<(usize, String, String)> {
        let mut headings = Vec::new();
        for block in &self.blocks {
            if let BlockNode::Heading { level, id, children } = block {
                let text = extract_inlines_text(children);
                let slug = if id.is_empty() {
                    slugify_heading(&text)
                } else {
                    id.clone()
                };
                headings.push((*level as usize, text, slug));
            }
        }
        headings
    }

    /// Extract plain text from all blocks for FTS search indexing
    pub fn extract_plain_text(&self) -> String {
        let mut buf = String::new();
        for block in &self.blocks {
            extract_block_text(block, &mut buf);
            buf.push('\n');
        }
        buf.trim().to_string()
    }
}

/// Block-level AST nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum BlockNode {
    Heading {
        level: u8,
        id: String,
        children: Vec<InlineNode>,
    },
    Paragraph {
        children: Vec<InlineNode>,
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
    Table {
        headers: Vec<Vec<InlineNode>>,
        rows: Vec<Vec<Vec<InlineNode>>>,
    },
    Admonition {
        kind: String,
        title: Option<String>,
        collapsible: bool,
        open: bool,
        children: Vec<BlockNode>,
    },
    ThematicBreak,
    HtmlBlock {
        html: String,
    },
    CustomBlock {
        name: String,
        attributes: std::collections::HashMap<String, String>,
        children: Vec<BlockNode>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListItem {
    pub task: Option<bool>, // Some(true) = checked, Some(false) = unchecked, None = normal item
    pub children: Vec<BlockNode>,
}

/// Inline-level AST nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum InlineNode {
    Text(String),
    Bold(Vec<InlineNode>),
    Italic(Vec<InlineNode>),
    Strikethrough(Vec<InlineNode>),
    Code(String),
    Link {
        url: String,
        title: Option<String>,
        children: Vec<InlineNode>,
    },
    Image {
        url: String,
        alt: String,
        title: Option<String>,
    },
    Wikilink {
        target: String,
        alias: Option<String>,
        resolved_url: Option<String>,
    },
    HtmlInline(String),
    LineBreak,
    SoftBreak,
}

fn extract_inlines_text(inlines: &[InlineNode]) -> String {
    let mut buf = String::new();
    for inline in inlines {
        match inline {
            InlineNode::Text(t) => buf.push_str(t),
            InlineNode::Bold(children)
            | InlineNode::Italic(children)
            | InlineNode::Strikethrough(children) => buf.push_str(&extract_inlines_text(children)),
            InlineNode::Code(c) => buf.push_str(c),
            InlineNode::Link { children, .. } => buf.push_str(&extract_inlines_text(children)),
            InlineNode::Image { alt, .. } => buf.push_str(alt),
            InlineNode::Wikilink { alias, target, .. } => {
                buf.push_str(alias.as_deref().unwrap_or(target));
            }
            InlineNode::HtmlInline(_) => {}
            InlineNode::LineBreak | InlineNode::SoftBreak => buf.push(' '),
        }
    }
    buf
}

fn extract_block_text(block: &BlockNode, buf: &mut String) {
    match block {
        BlockNode::Heading { children, .. } | BlockNode::Paragraph { children } => {
            buf.push_str(&extract_inlines_text(children));
            buf.push('\n');
        }
        BlockNode::Blockquote { children } => {
            for child in children {
                extract_block_text(child, buf);
            }
        }
        BlockNode::CodeBlock { code, .. } => {
            buf.push_str(code);
            buf.push('\n');
        }
        BlockNode::List { items, .. } => {
            for item in items {
                for child in &item.children {
                    extract_block_text(child, buf);
                }
            }
        }
        BlockNode::Table { headers, rows } => {
            for header in headers {
                buf.push_str(&extract_inlines_text(header));
                buf.push(' ');
            }
            buf.push('\n');
            for row in rows {
                for cell in row {
                    buf.push_str(&extract_inlines_text(cell));
                    buf.push(' ');
                }
                buf.push('\n');
            }
        }
        BlockNode::Admonition { title, children, .. } => {
            if let Some(t) = title {
                buf.push_str(t);
                buf.push('\n');
            }
            for child in children {
                extract_block_text(child, buf);
            }
        }
        BlockNode::CustomBlock { children, .. } => {
            for child in children {
                extract_block_text(child, buf);
            }
        }
        BlockNode::HtmlBlock { html } => {
            buf.push_str(html);
            buf.push('\n');
        }
        BlockNode::ThematicBreak => {}
    }
}

pub fn slugify_heading(title: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ast_cbor_roundtrip() {
        let mut ast = DocAst::new(
            "Test Document".to_string(),
            "frontmatter".to_string(),
            serde_json::json!({"tags": ["rust", "cms"]}),
        );
        ast.blocks.push(BlockNode::Heading {
            level: 1,
            id: "test-document".to_string(),
            children: vec![InlineNode::Text("Test Document".to_string())],
        });
        ast.blocks.push(BlockNode::Paragraph {
            children: vec![
                InlineNode::Text("Hello ".to_string()),
                InlineNode::Bold(vec![InlineNode::Text("World".to_string())]),
                InlineNode::Wikilink {
                    target: "TargetPage".to_string(),
                    alias: Some("My Target".to_string()),
                    resolved_url: Some("/target-page".to_string()),
                },
            ],
        });
        ast.blocks.push(BlockNode::Admonition {
            kind: "note".to_string(),
            title: Some("Important Note".to_string()),
            collapsible: true,
            open: true,
            children: vec![BlockNode::Paragraph {
                children: vec![InlineNode::Text("Admonition content".to_string())],
            }],
        });

        let cbor_bytes = ast.to_cbor().expect("serialize to cbor");
        assert!(!cbor_bytes.is_empty());

        let deserialized = DocAst::from_cbor(&cbor_bytes).expect("deserialize from cbor");
        assert_eq!(ast, deserialized);
        assert_eq!(deserialized.extract_headings(), vec![(1, "Test Document".to_string(), "test-document".to_string())]);
        assert!(deserialized.extract_plain_text().contains("Hello World"));
        assert!(deserialized.extract_plain_text().contains("Important Note"));
    }
}
