//! Graph neighborhood traversal types. The traversal itself happens in
//! `sbol-db-postgres` via a recursive CTE over `sbol_quads`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::iri::IriString;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Follow edges `subject == root → object`. The "what does this design
    /// contain" view.
    #[default]
    Forward,
    /// Follow edges `subject → object == root`. The "what references this
    /// thing" view.
    Backward,
    /// Both directions, merged.
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeighborhoodQuery {
    pub root_iri: IriString,
    pub depth: u32,
    pub direction: Direction,
    /// If non-empty, only edges whose predicate IRI is in the set are
    /// followed. Useful for "only traverse `sbol:hasFeature` /
    /// `sbol:hasSequence`" style walks.
    #[serde(default)]
    pub predicate_allowlist: Vec<IriString>,
    /// If `Some`, traversal stops after the visited node count reaches this
    /// limit; `truncated` is set on the result.
    pub max_nodes: Option<u32>,
    /// When false (the default) literal-position edges are skipped so the
    /// node frontier only contains IRIs/blank nodes.
    #[serde(default)]
    pub include_literals: bool,
}

impl NeighborhoodQuery {
    pub fn new(root: IriString) -> Self {
        Self {
            root_iri: root,
            depth: 1,
            direction: Direction::Forward,
            predicate_allowlist: Vec::new(),
            max_nodes: Some(2048),
            include_literals: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeighborhoodResult {
    pub root_iri: IriString,
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<EdgeInfo>,
    pub max_depth_reached: u32,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    /// IRI of the node, or a blank-node identifier prefixed with `_:`.
    pub id: String,
    pub depth: u32,
    pub is_blank: bool,
    pub sbol_class: Option<String>,
    pub display_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeInfo {
    pub subject: String,
    pub subject_is_blank: bool,
    pub predicate: String,
    pub object: EdgeObject,
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeObject {
    Iri {
        value: String,
    },
    BlankNode {
        value: String,
    },
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}

/// Convenience: build a `depth -> Vec<NodeInfo>` index. Useful for UI layout.
pub fn group_by_depth(nodes: &[NodeInfo]) -> HashMap<u32, Vec<&NodeInfo>> {
    let mut map: HashMap<u32, Vec<&NodeInfo>> = HashMap::new();
    for node in nodes {
        map.entry(node.depth).or_default().push(node);
    }
    map
}
