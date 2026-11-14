//! # ghostwalk-dom
//!
//! Flat arena DOM for the ghostwalk engine: spec-compliant HTML5 parsing
//! (via html5ever) into contiguous, cache-friendly storage, plus the
//! read projections agents actually want (text, Markdown, tree dumps).
//!
//! See ARCHITECTURE.md for why the arena model exists and where it is
//! heading (structure-of-arrays layout and parallel style/layout on top of it).

pub mod extract;
pub mod node;
pub mod parse;
pub mod select;

pub use node::{Dom, ElementData, InvalidNodeId, Node, NodeData, NodeId};
pub use parse::{parse_html, DomBuilder};
