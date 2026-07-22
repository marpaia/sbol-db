//! The top-level link graph and PageRank power iteration.
//!
//! SBOLExplorer ranks objects by running PageRank over a link graph whose nodes
//! are the top-level objects and whose edges connect two top-level objects that
//! reference each other directly or through one intermediate (a blank
//! Component, in SBOL2). [`top_level_link_graph`] reproduces its `link_query`
//! and [`pagerank`] reproduces its power iteration exactly, so the native
//! ranking matches the tool it replaces.

use std::collections::{HashMap, HashSet};

use sbol_db_core::{ObjectTerm, SubjectTerm, Triple};

/// The SynBioHub `topLevel` marker: `<self> sbh:topLevel <self>` flags a
/// top-level object.
const TOPLEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";

/// PageRank damping factor: the probability a random surfer follows a link
/// rather than teleporting. SBOLExplorer's `s`.
const DAMPING: f64 = 0.85;

/// L1-norm convergence threshold between successive rank vectors.
const TOLERANCE: f64 = 1e-4;

/// A safety cap on power-iteration rounds; convergence at the tolerance above is
/// reached well within this on real corpora, and the cap keeps a pathological
/// input from looping forever.
const MAX_ITERATIONS: usize = 1000;

/// A node id for reference adjacency. Blank nodes are kept (prefixed `_:`) as
/// traversable intermediates but never match the IRI top-level set, so an edge
/// spans a blank node without ever terminating on one.
fn subj_node(subject: &SubjectTerm) -> String {
    match subject {
        SubjectTerm::Iri(iri) => iri.as_str().to_owned(),
        SubjectTerm::BlankNode(b) => format!("_:{b}"),
    }
}

fn obj_node(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
        ObjectTerm::BlankNode(b) => Some(format!("_:{b}")),
        ObjectTerm::Literal { .. } => None,
    }
}

/// The blank-node-spanning out-edge map: each subject node mapped to the node
/// objects it directly references, in triple order. Both the SynBioHub
/// accelerator's root-member anti-join and [`top_level_link_graph`] traverse
/// this same adjacency, so the reference-graph construction has one
/// implementation.
pub fn reference_adjacency(triples: &[Triple]) -> HashMap<String, Vec<String>> {
    let mut out_edges: HashMap<String, Vec<String>> = HashMap::new();
    for t in triples {
        if let Some(object) = obj_node(&t.object) {
            out_edges
                .entry(subj_node(&t.subject))
                .or_default()
                .push(object);
        }
    }
    out_edges
}

/// SBOLExplorer's `link_query`: the set of edges `(parent, child)` where both
/// `parent` and `child` are top-level objects and `child` is reachable from
/// `parent` in one hop (`parent ?p child`) or two (`parent ?a ?tmp . ?tmp ?b
/// child`, the intermediate typically a blank Component). Edges are distinct
/// and both endpoints are always top-level IRIs; the marker triple `?x
/// sbh:topLevel ?x` yields a self-edge exactly as the query does.
pub fn top_level_link_graph(triples: &[Triple]) -> Vec<(String, String)> {
    let top_levels = top_level_iris(triples);
    let out_edges = reference_adjacency(triples);

    let mut edges: HashSet<(String, String)> = HashSet::new();
    for parent in &top_levels {
        let Some(direct) = out_edges.get(parent) else {
            continue;
        };
        for mid in direct {
            if top_levels.contains(mid) {
                edges.insert((parent.clone(), mid.clone()));
            }
            if let Some(grandchildren) = out_edges.get(mid) {
                for child in grandchildren {
                    if top_levels.contains(child) {
                        edges.insert((parent.clone(), child.clone()));
                    }
                }
            }
        }
    }
    edges.into_iter().collect()
}

/// Every IRI flagged top-level by a `<self> sbh:topLevel <self>` triple. These
/// are the nodes PageRank ranks (SBOLExplorer's `uri_query`).
pub fn top_level_iris(triples: &[Triple]) -> HashSet<String> {
    let mut set = HashSet::new();
    for t in triples {
        if t.predicate.as_str() != TOPLEVEL {
            continue;
        }
        if let (SubjectTerm::Iri(s), ObjectTerm::Iri(o)) = (&t.subject, &t.object) {
            if s.as_str() == o.as_str() {
                set.insert(s.as_str().to_owned());
            }
        }
    }
    set
}

