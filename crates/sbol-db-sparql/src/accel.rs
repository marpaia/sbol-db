//! Recognizes SynBioHub's fixed query templates and maps them to an
//! [`AcceleratedQuery`] the backend answers from purpose-built indexes.
//!
//! Recognition is structural and conservative: it gathers the query's triple
//! patterns, projection, pagination, and target graph, then matches the known
//! SynBioHub shapes. Anything that does not match exactly returns `None` and the
//! engine evaluates it normally, so correctness never depends on a match.

use std::collections::HashMap;

use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern, Variable};
use spargebra::Query;

use sbol_db_storage::{AcceleratedQuery, FacetKind, Field, Scope};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const TOPLEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
const DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const VERSION: &str = "http://sbols.org/v2#version";
const SBOL_TYPE: &str = "http://sbols.org/v2#type";
const ROLE: &str = "http://sbols.org/v2#role";
const TITLE: &str = "http://purl.org/dc/terms/title";
const DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const CREATOR: &str = "http://purl.org/dc/elements/1.1/creator";
const MEMBER: &str = "http://sbols.org/v2#member";

/// Recognize a query, returning the accelerator plan if it matches a known
/// SynBioHub template. `default_graph` is the protocol `default-graph-uri`,
/// used when the query has no `FROM`.
pub fn recognize(query: &Query, default_graph: Option<&str>) -> Option<AcceleratedQuery> {
    let (pattern, dataset) = match query {
        Query::Select {
            pattern, dataset, ..
        } => (pattern, dataset),
        _ => return None,
    };
    let graph = single_graph(dataset, default_graph)?;

    // Unwrap solution modifiers, noting projection, pagination, and whether an
    // aggregate (Group) is present.
    let mut p = pattern;
    let mut projection: Option<&[Variable]> = None;
    let mut offset = 0usize;
    let mut limit: Option<usize> = None;
    let mut is_aggregate = false;
    loop {
        match p {
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => {
                offset = *start;
                limit = *length;
                p = inner;
            }
            GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => p = inner,
            GraphPattern::OrderBy { inner, .. } => p = inner,
            // Aggregates bind their result through an `Extend` over the `Group`.
            GraphPattern::Extend { inner, .. } => p = inner,
            GraphPattern::Project { inner, variables } => {
                if projection.is_none() {
                    projection = Some(variables);
                }
                p = inner;
            }
            GraphPattern::Group { inner, .. } => {
                is_aggregate = true;
                p = inner;
            }
            other => {
                p = other;
                break;
            }
        }
    }
    let projection = projection?;

    let mut patterns: Vec<&TriplePattern> = Vec::new();
    collect_patterns(p, &mut patterns);
    if patterns.is_empty() {
        return None;
    }
    // A `FILTER NOT EXISTS` on the collection-member query selects "root" members
    // (those not referenced by another member); the anti-join is precomputed.
    let root_only = has_not_exists(p);

    if is_aggregate {
        let var = projection.first()?.as_str().to_owned();
        let (scope, subject_var) = detect_scope(&patterns, root_only)?;
        let subject_prefix = strstarts_prefix(p, &subject_var);
        return Some(AcceleratedQuery::Count {
            graph,
            scope,
            var,
            subject_prefix,
        });
    }

    let vars: Vec<String> = projection.iter().map(|v| v.as_str().to_owned()).collect();

    if vars.len() == 1 {
        if let Some(kind) = facet_kind(&patterns, &vars[0]) {
            return Some(AcceleratedQuery::Facet {
                graph,
                kind,
                var: vars.into_iter().next()?,
            });
        }
    }

    let (scope, subject_var) = detect_scope(&patterns, root_only)?;
    let fields = field_map(&patterns);
    let mut proj = Vec::with_capacity(vars.len());
    for v in &vars {
        let field = if *v == subject_var {
            Field::Subject
        } else {
            *fields.get(v)?
        };
        proj.push((v.clone(), field));
    }
    let subject_prefix = strstarts_prefix(p, &subject_var);
    Some(AcceleratedQuery::ObjectList {
        graph,
        scope,
        projection: proj,
        offset,
        limit,
        subject_prefix,
    })
}

