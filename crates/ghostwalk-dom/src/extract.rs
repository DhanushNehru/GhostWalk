//! Read-facing projections of the arena DOM:
//! indented tree dumps, readable text, and semantic Markdown.
//!
//! The Markdown extractor is one of ghostwalk's agent-first features:
//! structure-preserving output straight from the DOM, without a
//! readability sidecar or a second parse.

use crate::node::{Dom, NodeData, NodeId};

/// Elements whose contents are never rendered text.
const SKIP_CONTENT: &[&str] = &["script", "style", "noscript", "template", "head"];

/// Block-level elements that break lines in text/markdown output.
const BLOCK: &[&str] = &[
    "address", "article", "aside", "blockquote", "br", "dd", "details", "div", "dl", "dt",
    "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6",
    "header", "hr", "li", "main", "nav", "ol", "p", "pre", "section", "table", "td", "th", "tr", "ul",
];

fn is_skip(tag: &str) -> bool {
    SKIP_CONTENT.contains(&tag)
}

fn is_block(tag: &str) -> bool {
    BLOCK.contains(&tag)
}

/// Indented structural dump of the tree, one line per node.
pub fn dump_tree(dom: &Dom) -> String {
    let mut out = String::new();
    dump_node(dom, dom.root(), 0, &mut out);
    out
}

fn dump_node(dom: &Dom, id: NodeId, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match &dom.node(id).data {
        NodeData::Document => {
            out.push_str(&format!("{indent}#document\n"));
        }
        NodeData::Doctype { name, .. } => {
            out.push_str(&format!("{indent}<!DOCTYPE {name}>\n"));
        }
        NodeData::Element(el) => {
            let mut label = String::from("<");
            label.push_str(&el.name.local);
            if let Some(id_attr) = dom.attr(id, "id") {
                label.push_str(&format!("#{id_attr}"));
            }
            if let Some(class) = dom.attr(id, "class") {
                for c in class.split_whitespace() {
                    label.push_str(&format!(".{c}"));
                }
            }
            label.push('>');
            out.push_str(&format!("{indent}{label}\n"));
        }
        NodeData::Text(t) => {
            let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            if !trimmed.is_empty() {
                let shown: String = trimmed.chars().take(60).collect();
                out.push_str(&format!("{indent}\"{shown}\"\n"));
            }
        }
        NodeData::Comment(c) => {
            let shown: String = c.chars().take(60).collect();
            out.push_str(&format!("{indent}<!--{shown}-->\n"));
        }
        NodeData::ProcessingInstruction { target, .. } => {
            out.push_str(&format!("{indent}<?{target}>\n"));
        }
    }
    for kid in dom.children(id) {
        dump_node(dom, kid, depth + 1, out);
    }
}

/// Readable plain text: block elements break lines, skipped elements
/// and pure-whitespace text nodes are dropped.
pub fn extract_text(dom: &Dom) -> String {
    let mut out = String::new();
    let mut stack: Vec<(NodeId, bool)> = vec![(dom.root(), false)];
    while let Some((id, skip)) = stack.pop() {
        match &dom.node(id).data {
            NodeData::Text(t) if !skip => {
                out.push_str(t);
            }
            NodeData::Element(el) => {
                let tag = el.name.local.to_string();
                let child_skip = skip || is_skip(&tag);
                if !child_skip && is_block(&tag) {
                    out.push('\n');
                }
                let kids: Vec<NodeId> = dom.children(id).collect();
                for kid in kids.into_iter().rev() {
                    stack.push((kid, child_skip));
                }
                if !child_skip && is_block(&tag) {
                    out.push('\n');
                }
            }
            _ => {
                let kids: Vec<NodeId> = dom.children(id).collect();
                for kid in kids.into_iter().rev() {
                    stack.push((kid, skip));
                }
            }
        }
    }
    normalize_text(&out)
}

fn normalize_text(raw: &str) -> String {
    let mut lines: Vec<String> = raw
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// Semantic Markdown: headings, links, lists, emphasis, code, tables
/// (tables land as preformatted text in the MVP).
pub fn extract_markdown(dom: &Dom) -> String {
    let mut out = String::new();
    render_md(dom, dom.root(), 0, &mut out);
    normalize_markdown(&out)
}

const MAX_DEPTH: usize = 256;

fn render_md(dom: &Dom, id: NodeId, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH {
        return;
    }
    match &dom.node(id).data {
        NodeData::Text(t) => out.push_str(t),
        NodeData::Element(el) => {
            let tag = el.name.local.to_string();
            if is_skip(&tag) {
                return;
            }
            match tag.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level: usize = tag[1..].parse().unwrap_or(1);
                    out.push_str(&format!("\n\n{} ", "#".repeat(level)));
                    render_children(dom, id, depth, out);
                    out.push_str("\n\n");
                }
                "a" => {
                    let mut inner = String::new();
                    render_children(dom, id, depth, &mut inner);
                    let inner = inner.trim().to_string();
                    match dom.attr(id, "href") {
                        Some(href) if !href.is_empty() => {
                            out.push_str(&format!("[{inner}]({href})"));
                        }
                        _ => out.push_str(&inner),
                    }
                }
                "li" => {
                    out.push_str("\n- ");
                    render_children(dom, id, depth, out);
                }
                "strong" | "b" => {
                    out.push_str("**");
                    render_children(dom, id, depth, out);
                    out.push_str("**");
                }
                "em" | "i" => {
                    out.push('*');
                    render_children(dom, id, depth, out);
                    out.push('*');
                }
                "code" => {
                    out.push('`');
                    render_children(dom, id, depth, out);
                    out.push('`');
                }
                "pre" => {
                    out.push_str("\n\n```\n");
                    out.push_str(&dom.text_content(id));
                    out.push_str("\n```\n\n");
                }
                "img" => {
                    let alt = dom.attr(id, "alt").unwrap_or("");
                    if let Some(src) = dom.attr(id, "src") {
                        out.push_str(&format!("![{alt}]({src})"));
                    }
                }
                "blockquote" => {
                    let mut inner = String::new();
                    render_children(dom, id, depth, &mut inner);
                    for line in inner.trim().lines() {
                        out.push_str(&format!("\n> {line}"));
                    }
                    out.push('\n');
                }
                "br" => out.push('\n'),
                "tr" => {
                    out.push('\n');
                    render_children(dom, id, depth, out);
                }
                "td" | "th" => {
                    out.push_str(" | ");
                    render_children(dom, id, depth, out);
                }
                "hr" => out.push_str("\n\n---\n\n"),
                _ => {
                    if is_block(&tag) {
                        out.push_str("\n\n");
                    }
                    render_children(dom, id, depth, out);
                    if is_block(&tag) {
                        out.push_str("\n\n");
                    }
                }
            }
        }
        _ => render_children(dom, id, depth, out),
    }
}

fn render_children(dom: &Dom, id: NodeId, depth: usize, out: &mut String) {
    for kid in dom.children(id) {
        render_md(dom, kid, depth + 1, out);
    }
}

fn normalize_markdown(raw: &str) -> String {
    // Collapse horizontal whitespace per line, then 3+ newlines to 2.
    let lines: Vec<String> = raw
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let mut out = String::new();
    let mut blank = 0;
    for line in lines {
        if line.is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}
