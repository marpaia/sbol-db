//! Graph-neighborhood traversal over SQLite, as a breadth-first walk over the
//! triplestore (Postgres uses a recursive CTE; the observable result matches).
//!
//! From the root, each level expands resource-position edges in the requested
//! direction(s): forward follows `root -> object`, backward follows
//! `subject -> root`. Literal objects never widen the frontier and are included
//! as edges only when `include_literals` is set. Visited resource nodes are
//! then enriched with their derived-view metadata.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use sbol_db_core::{
    Direction, DomainError, EdgeInfo, EdgeObject, NeighborhoodQuery, NeighborhoodResult, NodeInfo,
    ObjectTerm, SubjectTerm,
};
use sbol_db_storage::{PatternObject, PatternSubject};

use crate::repo::{SbolObjectRepository, TripleRepository};

/// Per-node scan cap. Far above any real fan-out for one subject/object.
const SCAN_LIMIT: i64 = 100_000;

pub async fn walk(
    triples: &TripleRepository,
    objects: &SbolObjectRepository,
    query: &NeighborhoodQuery,
) -> Result<NeighborhoodResult, DomainError> {
    let allow: HashSet<String> = query
        .predicate_allowlist
        .iter()
        .map(|i| i.as_str().to_owned())
        .collect();
    let max_nodes = query.max_nodes.map(|m| m as usize);
    let root = query.root_iri.as_str().to_owned();

    let mut nodes: BTreeMap<String, NodeInfo> = BTreeMap::new();
    let mut edges: Vec<EdgeInfo> = Vec::new();
    let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut max_depth = 0u32;
    let mut truncated = false;

    nodes.insert(root.clone(), node_info(&root, 0, false));
    let mut frontier: Vec<(String, bool)> = vec![(root.clone(), false)];

    'levels: for depth in 0..query.depth {
        let mut next: Vec<(String, bool)> = Vec::new();
        let child_depth = depth + 1;

        for (node_id, node_blank) in &frontier {
            if matches!(query.direction, Direction::Forward | Direction::Both) {
                let subject = pattern_subject(node_id, *node_blank);
                let found = triples
                    .scan_pattern(Some(&subject), None, None, None, SCAN_LIMIT)
                    .await?;
                for triple in found {
                    let predicate = triple.predicate.as_str().to_owned();
                    if !allow.is_empty() && !allow.contains(&predicate) {
                        continue;
                    }
                    let (object, child) = object_to_edge(&triple.object);
                    if matches!(object, EdgeObject::Literal { .. }) && !query.include_literals {
                        continue;
                    }
                    push_edge(
                        &mut edges,
                        &mut seen_edges,
                        node_id.clone(),
                        *node_blank,
                        predicate,
                        object,
                        child_depth,
                    );
                    if let Some((child_id, child_blank)) = child {
                        if add_node(
                            &mut nodes,
                            &mut next,
                            &child_id,
                            child_blank,
                            child_depth,
                            max_nodes,
                            &mut truncated,
                        ) {
                            max_depth = max_depth.max(child_depth);
                        }
                        if truncated {
                            break 'levels;
                        }
                    }
                }
            }

            if matches!(query.direction, Direction::Backward | Direction::Both) {
                let object = pattern_object(node_id, *node_blank);
                let found = triples
                    .scan_pattern(None, None, Some(&object), None, SCAN_LIMIT)
                    .await?;
                for triple in found {
                    let predicate = triple.predicate.as_str().to_owned();
                    if !allow.is_empty() && !allow.contains(&predicate) {
                        continue;
                    }
                    let (subject_id, subject_blank) = subject_id(&triple.subject);
                    let object_edge = if *node_blank {
                        EdgeObject::BlankNode {
                            value: node_id.clone(),
                        }
                    } else {
                        EdgeObject::Iri {
                            value: node_id.clone(),
                        }
                    };
                    push_edge(
                        &mut edges,
                        &mut seen_edges,
                        subject_id.clone(),
                        subject_blank,
                        predicate,
                        object_edge,
                        child_depth,
                    );
                    if add_node(
                        &mut nodes,
                        &mut next,
                        &subject_id,
                        subject_blank,
                        child_depth,
                        max_nodes,
                        &mut truncated,
                    ) {
                        max_depth = max_depth.max(child_depth);
                    }
                    if truncated {
                        break 'levels;
                    }
                }
            }
        }

        if next.is_empty() {
            break;
        }
        frontier = next;
    }

    // Enrich resource nodes with derived-view metadata.
    for (id, info) in nodes.iter_mut() {
        if info.is_blank {
            continue;
        }
        if let Some(obj) = objects.get_by_iri(id).await? {
            info.sbol_class = Some(obj.sbol_class);
            info.display_id = obj.display_id;
            info.name = obj.name;
        }
    }

    Ok(NeighborhoodResult {
        root_iri: query.root_iri.clone(),
        nodes: nodes.into_values().collect(),
        edges,
        max_depth_reached: max_depth,
        truncated,
    })
}