/// The single target graph: the lone `FROM` graph, or the protocol default. More
/// than one `FROM` graph is not accelerated (returns `None`).
fn single_graph(
    dataset: &Option<spargebra::algebra::QueryDataset>,
    default_graph: Option<&str>,
) -> Option<String> {
    match dataset {
        Some(ds) => match ds.default.as_slice() {
            [g] => Some(g.as_str().to_owned()),
            _ => None,
        },
        None => default_graph.map(str::to_owned),
    }
}

fn collect_patterns<'a>(pattern: &'a GraphPattern, out: &mut Vec<&'a TriplePattern>) {
    use GraphPattern::*;
    match pattern {
        Bgp { patterns } => out.extend(patterns.iter()),
        Join { left, right }
        | LeftJoin { left, right, .. }
        | Union { left, right }
        | Minus { left, right } => {
            collect_patterns(left, out);
            collect_patterns(right, out);
        }
        Filter { inner, .. }
        | Graph { inner, .. }
        | Extend { inner, .. }
        | OrderBy { inner, .. }
        | Project { inner, .. }
        | Distinct { inner }
        | Reduced { inner }
        | Slice { inner, .. }
        | Group { inner, .. }
        | Service { inner, .. } => collect_patterns(inner, out),
        // Path/Values/leaves carry no plain triple patterns this pass uses.
        _ => {}
    }
}

/// The scope and subject variable for a listing/aggregate: collection members
/// (a `<collection> sbol2:member ?uri` pattern), else top-level (`?s topLevel ?s`),
/// else by a constant `rdf:type`.
fn detect_scope(patterns: &[&TriplePattern], root_only: bool) -> Option<(Scope, String)> {
    for t in patterns {
        if pred(t) == Some(MEMBER) {
            if let (TermPattern::NamedNode(coll), TermPattern::Variable(uri)) =
                (&t.subject, &t.object)
            {
                let scope = Scope::Collection {
                    collection: coll.as_str().to_owned(),
                    root_only,
                };
                return Some((scope, uri.as_str().to_owned()));
            }
        }
    }
    for t in patterns {
        if is_toplevel(t) {
            if let TermPattern::Variable(s) = &t.subject {
                return Some((Scope::TopLevel, s.as_str().to_owned()));
            }
        }
    }
    for t in patterns {
        if pred(t) == Some(RDF_TYPE) {
            if let (TermPattern::Variable(s), TermPattern::NamedNode(n)) = (&t.subject, &t.object) {
                return Some((Scope::ByType(n.as_str().to_owned()), s.as_str().to_owned()));
            }
        }
    }
    None
}

/// Map each metadata predicate's object variable to the field it projects.
fn field_map(patterns: &[&TriplePattern]) -> HashMap<String, Field> {
    let mut fields = HashMap::new();
    for t in patterns {
        let var = match &t.object {
            TermPattern::Variable(v) => v.as_str().to_owned(),
            _ => continue,
        };
        let field = match pred(t) {
            Some(RDF_TYPE) => Field::Type,
            Some(DISPLAY_ID) => Field::DisplayId,
            Some(VERSION) => Field::Version,
            Some(TITLE) => Field::Name,
            Some(DESCRIPTION) => Field::Description,
            Some(SBOL_TYPE) => Field::SbolType,
            Some(ROLE) => Field::Role,
            _ => continue,
        };
        fields.insert(var, field);
    }
    fields
}

/// Whether the pattern contains a `FILTER NOT EXISTS`.
fn has_not_exists(pattern: &GraphPattern) -> bool {
    use GraphPattern::*;
    match pattern {
        Filter { expr, inner } => expr_has_not_exists(expr) || has_not_exists(inner),
        Join { left, right }
        | LeftJoin { left, right, .. }
        | Union { left, right }
        | Minus { left, right } => has_not_exists(left) || has_not_exists(right),
        Graph { inner, .. }
        | Extend { inner, .. }
        | OrderBy { inner, .. }
        | Project { inner, .. }
        | Distinct { inner }
        | Reduced { inner }
        | Slice { inner, .. }
        | Group { inner, .. }
        | Service { inner, .. } => has_not_exists(inner),
        _ => false,
    }
}

