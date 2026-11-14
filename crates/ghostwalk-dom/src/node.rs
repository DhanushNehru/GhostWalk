//! Flat arena DOM storage.
//!
//! Instead of `Rc`/pointer-linked nodes, every node lives in one
//! contiguous arena and is referenced by a compact [`NodeId`].
//! This keeps allocations cache-friendly, avoids borrow-checker fights,
//! and is the storage model the rest of the engine builds on.
//! See ARCHITECTURE.md for the long-term data-oriented plan.

use html5ever::interface::tree_builder::QuirksMode;
use html5ever::{ns, Attribute, LocalName, QualName};
use std::fmt;

/// Handle to a node in its originating [`Dom`]. Live handles survive compaction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// Stable slot index. A recycled slot has the same index and a new generation.
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// A node handle whose slot is vacant, out of bounds, or has been recycled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNodeId(NodeId);

impl fmt::Display for InvalidNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid or stale DOM node handle {:?}", self.0)
    }
}

impl std::error::Error for InvalidNodeId {}

/// Data carried by an element node.
#[derive(Debug, Clone)]
pub struct ElementData {
    pub name: QualName,
    pub attrs: Vec<Attribute>,
    /// `<template>` contents fragment, if this element is a template.
    pub template_contents: Option<NodeId>,
    pub mathml_annotation_xml_integration_point: bool,
}

/// The payload of one arena node.
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

/// One node in the arena, with child and sibling links stored as handles.
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

#[derive(Debug)]
struct Slot {
    generation: u32,
    position: Option<usize>,
}

#[derive(Debug)]
struct ArenaNode {
    slot: u32,
    node: Node,
}

/// The document: dense node storage, stable handle slots, and document state.
#[derive(Debug)]
pub struct Dom {
    nodes: Vec<ArenaNode>,
    slots: Vec<Slot>,
    free_slots: Vec<u32>,
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
        Dom {
            nodes: vec![ArenaNode {
                slot: 0,
                node: Node::detached(NodeData::Document),
            }],
            slots: vec![Slot {
                generation: 0,
                position: Some(0),
            }],
            free_slots: Vec::new(),
            root: NodeId {
                index: 0,
                generation: 0,
            },
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

    /// Number of stored nodes, including detached nodes awaiting compaction.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn node_position(&self, id: NodeId) -> Result<usize, InvalidNodeId> {
        match self.slots.get(id.index()) {
            Some(slot) if slot.generation == id.generation => {
                slot.position.ok_or(InvalidNodeId(id))
            }
            _ => Err(InvalidNodeId(id)),
        }
    }

    /// Look up a handle, returning an error if the node has been reclaimed.
    pub fn try_node(&self, id: NodeId) -> Result<&Node, InvalidNodeId> {
        let position = self.node_position(id)?;
        Ok(&self.nodes[position].node)
    }

    /// Look up a mutable node, returning an error if it has been reclaimed.
    pub fn try_node_mut(&mut self, id: NodeId) -> Result<&mut Node, InvalidNodeId> {
        let position = self.node_position(id)?;
        Ok(&mut self.nodes[position].node)
    }

    /// Look up a live handle. Panics if the handle is invalid.
    pub fn node(&self, id: NodeId) -> &Node {
        self.try_node(id)
            .expect("DOM lookup requires a live handle")
    }

