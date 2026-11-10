//! html5ever tree-builder sink that writes directly into the flat arena.
//!
//! The spec-compliant tokenizer and tree builder come from `html5ever`
//! (the Servo project's parser); ghostwalk owns the storage layer. The sink
//! receives tree-builder callbacks and turns them into arena operations.

use std::borrow::Cow;
use std::cell::RefCell;

use html5ever::interface::tree_builder::{
    ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink,
};
use html5ever::{parse_document, Attribute, LocalName, Namespace, ParseOpts, QualName};
use tendril::{StrTendril, TendrilSink};

use crate::node::{Dom, ElementData, NodeData, NodeId};

/// Owned element name handed back to the tree builder.
///
/// The arena lives behind a `RefCell`, so we cannot lend references
/// into it; this cheap atom pair is the `ElemName` currency instead.
#[derive(Debug, Clone)]
pub struct OwnedElemName {
    ns: Namespace,
    local: LocalName,
}

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.local
    }
}

/// TreeSink implementation building a [`Dom`].
#[derive(Debug)]
pub struct DomBuilder {
    dom: RefCell<Dom>,
}

impl Default for DomBuilder {
    fn default() -> Self {
        DomBuilder::new()
    }
}

impl DomBuilder {
    pub fn new() -> Self {
        DomBuilder {
            dom: RefCell::new(Dom::new()),
        }
    }
}

impl TreeSink for DomBuilder {
    type Handle = NodeId;
    type Output = Dom;
    type ElemName<'a> = OwnedElemName;

    fn finish(self) -> Dom {
        self.dom.into_inner()
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {
        self.dom.borrow_mut().note_parse_error();
    }

    fn get_document(&self) -> NodeId {
        self.dom.borrow().root()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedElemName {
        let dom = self.dom.borrow();
        let el = dom.element(*target).expect("elem_name on a non-element");
        OwnedElemName {
            ns: el.name.ns.clone(),
            local: el.name.local.clone(),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let mut dom = self.dom.borrow_mut();
        let id = dom.create_node(NodeData::Element(ElementData {
            name,
            attrs,
            template_contents: None,
            mathml_annotation_xml_integration_point: flags
                .mathml_annotation_xml_integration_point,
        }));
        if flags.template {
            let contents = dom.create_node(NodeData::Document);
            if let NodeData::Element(ref mut el) = dom.node_mut(id).data {
                el.template_contents = Some(contents);
            }
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.dom
            .borrow_mut()
            .create_node(NodeData::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.dom
            .borrow_mut()
            .create_node(NodeData::ProcessingInstruction {
                target: target.to_string(),
                data: data.to_string(),
            })
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let mut dom = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(node) => dom.append(*parent, node),
            NodeOrText::AppendText(text) => {
                // Merge with a trailing text sibling, per the TreeSink contract.
                let last = dom.node(*parent).last_child;
                if let Some(last) = last {
                    if let NodeData::Text(ref mut existing) = dom.node_mut(last).data {
                        existing.push_str(&text);
                        return;
                    }
                }
                let t = dom.create_node(NodeData::Text(text.to_string()));
                dom.append(*parent, t);
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.dom.borrow().node(*element).parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut dom = self.dom.borrow_mut();
        let d = dom.create_node(NodeData::Doctype {
            name: name.to_string(),
            public_id: public_id.to_string(),
            system_id: system_id.to_string(),
        });
        let root = dom.root();
        dom.append(root, d);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.dom
            .borrow()
            .element(*target)
            .and_then(|el| el.template_contents)
            .expect("get_template_contents on a non-template element")
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.borrow_mut().set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let mut dom = self.dom.borrow_mut();
        match new_node {
            NodeOrText::AppendNode(node) => dom.insert_before(*sibling, node),
            NodeOrText::AppendText(text) => {
                // Merge with the preceding text sibling when there is one.
                let prev = dom.node(*sibling).prev_sibling;
                if let Some(prev) = prev {
                    if let NodeData::Text(ref mut existing) = dom.node_mut(prev).data {
                        existing.push_str(&text);
                        return;
                    }
                }
                let t = dom.create_node(NodeData::Text(text.to_string()));
                dom.insert_before(*sibling, t);
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        self.dom.borrow_mut().add_attrs_if_missing(*target, attrs);
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.dom.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.dom.borrow_mut().reparent_children(*node, *new_parent);
    }
}

/// Parse an HTML document into a flat arena [`Dom`].
pub fn parse_html(html: &str) -> Dom {
    parse_document(DomBuilder::new(), ParseOpts::default()).one(html)
}
