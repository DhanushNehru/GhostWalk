//! Flat arena DOM storage.
//!
//! Instead of `Rc`/pointer-linked nodes, every node lives in one
//! contiguous `Vec<Node>` and is referenced by a compact [`NodeId`].
//! This keeps allocations cache-friendly, avoids borrow-checker fights,
//! and is the storage model the rest of the engine builds on.
//! See ARCHITECTURE.md for the long-term data-oriented plan.

use html5ever::interface::tree_builder::QuirksMode;
use html5ever::{ns, Attribute, LocalName, QualName};
use std::num::NonZeroU32;

/// Handle to a node in the arena. 4 bytes, `Copy`, no lifetime.
///
/// The arena is append-only: a `NodeId` stays valid for the lifetime
/// of its [`Dom`]. (Generational validation is on the roadmap.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    pub(crate) fn new(index: usize) -> NodeId {
        // +1 so index 0 maps to a non-zero value and Option<NodeId> stays 4 bytes.
        NodeId(NonZeroU32::new((index as u32) + 1).expect("arena index overflow"))
    }

    /// Position of this node inside the arena.
    pub fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }
}

/// Data carried by an element node.
#[derive(Debug, Clone)]
pub struct ElementData {
    pub name: QualName,
    pub attrs: Vec<Attribute>,
    /// `<template>` contents fragment, if this element is a template.
    pub template_contents: Option<NodeId>,
    pub mathml_annotation_xml_integration_point: bool,
}

/// The payload of one arena slot.
#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element(ElementData),
    Text(String),
    Comment(String),
    ProcessingInstruction {
        target: String,
        data: String,
    },
}

/// One node in the arena: intrusive child/sibling links as arena indices.
#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub prev_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub data: NodeData,
}

impl Node {
    fn detached(data: NodeData) -> Node {
        Node {
            parent: None,
            prev_sibling: None,
            next_sibling: None,
            first_child: None,
            last_child: None,
            data,
        }
    }
}

/// The document: a flat vector of nodes plus document-level state.
#[derive(Debug)]
pub struct Dom {
    nodes: Vec<Node>,
    root: NodeId,
    quirks_mode: QuirksMode,
    parse_error_count: u64,
}

impl Default for Dom {
    fn default() -> Self {
        Dom::new()
    }
}