/// PageRank over the top-level link graph by power iteration, reproducing
/// SBOLExplorer's `pagerank`: damping `s = 0.85`, teleport `(1 - s) / n` to
/// every node, dangling mass (nodes with no out-links) redistributed uniformly,
/// and the L1 norm between successive vectors as the stop condition. `uris` is
/// the full node set; `edges` are the `(parent, child)` links from
/// [`top_level_link_graph`]. Edge endpoints outside `uris` are ignored. An empty
/// node set returns an empty map; otherwise every node in `uris` is present.
///
/// A URI absent from the returned map is unranked; the search combine step reads
/// its rank as `1.0`, SBOLExplorer's convention for unknown or newly added
/// parts.
pub fn pagerank(edges: &[(String, String)], uris: &[String]) -> HashMap<String, f64> {
    let index: HashMap<&str, usize> = uris
        .iter()
        .enumerate()
        .map(|(i, u)| (u.as_str(), i))
        .collect();
    let n = uris.len();
    if n == 0 {
        return HashMap::new();
    }

    // Out-degree and inbound adjacency by node index, with set semantics on
    // edges so a repeated link does not inflate a node's out-degree.
    let mut children: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (parent, child) in edges {
        if let (Some(&p), Some(&c)) = (index.get(parent.as_str()), index.get(child.as_str())) {
            children[p].insert(c);
        }
    }
    let out_degree: Vec<usize> = children.iter().map(HashSet::len).collect();
    let mut in_links: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (p, kids) in children.iter().enumerate() {
        for &c in kids {
            in_links[c].push(p);
        }
    }
    let dangling: Vec<usize> = (0..n).filter(|&j| out_degree[j] == 0).collect();

    let nf = n as f64;
    let teleport = (1.0 - DAMPING) / nf;
    let mut p = vec![1.0 / nf; n];

    for _ in 0..MAX_ITERATIONS {
        let dangling_contrib: f64 = dangling.iter().map(|&j| p[j]).sum::<f64>() / nf;
        let mut v = vec![0.0; n];
        for (j, vj) in v.iter_mut().enumerate() {
            let in_contrib: f64 = in_links[j]
                .iter()
                .map(|&k| p[k] / out_degree[k] as f64)
                .sum();
            *vj = DAMPING * (in_contrib + dangling_contrib) + teleport;
        }
        let sum: f64 = v.iter().sum();
        if sum > 0.0 {
            for vj in v.iter_mut() {
                *vj /= sum;
            }
        }
        let delta: f64 = p.iter().zip(&v).map(|(a, b)| (a - b).abs()).sum();
        p = v;
        if delta <= TOLERANCE {
            break;
        }
    }

    uris.iter().cloned().zip(p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_core::IriString;

    fn top_level(iri: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::Iri(IriString::unchecked(iri)),
            predicate: IriString::unchecked(TOPLEVEL),
            object: ObjectTerm::Iri(IriString::unchecked(iri)),
        }
    }

    fn iri_edge(subject: &str, predicate: &str, object: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::Iri(IriString::unchecked(subject)),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::Iri(IriString::unchecked(object)),
        }
    }

    fn blank_from(subject: &str, predicate: &str, blank: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::Iri(IriString::unchecked(subject)),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::BlankNode(blank.to_owned()),
        }
    }

    fn blank_to(blank: &str, predicate: &str, object: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::BlankNode(blank.to_owned()),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::Iri(IriString::unchecked(object)),
        }
    }

    const CD1: &str = "http://example.org/cd1";
    const CD2: &str = "http://example.org/cd2";
    const NON_TL: &str = "http://example.org/plain";
    const P: &str = "http://sbols.org/v2#component";
    const DEF: &str = "http://sbols.org/v2#definition";

    #[test]
    fn two_hop_blank_node_link_between_top_levels() {
        // CD1 -> _:c (a blank Component) -> CD2, both CDs top-level, plus a
        // reference from CD1 to a non-top-level object that must be excluded.
        let triples = vec![
            top_level(CD1),
            top_level(CD2),
            blank_from(CD1, P, "c"),
            blank_to("c", DEF, CD2),
            iri_edge(CD1, P, NON_TL),
        ];
        let edges: HashSet<(String, String)> = top_level_link_graph(&triples).into_iter().collect();

        assert!(
            edges.contains(&(CD1.to_owned(), CD2.to_owned())),
            "two-hop blank-node-spanning edge CD1->CD2 is present"
        );
        // The non-top-level target never appears as an endpoint.
        assert!(edges.iter().all(|(a, b)| a != NON_TL && b != NON_TL));
        // The marker triple yields the self-edges the query produces.
        assert!(edges.contains(&(CD1.to_owned(), CD1.to_owned())));
        assert!(edges.contains(&(CD2.to_owned(), CD2.to_owned())));
    }

    #[test]
    fn hub_outranks_leaf() {
        // Three leaves all reference one hub; the hub should rank highest.
        let hub = "http://example.org/hub".to_owned();
        let a = "http://example.org/a".to_owned();
        let b = "http://example.org/b".to_owned();
        let c = "http://example.org/c".to_owned();
        let uris = vec![hub.clone(), a.clone(), b.clone(), c.clone()];
        let edges = vec![
            (a.clone(), hub.clone()),
            (b.clone(), hub.clone()),
            (c.clone(), hub.clone()),
        ];
        let ranks = pagerank(&edges, &uris);
        let hub_rank = ranks[&hub];
        assert!(hub_rank > ranks[&a]);
        assert!(hub_rank > ranks[&b]);
        assert!(hub_rank > ranks[&c]);
        // Ranks form a probability distribution.
        let total: f64 = ranks.values().sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn three_node_cycle_is_uniform() {
        let x = "http://example.org/x".to_owned();
        let y = "http://example.org/y".to_owned();
        let z = "http://example.org/z".to_owned();
        let uris = vec![x.clone(), y.clone(), z.clone()];
        let edges = vec![
            (x.clone(), y.clone()),
            (y.clone(), z.clone()),
            (z.clone(), x.clone()),
        ];
        let ranks = pagerank(&edges, &uris);
        for u in &uris {
            assert!((ranks[u] - 1.0 / 3.0).abs() < 1e-3);
        }
    }

    #[test]
    fn empty_graph_returns_empty() {
        assert!(pagerank(&[], &[]).is_empty());
    }
}
