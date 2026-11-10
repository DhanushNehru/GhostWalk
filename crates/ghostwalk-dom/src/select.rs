//! Minimal CSS selector engine: a deliberate subset, matched against
//! the arena without any extra index structures.
//!
//! Supported (MVP):
//!   - type selectors: `div`, `h1`
//!   - `*` universal
//!   - `#id` and `.class` (compound: `div.card#main`)
//!   - descendant combinator: `main article p`
//!   - selector lists: `h1, h2`
//!
//! Child (`>`), sibling (`+`, `~`), attribute and pseudo selectors are
//! tracked as a contributor issue.

use crate::node::{Dom, NodeId};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    Universal,
    Tag(String),
    Id(String),
    Class(String),
}

/// One compound selector, e.g. `div.card#main` -> [Tag, Class, Id].
type Compound = Vec<Condition>;

/// A full selector: list of compound chains (split on `,`),
/// each chain split on whitespace (descendant combinator).
#[derive(Debug, Clone)]
pub struct Selector {
    groups: Vec<Vec<Compound>>,
}

/// Parse error for an unsupported or malformed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorError(pub String);

impl std::fmt::Display for SelectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported selector: {}", self.0)
    }
}

impl std::error::Error for SelectorError {}

/// Parse a selector string into the supported subset.
pub fn parse_selector(input: &str) -> Result<Selector, SelectorError> {
    let mut groups = Vec::new();
    for group in input.split(',') {
        let group = group.trim();
        if group.is_empty() {
            return Err(SelectorError(input.to_string()));
        }
        let mut chain = Vec::new();
        for part in group.split_whitespace() {
            chain.push(parse_compound(part)?);
        }
        groups.push(chain);
    }
    Ok(Selector { groups })
}

fn parse_compound(part: &str) -> Result<Compound, SelectorError> {
    let mut conditions = Vec::new();
    let chars: Vec<char> = part.chars().collect();
    let mut i = 0;
    // Optional leading type or universal selector.
    if i < chars.len() && chars[i] == '*' {
        conditions.push(Condition::Universal);
        i += 1;
    } else if i < chars.len() && (chars[i].is_ascii_alphabetic()) {
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
            i += 1;
        }
        conditions.push(Condition::Tag(chars[start..i].iter().collect()));
    }
    while i < chars.len() {
        let marker = chars[i];
        if marker != '#' && marker != '.' {
            return Err(SelectorError(part.to_string()));
        }
        i += 1;
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-' || chars[i] == '_') {
            i += 1;
        }
        if start == i {
            return Err(SelectorError(part.to_string()));
        }
        let value: String = chars[start..i].iter().collect();
        conditions.push(if marker == '#' {
            Condition::Id(value)
        } else {
            Condition::Class(value)
        });
    }
    if conditions.is_empty() {
        return Err(SelectorError(part.to_string()));
    }
    Ok(conditions)
}

fn matches_compound(dom: &Dom, id: NodeId, compound: &[Condition]) -> bool {
    if !dom.is_element(id) {
        return false;
    }
    compound.iter().all(|cond| match cond {
        Condition::Universal => true,
        Condition::Tag(tag) => dom
            .tag_name(id)
            .map(|t| t.eq_ignore_ascii_case(tag))
            .unwrap_or(false),
        Condition::Id(want) => dom.attr(id, "id") == Some(want.as_str()),
        Condition::Class(class) => dom.has_class(id, class),
    })
}

/// Right-to-left match: last compound must match `node`, each earlier
/// compound must match some ancestor (descendant combinator).
fn matches_chain(dom: &Dom, node: NodeId, chain: &[Compound]) -> bool {
    let (last, rest) = match chain.split_last() {
        Some(v) => v,
        None => return false,
    };
    if !matches_compound(dom, node, last) {
        return false;
    }
    let mut ancestor = dom.parent(node);
    for compound in rest.iter().rev() {
        let mut found = None;
        let mut cur = ancestor;
        while let Some(a) = cur {
            if matches_compound(dom, a, compound) {
                found = Some(a);
                break;
            }
            cur = dom.parent(a);
        }
        match found {
            Some(a) => ancestor = dom.parent(a),
            None => return false,
        }
    }
    true
}

/// All elements matching `selector`, in document order.
pub fn query_selector_all(dom: &Dom, selector: &str) -> Result<Vec<NodeId>, SelectorError> {
    let parsed = parse_selector(selector)?;
    let mut out = Vec::new();
    for id in dom.descendants(dom.root()) {
        if !dom.is_element(id) {
            continue;
        }
        if parsed
            .groups
            .iter()
            .any(|chain| matches_chain(dom, id, chain))
        {
            out.push(id);
        }
    }
    Ok(out)
}

/// First element matching `selector`, in document order.
pub fn query_selector(dom: &Dom, selector: &str) -> Result<Option<NodeId>, SelectorError> {
    let parsed = parse_selector(selector)?;
    for id in dom.descendants(dom.root()) {
        if !dom.is_element(id) {
            continue;
        }
        if parsed
            .groups
            .iter()
            .any(|chain| matches_chain(dom, id, chain))
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_html;

    #[test]
    fn selects_by_tag_id_class_and_descendant() {
        let dom = parse_html(
            r#"<!doctype html><html><body>
               <div id="app" class="card big"><p>one</p><p class="x">two</p></div>
               <p>three</p></body></html>"#,
        );
        assert_eq!(query_selector_all(&dom, "p").unwrap().len(), 3);
        assert_eq!(query_selector_all(&dom, "#app").unwrap().len(), 1);
        assert_eq!(query_selector_all(&dom, ".card").unwrap().len(), 1);
        assert_eq!(query_selector_all(&dom, "div.card").unwrap().len(), 1);
        assert_eq!(query_selector_all(&dom, "div p").unwrap().len(), 2);
        assert_eq!(query_selector_all(&dom, "p, #app").unwrap().len(), 4);
        assert_eq!(query_selector(&dom, "div p.x").unwrap().is_some(), true);
        assert!(parse_selector("div > p").is_err());
    }
}