impl Dom {
    pub fn new() -> Self {
        let nodes = vec![Node::detached(NodeData::Document)];
        Dom {
            nodes,
            root: NodeId::new(0),
            quirks_mode: QuirksMode::NoQuirks,
            parse_error_count: 0,
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    /// Number of HTML parse errors reported by the tree builder.
    pub fn parse_error_count(&self) -> u64 {
        self.parse_error_count
    }

    pub fn note_parse_error(&mut self) {
        self.parse_error_count += 1;
    }

    /// Number of slots in the arena (including detached nodes).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    /// Allocate a new detached node, returning its id.
    pub fn create_node(&mut self, data: NodeData) -> NodeId {
        let id = NodeId::new(self.nodes.len());
        self.nodes.push(Node::detached(data));
        id
    }

    /// Convenience: create a detached HTML element in the HTML namespace.
    pub fn create_html_element(&mut self, tag: &str) -> NodeId {
        self.create_node(NodeData::Element(ElementData {
            name: QualName::new(None, ns!(html), LocalName::from(tag)),
            attrs: Vec::new(),
            template_contents: None,
            mathml_annotation_xml_integration_point: false,
        }))
    }

    pub fn element(&self, id: NodeId) -> Option<&ElementData> {
        match &self.node(id).data {
            NodeData::Element(el) => Some(el),
            _ => None,
        }
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Element(_))
    }

    /// Lowercased local tag name, if `id` is an element.
    pub fn tag_name(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(|el| &*el.name.local)
    }

    /// Iterate over the direct children of `id` in document order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            dom: self,
            next: self.node(id).first_child,
        }
    }

    /// Preorder traversal of the subtree rooted at `id` (including `id`).
    /// Iterative: hostile deeply-nested documents must not overflow the stack.
    pub fn descendants(&self, root: NodeId) -> Descendants<'_> {
        Descendants {
            dom: self,
            stack: vec![root],
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    /// Detach `id` from its parent and siblings (its own subtree is kept).
    pub fn detach(&mut self, id: NodeId) {
        let (parent, prev, next) = {
            let n = self.node(id);
            (n.parent, n.prev_sibling, n.next_sibling)
        };
        if let Some(p) = prev {
            self.node_mut(p).next_sibling = next;
        }
        if let Some(n) = next {
            self.node_mut(n).prev_sibling = prev;
        }
        if let Some(p) = parent {
            if self.node(p).first_child == Some(id) {
                self.node_mut(p).first_child = next;
            }
            if self.node(p).last_child == Some(id) {
                self.node_mut(p).last_child = prev;
            }
        }
        let n = self.node_mut(id);
        n.parent = None;
        n.prev_sibling = None;
        n.next_sibling = None;
    }

    /// Append `child` as the last child of `parent`, detaching it first.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        let last = self.node(parent).last_child;
        self.node_mut(child).parent = Some(parent);
        self.node_mut(child).prev_sibling = last;
        self.node_mut(child).next_sibling = None;
        match last {
            Some(l) => self.node_mut(l).next_sibling = Some(child),
            None => self.node_mut(parent).first_child = Some(child),
        }
        self.node_mut(parent).last_child = Some(child);
    }

    /// Insert `node` immediately before `sibling`, detaching it first.
    pub fn insert_before(&mut self, sibling: NodeId, node: NodeId) {
        let (parent, prev) = {
            let s = self.node(sibling);
            (s.parent, s.prev_sibling)
        };
        self.detach(node);
        self.node_mut(node).parent = parent;
        self.node_mut(node).prev_sibling = prev;
        self.node_mut(node).next_sibling = Some(sibling);
        self.node_mut(sibling).prev_sibling = Some(node);
        match prev {
            Some(p) => self.node_mut(p).next_sibling = Some(node),
            None => {
                if let Some(par) = parent {
                    self.node_mut(par).first_child = Some(node);
                }
            }
        }
    }

    /// Move all children of `node` to `new_parent` (in order).
    pub fn reparent_children(&mut self, node: NodeId, new_parent: NodeId) {
        let kids: Vec<NodeId> = self.children(node).collect();
        for kid in kids {
            self.append(new_parent, kid);
        }
    }

    /// Add each attribute that is not already present on the element.
    pub fn add_attrs_if_missing(&mut self, target: NodeId, attrs: Vec<Attribute>) {
        if let NodeData::Element(ref mut el) = self.node_mut(target).data {
            for attr in attrs {
                let exists = el.attrs.iter().any(|a| a.name == attr.name);
                if !exists {
                    el.attrs.push(attr);
                }
            }
        }
    }

    /// Get an attribute by (case-insensitive) local name.
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let el = self.element(id)?;
        el.attrs
            .iter()
            .find(|a| (*a.name.local).eq_ignore_ascii_case(name))
            .map(|a| &*a.value)
    }

    /// Set (or replace) an attribute by local name.
    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        if let NodeData::Element(ref mut el) = self.node_mut(id).data {
            if let Some(existing) = el
                .attrs
                .iter_mut()
                .find(|a| (*a.name.local).eq_ignore_ascii_case(name))
            {
                existing.value = value.into();
            } else {
                el.attrs.push(Attribute {
                    name: QualName::new(None, ns!(), LocalName::from(name)),
                    value: value.into(),
                });
            }
        }
    }

    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        self.attr(id, "class")
            .map(|c| c.split_whitespace().any(|c| c == class))
            .unwrap_or(false)
    }

    /// Concatenated text of the subtree (script/style included; the
    /// extraction layer decides what to skip). Iterative.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            let node = self.node(cur);
            match &node.data {
                NodeData::Text(t) => out.push_str(t),
                _ => {
                    // Push children in reverse so they pop in document order.
                    let kids: Vec<NodeId> = self.children(cur).collect();
                    for kid in kids.into_iter().rev() {
                        stack.push(kid);
                    }
                }
            }
        }
        out
    }

    /// Replace the children of `id` with a single text node.
    ///
    /// Old child slots stay in the arena (append-only storage); they are
    /// unreachable garbage until arena compaction lands (roadmap).
    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        let kids: Vec<NodeId> = self.children(id).collect();
        for kid in kids {
            self.detach(kid);
        }
        if !text.is_empty() {
            let t = self.create_node(NodeData::Text(text.to_string()));
            self.append(id, t);
        }
    }

    /// First element in document order with this tag name (case-insensitive).
    pub fn find_first_element_by_tag(&self, tag: &str) -> Option<NodeId> {
        self.descendants(self.root()).find(|&id| {
            self.tag_name(id)
                .map(|t| t.eq_ignore_ascii_case(tag))
                .unwrap_or(false)
        })
    }
}

/// Child iterator.
pub struct Children<'a> {
    dom: &'a Dom,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.dom.node(cur).next_sibling;
        Some(cur)
    }
}

/// Preorder subtree iterator.
pub struct Descendants<'a> {
    dom: &'a Dom,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let cur = self.stack.pop()?;
        let kids: Vec<NodeId> = self.dom.children(cur).collect();
        for kid in kids.into_iter().rev() {
            self.stack.push(kid);
        }
        Some(cur)
    }
}