fn node_info(id: &str, depth: u32, is_blank: bool) -> NodeInfo {
    NodeInfo {
        id: id.to_owned(),
        depth,
        is_blank,
        sbol_class: None,
        display_id: None,
        name: None,
    }
}

fn pattern_subject(id: &str, is_blank: bool) -> PatternSubject {
    if is_blank {
        PatternSubject::Blank(id.to_owned())
    } else {
        PatternSubject::Iri(id.to_owned())
    }
}

fn pattern_object(id: &str, is_blank: bool) -> PatternObject {
    if is_blank {
        PatternObject::Blank(id.to_owned())
    } else {
        PatternObject::Iri(id.to_owned())
    }
}

fn subject_id(subject: &SubjectTerm) -> (String, bool) {
    match subject {
        SubjectTerm::Iri(iri) => (iri.as_str().to_owned(), false),
        SubjectTerm::BlankNode(node) => (node.clone(), true),
    }
}

/// Map an object term to its edge form plus, for resource objects, the
/// `(id, is_blank)` that should join the frontier.
fn object_to_edge(object: &ObjectTerm) -> (EdgeObject, Option<(String, bool)>) {
    match object {
        ObjectTerm::Iri(iri) => {
            let value = iri.as_str().to_owned();
            (
                EdgeObject::Iri {
                    value: value.clone(),
                },
                Some((value, false)),
            )
        }
        ObjectTerm::BlankNode(node) => (
            EdgeObject::BlankNode {
                value: node.clone(),
            },
            Some((node.clone(), true)),
        ),
        ObjectTerm::Literal {
            value,
            datatype,
            language,
        } => (
            EdgeObject::Literal {
                value: value.clone(),
                datatype: datatype.as_str().to_owned(),
                language: language.clone(),
            },
            None,
        ),
    }
}

fn edge_object_key(object: &EdgeObject) -> String {
    match object {
        EdgeObject::Iri { value } | EdgeObject::BlankNode { value } => value.clone(),
        EdgeObject::Literal { value, .. } => format!("\u{1}lit\u{1}{value}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_edge(
    edges: &mut Vec<EdgeInfo>,
    seen: &mut BTreeSet<(String, String, String)>,
    subject: String,
    subject_is_blank: bool,
    predicate: String,
    object: EdgeObject,
    depth: u32,
) {
    let key = (subject.clone(), predicate.clone(), edge_object_key(&object));
    if seen.insert(key) {
        edges.push(EdgeInfo {
            subject,
            subject_is_blank,
            predicate,
            object,
            depth,
        });
    }
}

/// Insert a newly reached node and enqueue it. Returns whether it was new.
/// Sets `truncated` and skips insertion once `max_nodes` is reached.
fn add_node(
    nodes: &mut BTreeMap<String, NodeInfo>,
    next: &mut Vec<(String, bool)>,
    id: &str,
    is_blank: bool,
    depth: u32,
    max_nodes: Option<usize>,
    truncated: &mut bool,
) -> bool {
    if nodes.contains_key(id) {
        return false;
    }
    if let Some(cap) = max_nodes {
        if nodes.len() >= cap {
            *truncated = true;
            return false;
        }
    }
    nodes.insert(id.to_owned(), node_info(id, depth, is_blank));
    next.push((id.to_owned(), is_blank));
    true
}
