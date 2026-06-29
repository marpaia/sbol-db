//! Breadth-first neighborhood traversal over the triplestore.
//!
//! Mirrors the SQL backends: expand the frontier one hop at a time via pattern
//! scans (forward, backward, or both), dedup nodes and edges, respect the
//! depth and node caps, then enrich resource nodes with derived-view metadata.

use std::collections::{BTreeMap, BTreeSet};

use sbol_db_core::{
    Direction, DomainError, EdgeInfo, EdgeObject, NeighborhoodQuery, NeighborhoodResult, NodeInfo,
    ObjectTerm, SubjectTerm,
};
use sbol_db_storage::{PatternObject, PatternSubject};

use super::object::ObjectRepository;
use super::triple::TripleRepository;

const SCAN_LIMIT: i64 = 100_000;

pub fn walk(
    triples: &TripleRepository,
    objects: &ObjectRepository,
    query: &NeighborhoodQuery,
) -> Result<NeighborhoodResult, DomainError> {
    let allow: BTreeSet<String> = query
        .predicate_allowlist
        .iter()
        .map(|p| p.as_str().to_owned())
        .collect();
    let max_nodes = query.max_nodes.map(|n| n as usize);
    let root = query.root_iri.as_str().to_owned();

    let mut nodes: BTreeMap<String, NodeInfo> = BTreeMap::new();
    let mut edges: Vec<EdgeInfo> = Vec::new();
    let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut max_depth = 0u32;
    let mut truncated = false;

    nodes.insert(root.clone(), node_info(&root, 0, false));
    let mut frontier: Vec<(String, bool)> = vec![(root.clone(), false)];

    'levels: for depth in 0..query.depth {
        let next_depth = depth + 1;
        let mut next: Vec<(String, bool)> = Vec::new();

        for (node_id, node_blank) in &frontier {
            if matches!(query.direction, Direction::Forward | Direction::Both) {
                let subject = pattern_subject(node_id, *node_blank);
                let rows = triples.scan_pattern(Some(&subject), None, None, None, SCAN_LIMIT)?;
                for triple in rows {
                    let predicate = triple.predicate.as_str().to_owned();
                    if !allow.is_empty() && !allow.contains(&predicate) {
                        continue;
                    }
                    let (edge_object, child) = object_to_edge(&triple.object);
                    if matches!(edge_object, EdgeObject::Literal { .. }) && !query.include_literals
                    {
                        continue;
                    }
                    push_edge(
                        &mut edges,
                        &mut seen_edges,
                        node_id.clone(),
                        *node_blank,
                        predicate,
                        edge_object,
                        next_depth,
                    );
                    if let Some((child_id, child_blank)) = child {
                        if add_node(
                            &mut nodes,
                            &mut next,
                            &child_id,
                            child_blank,
                            next_depth,
                            max_nodes,
                            &mut truncated,
                        ) {
                            max_depth = max_depth.max(next_depth);
                        } else if truncated {
                            break 'levels;
                        }
                    }
                }
            }

            if matches!(query.direction, Direction::Backward | Direction::Both) {
                let object = pattern_object(node_id, *node_blank);
                let rows = triples.scan_pattern(None, None, Some(&object), None, SCAN_LIMIT)?;
                for triple in rows {
                    let predicate = triple.predicate.as_str().to_owned();
                    if !allow.is_empty() && !allow.contains(&predicate) {
                        continue;
                    }
                    let (subject_id, subject_blank) = subject_id(&triple.subject);
                    let edge_object = if *node_blank {
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
                        edge_object,
                        next_depth,
                    );
                    if add_node(
                        &mut nodes,
                        &mut next,
                        &subject_id,
                        subject_blank,
                        next_depth,
                        max_nodes,
                        &mut truncated,
                    ) {
                        max_depth = max_depth.max(next_depth);
                    } else if truncated {
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

    for (id, info) in nodes.iter_mut() {
        if info.is_blank {
            continue;
        }
        if let Some(obj) = objects.get_by_iri(id)? {
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

#[allow(clippy::too_many_arguments)]
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