    /// Look up a mutable node. Panics if the handle is invalid.
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.try_node_mut(id)
            .expect("DOM lookup requires a live handle")
    }

    /// Allocate a new detached node, returning its id.
    pub fn create_node(&mut self, data: NodeData) -> NodeId {
        let position = Some(self.nodes.len());
        let (index, generation) = if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.slots[index as usize];
            slot.generation += 1;
            slot.position = position;
            (index, slot.generation)
        } else {
            let index = u32::try_from(self.slots.len()).expect("arena index overflow");
            self.slots.push(Slot {
                generation: 0,
                position,
            });
            (index, 0)
        };
        self.nodes.push(ArenaNode {
            slot: index,
            node: Node::detached(data),
        });
        NodeId { index, generation }
    }

    /// Reclaim nodes unreachable from the document, including detached subtrees.
    ///
    /// Template contents and their descendants remain live when their template
    /// is reachable. Handles to surviving nodes stay valid as storage moves.
    /// Call this when detached nodes are no longer needed for reattachment.
    /// Returns the number of nodes reclaimed.
    pub fn compact(&mut self) -> usize {
        let before = self.nodes.len();
        let mut reachable = vec![false; before];
        let mut pending = vec![self.root];
        while let Some(id) = pending.pop() {
            let position = self.node_position(id).expect("invalid DOM link");
            if std::mem::replace(&mut reachable[position], true) {
                continue;
            }
            let node = &self.nodes[position].node;
            pending.extend(
                [
                    node.parent,
                    node.prev_sibling,
                    node.next_sibling,
                    node.first_child,
                    node.last_child,
                ]
                .into_iter()
                .flatten(),
            );
            if let NodeData::Element(el) = &node.data {
                pending.extend(el.template_contents);
            }
        }

        let mut position = 0;
        self.nodes.retain(|entry| {
            let slot = &mut self.slots[entry.slot as usize];
            let keep = reachable[slot.position.expect("stored node has a live slot")];
            if keep {
                slot.position = Some(position);
                position += 1;
            } else {
                slot.position = None;
                // Retire exhausted slots so their generations never wrap.
                if slot.generation < u32::MAX {
                    self.free_slots.push(entry.slot);
                }
            }
            keep
        });
        before - self.nodes.len()
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
        self.node_position(parent).expect("invalid parent handle");
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
    /// Detached children remain accessible through existing handles until compaction.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{extract, parse_html, select};

    #[test]
    fn reclaimed_handles_stay_invalid_after_slot_reuse() {
        let mut dom = Dom::new();
        let stale = dom.create_html_element("div");
        dom.append(dom.root(), stale);
        dom.detach(stale);
        assert_eq!(dom.tag_name(stale), Some("div"));

        assert_eq!(dom.compact(), 1);
        assert!(dom.try_node(stale).is_err());
        assert_eq!(dom.compact(), 0);

        let replacement = dom.create_html_element("p");
        dom.append(dom.root(), replacement);
        assert_eq!(stale.index(), replacement.index());
        assert_ne!(stale, replacement);
        assert!(dom.try_node(stale).is_err());
        assert!(dom.try_node_mut(stale).is_err());
        assert_eq!(dom.tag_name(replacement), Some("p"));
    }

    #[test]
    fn compaction_preserves_live_handles_and_document_output() {
        let mut dom = parse_html(
            "<main><div id='replace'><template><p>discarded template</p></template><b>old</b></div>\
             <p id='keep'>Keep <em>this</em></p><template id='live'><p>template contents</p></template></main>",
        );
        let replace = select::query_selector(&dom, "#replace").unwrap().unwrap();
        let kept = select::query_selector(&dom, "#keep").unwrap().unwrap();
        let discarded = select::query_selector(&dom, "#replace template")
            .unwrap()
            .unwrap();
        let live_template = select::query_selector(&dom, "#live").unwrap().unwrap();
        let discarded_fragment = dom.element(discarded).unwrap().template_contents.unwrap();
        let live_fragment = dom
            .element(live_template)
            .unwrap()
            .template_contents
            .unwrap();
        dom.set_text_content(replace, "new");

        let kept_position = dom.node_position(kept).unwrap();
        let before = dom.node_count();
        let tree = extract::dump_tree(&dom);
        let text = extract::extract_text(&dom);
        let markdown = extract::extract_markdown(&dom);
        assert_eq!(dom.compact(), 6);
        assert_eq!(dom.node_count(), before - 6);
        assert!(dom.node_position(kept).unwrap() < kept_position);
        assert!(dom.try_node(discarded).is_err());
        assert!(dom.try_node(discarded_fragment).is_err());
        assert_eq!(dom.text_content(live_fragment), "template contents");
        assert_eq!(select::query_selector(&dom, "#keep").unwrap(), Some(kept));
        assert_eq!(dom.text_content(kept), "Keep this");
        assert_eq!(extract::dump_tree(&dom), tree);
        assert_eq!(extract::extract_text(&dom), text);
        assert_eq!(extract::extract_markdown(&dom), markdown);
    }
}