fn expr_has_not_exists(expr: &spargebra::algebra::Expression) -> bool {
    use spargebra::algebra::Expression::*;
    match expr {
        Not(inner) => matches!(inner.as_ref(), Exists(_)) || expr_has_not_exists(inner),
        And(a, b) | Or(a, b) => expr_has_not_exists(a) || expr_has_not_exists(b),
        _ => false,
    }
}

/// The constant prefix of a `FILTER(STRSTARTS(str(?subject), "prefix"))` on the
/// subject variable, if the pattern has one (the member-namespace filter).
fn strstarts_prefix(pattern: &GraphPattern, subject_var: &str) -> Option<String> {
    use GraphPattern::*;
    match pattern {
        Filter { expr, inner } => {
            expr_strstarts(expr, subject_var).or_else(|| strstarts_prefix(inner, subject_var))
        }
        Join { left, right }
        | LeftJoin { left, right, .. }
        | Union { left, right }
        | Minus { left, right } => {
            strstarts_prefix(left, subject_var).or_else(|| strstarts_prefix(right, subject_var))
        }
        Graph { inner, .. }
        | Extend { inner, .. }
        | OrderBy { inner, .. }
        | Project { inner, .. }
        | Distinct { inner }
        | Reduced { inner }
        | Slice { inner, .. }
        | Group { inner, .. }
        | Service { inner, .. } => strstarts_prefix(inner, subject_var),
        _ => None,
    }
}

fn expr_strstarts(expr: &spargebra::algebra::Expression, subject_var: &str) -> Option<String> {
    use spargebra::algebra::{Expression::*, Function};
    match expr {
        And(a, b) | Or(a, b) => {
            expr_strstarts(a, subject_var).or_else(|| expr_strstarts(b, subject_var))
        }
        FunctionCall(Function::StrStarts, args) if args.len() == 2 => {
            let on_subject = match &args[0] {
                FunctionCall(Function::Str, inner) if inner.len() == 1 => {
                    matches!(&inner[0], Variable(v) if v.as_str() == subject_var)
                }
                Variable(v) => v.as_str() == subject_var,
                _ => false,
            };
            if on_subject {
                if let Literal(lit) = &args[1] {
                    return Some(lit.value().to_owned());
                }
            }
            None
        }
        _ => None,
    }
}

fn facet_kind(patterns: &[&TriplePattern], var: &str) -> Option<FacetKind> {
    let object_is_var =
        |t: &TriplePattern| matches!(&t.object, TermPattern::Variable(v) if v.as_str() == var);
    let has_toplevel = patterns.iter().any(|t| is_toplevel(t));
    for t in patterns {
        if !object_is_var(t) {
            continue;
        }
        match pred(t) {
            Some(RDF_TYPE) if has_toplevel => return Some(FacetKind::Types),
            Some(ROLE) if has_toplevel => return Some(FacetKind::Roles),
            Some(CREATOR) => return Some(FacetKind::Creators),
            _ => {}
        }
    }
    None
}

fn is_toplevel(t: &TriplePattern) -> bool {
    pred(t) == Some(TOPLEVEL)
        && matches!((&t.subject, &t.object),
            (TermPattern::Variable(s), TermPattern::Variable(o)) if s.as_str() == o.as_str())
}

fn pred(t: &TriplePattern) -> Option<&str> {
    match &t.predicate {
        NamedNodePattern::NamedNode(n) => Some(n.as_str()),
        NamedNodePattern::Variable(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "http://synbiohub.org/public";
    const PFX: &str = "PREFIX sbol2: <http://sbols.org/v2#>\nPREFIX dcterms: <http://purl.org/dc/terms/>\nPREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>\nPREFIX dc: <http://purl.org/dc/elements/1.1/>\n";

    fn rec(q: &str) -> Option<AcceleratedQuery> {
        let query = spargebra::SparqlParser::new()
            .parse_query(&format!("{PFX}{q}"))
            .expect("parse");
        recognize(&query, Some(G))
    }

    #[test]
    fn recognizes_get_collections() {
        let q = format!("SELECT DISTINCT ?subject ?displayId ?name FROM <{G}> WHERE {{ ?subject a sbol2:Collection . OPTIONAL{{?subject sbol2:displayId ?displayId}} OPTIONAL{{?subject dcterms:title ?name}} }}");
        assert!(
            matches!(rec(&q), Some(AcceleratedQuery::ObjectList { scope: Scope::ByType(t), .. }) if t == "http://sbols.org/v2#Collection")
        );
    }

    #[test]
    fn recognizes_count_by_type() {
        let q = format!("SELECT (COUNT(DISTINCT ?cd) AS ?count) FROM <{G}> WHERE {{ ?cd a sbol2:ComponentDefinition }}");
        assert!(matches!(
            rec(&q),
            Some(AcceleratedQuery::Count {
                scope: Scope::ByType(_),
                ..
            })
        ));
    }

    #[test]
    fn recognizes_search() {
        let q = format!("SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role FROM <{G}> WHERE {{ ?subject a ?type . ?subject sbh:topLevel ?subject . OPTIONAL{{?subject sbol2:displayId ?displayId}} OPTIONAL{{?subject sbol2:version ?version}} OPTIONAL{{?subject dcterms:title ?name}} OPTIONAL{{?subject dcterms:description ?description}} OPTIONAL{{?subject sbol2:type ?sbolType}} OPTIONAL{{?subject sbol2:role ?role}} }} LIMIT 50 OFFSET 0");
        assert!(matches!(
            rec(&q),
            Some(AcceleratedQuery::ObjectList {
                scope: Scope::TopLevel,
                limit: Some(50),
                ..
            })
        ));
    }

    #[test]
    fn recognizes_search_count_nested_sum() {
        let q = format!("SELECT (sum(?tc) AS ?count) FROM <{G}> WHERE {{ {{ SELECT (count(distinct ?subject) AS ?tc) WHERE {{ ?subject a ?type . ?subject sbh:topLevel ?subject . OPTIONAL{{?subject dcterms:title ?n}} }} }} }}");
        assert!(
            matches!(
                rec(&q),
                Some(AcceleratedQuery::Count {
                    scope: Scope::TopLevel,
                    ..
                })
            ),
            "got {:?}",
            rec(&q)
        );
    }

    #[test]
    fn recognizes_facets() {
        let types = format!("SELECT DISTINCT ?object FROM <{G}> WHERE {{ ?subject a ?object . ?subject sbh:topLevel ?subject }}");
        assert!(matches!(
            rec(&types),
            Some(AcceleratedQuery::Facet {
                kind: FacetKind::Types,
                ..
            })
        ));
        let roles = format!("SELECT DISTINCT ?object FROM <{G}> WHERE {{ ?tl sbol2:role ?object . ?tl sbh:topLevel ?tl }}");
        assert!(matches!(
            rec(&roles),
            Some(AcceleratedQuery::Facet {
                kind: FacetKind::Roles,
                ..
            })
        ));
        let creators =
            format!("SELECT DISTINCT ?object FROM <{G}> WHERE {{ ?tl dc:creator ?object }}");
        assert!(matches!(
            rec(&creators),
            Some(AcceleratedQuery::Facet {
                kind: FacetKind::Creators,
                ..
            })
        ));
    }

    const COLL: &str = "http://localhost:7777/user/testuser/Tester_1/Tester_1_collection/1";

    #[test]
    fn recognizes_collection_members_root() {
        let q = format!("SELECT DISTINCT ?uri ?displayId ?name ?description ?type ?sbolType ?role FROM <{G}> WHERE {{ <{COLL}> a sbol2:Collection . <{COLL}> sbol2:member ?uri . OPTIONAL{{?uri a ?type}} OPTIONAL{{?uri sbol2:displayId ?displayId}} OPTIONAL{{?uri dcterms:title ?name}} OPTIONAL{{?uri dcterms:description ?description}} OPTIONAL{{?uri sbol2:type ?sbolType}} OPTIONAL{{?uri sbol2:role ?role}} FILTER(STRSTARTS(str(?uri), 'http://localhost:7777/')) FILTER NOT EXISTS {{ <{COLL}> sbol2:member ?om . {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }} FILTER(?om != ?uri) }} }} LIMIT 50 OFFSET 0");
        assert!(
            matches!(
                rec(&q),
                Some(AcceleratedQuery::ObjectList {
                    scope: Scope::Collection {
                        root_only: true,
                        ..
                    },
                    ..
                })
            ),
            "got {:?}",
            rec(&q)
        );
    }

    #[test]
    fn recognizes_collection_members_all() {
        let q = format!("SELECT DISTINCT ?uri ?displayId ?name ?description ?type ?sbolType ?role FROM <{G}> WHERE {{ <{COLL}> a sbol2:Collection . <{COLL}> sbol2:member ?uri . OPTIONAL{{?uri a ?type}} OPTIONAL{{?uri sbol2:displayId ?displayId}} OPTIONAL{{?uri dcterms:title ?name}} OPTIONAL{{?uri dcterms:description ?description}} OPTIONAL{{?uri sbol2:type ?sbolType}} OPTIONAL{{?uri sbol2:role ?role}} }} LIMIT 50 OFFSET 0");
        assert!(
            matches!(
                rec(&q),
                Some(AcceleratedQuery::ObjectList {
                    scope: Scope::Collection {
                        root_only: false,
                        ..
                    },
                    ..
                })
            ),
            "got {:?}",
            rec(&q)
        );
    }

    #[test]
    fn recognizes_count_members() {
        let q = format!("SELECT (COUNT(DISTINCT ?uri) AS ?count) FROM <{G}> WHERE {{ <{COLL}> sbol2:member ?uri . FILTER NOT EXISTS {{ <{COLL}> sbol2:member ?om . {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }} FILTER(?om != ?uri) }} }}");
        assert!(
            matches!(
                rec(&q),
                Some(AcceleratedQuery::Count {
                    scope: Scope::Collection {
                        root_only: true,
                        ..
                    },
                    ..
                })
            ),
            "got {:?}",
            rec(&q)
        );
    }

    #[test]
    fn recognizes_collection_members_root_full() {
        // The full SynBioHub shape: OPTIONALs with inner FILTERs, a STRSTARTS, and
        // the NOT EXISTS, no LIMIT.
        let q = format!("SELECT DISTINCT ?uri ?displayId ?name ?description ?type ?sbolType ?role FROM <{G}> WHERE {{ <{COLL}> a sbol2:Collection . <{COLL}> sbol2:member ?uri . OPTIONAL{{?uri a ?type}} OPTIONAL{{?uri sbol2:displayId ?displayId}} OPTIONAL{{?uri dcterms:title ?name}} OPTIONAL{{?uri dcterms:description ?description}} OPTIONAL{{?uri sbol2:type ?sbolType . FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl'))}} OPTIONAL{{?uri sbol2:role ?role . FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/'))}} FILTER(STRSTARTS(str(?uri), 'http://localhost:7777/')) FILTER NOT EXISTS {{ <{COLL}> sbol2:member ?om . {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }} FILTER(?om != ?uri) }} }}");
        assert!(
            matches!(
                rec(&q),
                Some(AcceleratedQuery::ObjectList {
                    scope: Scope::Collection {
                        root_only: true,
                        ..
                    },
                    ..
                })
            ),
            "got {:?}",
            rec(&q)
        );
    }

    #[test]
    fn declines_unknown() {
        assert!(rec("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1").is_none());
    }
}
