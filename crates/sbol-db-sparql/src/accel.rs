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

    // The accelerated templates are single-shape BGPs (with OPTIONALs). A query
    // whose graph pattern contains a `UNION` or `MINUS` is a different shape:
    // pattern collection would flatten the alternatives into one set and match a
    // scope the query does not actually express — e.g. SynBioHub's member
    // types/roles query, `<c> sbol2:member ?m . {?m a ?u} UNION {?m sbol2:type ?u}
    // UNION {?m sbol2:role ?u}`, would be served as a single member field and
    // yield null/incorrect rows. Decline so the engine evaluates it. (The member
    // anti-join's `UNION` lives inside a `FILTER NOT EXISTS` expression, not the
    // pattern tree, so the root-member template is unaffected.)
    if has_union_or_minus(p) {
        return None;
    }

    if is_aggregate {
        let var = projection.first()?.as_str().to_owned();
        let (scope, subject_var) = detect_scope(&patterns, root_only)?;
        if pins_subject(p, &subject_var) || !scope_subject_predicates_known(&patterns, &subject_var)
        {
            return None;
        }
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

    // A constant-subject metadata fetch (`getMetadata`) has no scope variable;
    // recognize it from the algebra so required (non-`OPTIONAL`) fields are known.
    if let Some(q) = recognize_object_metadata(p, projection, &graph) {
        return Some(q);
    }

    let (scope, subject_var) = detect_scope(&patterns, root_only)?;
    if pins_subject(p, &subject_var) || !scope_subject_predicates_known(&patterns, &subject_var) {
        return None;
    }
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
                // The membership index enumerates a collection's members without
                // filtering by type. A query that also pins the member to a
                // constant rdf:type (e.g. SynBioHub's "sub-collections":
                // `?c a sbol2:Collection . <parent> sbol2:member ?c`) asks for
                // members *of that type*, which this scope cannot filter to.
                // Decline so the engine evaluates it generically rather than
                // returning every member regardless of type.
                if has_const_type(patterns, uri.as_str()) {
                    return None;
                }
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
                // The top-level index is type-agnostic and carries no member
                // anti-join. A query that also pins the subject to a constant
                // rdf:type, or that adds `FILTER NOT EXISTS { ?o sbol2:member ?s }`
                // (root_only), asks for a narrower set than the index holds
                // (SynBioHub's "manage"/root-collections query does both). Neither
                // is filterable here, so decline and let the engine evaluate it.
                if has_const_type(patterns, s.as_str()) || root_only {
                    return None;
                }
                return Some((Scope::TopLevel, s.as_str().to_owned()));
            }
        }
    }
    for t in patterns {
        if pred(t) == Some(RDF_TYPE) {
            if let (TermPattern::Variable(s), TermPattern::NamedNode(n)) = (&t.subject, &t.object) {
                // The by-type index enumerates every object of a type; the member
                // anti-join (`FILTER NOT EXISTS { ?o sbol2:member ?s }`, root_only)
                // that SynBioHub's public "browse root collections" query adds is
                // not precomputed for it. Decline so those root-only listings are
                // evaluated generically rather than including nested members.
                if root_only {
                    return None;
                }
                return Some((Scope::ByType(n.as_str().to_owned()), s.as_str().to_owned()));
            }
        }
    }
    None
}

/// Whether some pattern restricts `subject` to a constant rdf:type
/// (`?subject a <NamedNode>`), as opposed to the type-projection pattern
/// `?subject a ?type`.
fn has_const_type(patterns: &[&TriplePattern], subject: &str) -> bool {
    patterns.iter().any(|t| {
        pred(t) == Some(RDF_TYPE)
            && matches!(&t.subject, TermPattern::Variable(s) if s.as_str() == subject)
            && matches!(&t.object, TermPattern::NamedNode(_))
    })
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

/// Recognize a constant-subject metadata fetch (`getMetadata`):
///
/// ```sparql
/// SELECT ?name ?description WHERE {
///   <subject> dcterms:title ?name .
///   OPTIONAL { <subject> dcterms:description ?description }
/// }
/// ```
///
/// Every triple pattern shares one constant IRI subject and maps a known
/// metadata predicate to a distinct projected variable; patterns outside an
/// `OPTIONAL` are required. A variable subject, a filter, a sub-select, or a
/// bound variable that is not projected returns `None` (generic evaluation).
fn recognize_object_metadata(
    pattern: &GraphPattern,
    projection: &[Variable],
    graph: &str,
) -> Option<AcceleratedQuery> {
    let mut required: Vec<&TriplePattern> = Vec::new();
    let mut optional: Vec<&TriplePattern> = Vec::new();
    if !split_required_optional(pattern, false, &mut required, &mut optional) {
        return None;
    }
    if required.is_empty() {
        return None;
    }

    let mut subject: Option<&str> = None;
    let mut var_field: Vec<(String, Field)> = Vec::new();
    let mut required_vars: Vec<&str> = Vec::new();
    for (is_required, t) in required
        .iter()
        .map(|t| (true, *t))
        .chain(optional.iter().map(|t| (false, *t)))
    {
        let s = match &t.subject {
            TermPattern::NamedNode(n) => n.as_str(),
            _ => return None,
        };
        match subject {
            None => subject = Some(s),
            Some(prev) if prev == s => {}
            Some(_) => return None,
        }
        let var = match &t.object {
            TermPattern::Variable(v) => v.as_str(),
            _ => return None,
        };
        var_field.push((var.to_owned(), meta_field(pred(t)?)?));
        if is_required {
            required_vars.push(var);
        }
    }
    let subject = subject?.to_owned();

    // Every bound variable must be projected (an unprojected required var is an
    // extra inner join; an unprojected optional multi-valued var would multiply
    // rows), and every projected var must be one of the bound metadata fields.
    let proj_names: Vec<&str> = projection.iter().map(Variable::as_str).collect();
    if var_field
        .iter()
        .any(|(v, _)| !proj_names.contains(&v.as_str()))
    {
        return None;
    }
    let mut proj = Vec::with_capacity(projection.len());
    let mut required_flags = Vec::with_capacity(projection.len());
    for v in projection {
        let name = v.as_str();
        let field = var_field.iter().find(|(n, _)| n == name).map(|(_, f)| *f)?;
        proj.push((name.to_owned(), field));
        required_flags.push(required_vars.contains(&name));
    }
    Some(AcceleratedQuery::ObjectMetadata {
        graph: graph.to_owned(),
        subject,
        projection: proj,
        required: required_flags,
    })
}

/// Split a graph pattern's triple patterns into required (outside any `OPTIONAL`)
/// and optional (the right side of a `LeftJoin`). Returns `false` for any shape
/// outside the simple join/optional/graph nesting `getMetadata` uses (a filter,
/// a union, a sub-select, a filtered `OPTIONAL`, ...), leaving the query to
/// generic evaluation.
fn split_required_optional<'a>(
    pattern: &'a GraphPattern,
    in_optional: bool,
    required: &mut Vec<&'a TriplePattern>,
    optional: &mut Vec<&'a TriplePattern>,
) -> bool {
    use GraphPattern::*;
    match pattern {
        Bgp { patterns } => {
            if in_optional { optional } else { required }.extend(patterns.iter());
            true
        }
        Join { left, right } => {
            split_required_optional(left, in_optional, required, optional)
                && split_required_optional(right, in_optional, required, optional)
        }
        LeftJoin {
            left,
            right,
            expression,
        } => {
            expression.is_none()
                && split_required_optional(left, in_optional, required, optional)
                && split_required_optional(right, true, required, optional)
        }
        Graph { inner, .. } => split_required_optional(inner, in_optional, required, optional),
        _ => false,
    }
}

/// Map a metadata predicate IRI to the field it projects.
fn meta_field(predicate: &str) -> Option<Field> {
    match predicate {
        RDF_TYPE => Some(Field::Type),
        DISPLAY_ID => Some(Field::DisplayId),
        VERSION => Some(Field::Version),
        TITLE => Some(Field::Name),
        DESCRIPTION => Some(Field::Description),
        SBOL_TYPE => Some(Field::SbolType),
        ROLE => Some(Field::Role),
        _ => None,
    }
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

/// Whether a `FILTER` restricts the scope's subject variable to a constant term
/// (`FILTER(?s = <iri>)`, `sameTerm(?s, <iri>)`, or `?s IN (<iri>, ...)`). The
/// accelerator's scopes enumerate every object of a type / top-level set /
/// collection and cannot apply a subject restriction, so a query that pins the
/// subject this way — e.g. SynBioHub's "is <X> a Collection?" probe,
/// `?c a sbol2:Collection . FILTER(?c = <X>)` — must be declined and evaluated
/// generically. Otherwise the accelerator answers for the whole type, ignoring
/// the filter, and reports a match for a subject that is not of that type.
/// The predicates an accelerated scope can carry on its *enumerated* subject:
/// the type assertion, the top-level self-edge, and the projected metadata
/// fields. Anything else (e.g. `?s sbol2:member <x>`, which restricts the
/// enumerated subject to collections containing `<x>`) is a constraint the scope
/// indexes don't encode.
const SCOPE_SUBJECT_PREDS: &[&str] = &[
    RDF_TYPE,
    TOPLEVEL,
    DISPLAY_ID,
    VERSION,
    TITLE,
    DESCRIPTION,
    SBOL_TYPE,
    ROLE,
    CREATOR,
];

/// Whether every triple pattern whose subject is the scope's enumerated variable
/// uses only [`SCOPE_SUBJECT_PREDS`]. A pattern that puts another predicate on
/// the enumerated subject is an extra constraint the scope cannot honor (e.g.
/// SynBioHub's "collections containing <x>" query, `?s a sbol2:Collection . ?s
/// sbol2:member <x>`), so the recognizer must decline rather than enumerate the
/// whole type. (Scope-defining patterns like `<coll> sbol2:member ?s` put the
/// constant collection — not `?s` — in subject position, so they are unaffected.)
fn scope_subject_predicates_known(patterns: &[&TriplePattern], subject_var: &str) -> bool {
    patterns.iter().all(|t| match &t.subject {
        TermPattern::Variable(s) if s.as_str() == subject_var => {
            matches!(pred(t), Some(p) if SCOPE_SUBJECT_PREDS.contains(&p))
        }
        _ => true,
    })
}

/// Whether the graph pattern tree contains a `UNION` or `MINUS` node. Only the
/// pattern tree is walked, not filter expressions, so a `FILTER NOT EXISTS`
/// whose sub-pattern uses `UNION` (the root-member anti-join) is not flagged.
fn has_union_or_minus(pattern: &GraphPattern) -> bool {
    use GraphPattern::*;
    match pattern {
        Union { .. } | Minus { .. } => true,
        Join { left, right } | LeftJoin { left, right, .. } => {
            has_union_or_minus(left) || has_union_or_minus(right)
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
        | Service { inner, .. } => has_union_or_minus(inner),
        _ => false,
    }
}

fn pins_subject(pattern: &GraphPattern, subject_var: &str) -> bool {
    use GraphPattern::*;
    match pattern {
        Filter { expr, inner } => {
            expr_pins_subject(expr, subject_var) || pins_subject(inner, subject_var)
        }
        Join { left, right }
        | LeftJoin { left, right, .. }
        | Union { left, right }
        | Minus { left, right } => {
            pins_subject(left, subject_var) || pins_subject(right, subject_var)
        }
        Graph { inner, .. }
        | Extend { inner, .. }
        | OrderBy { inner, .. }
        | Project { inner, .. }
        | Distinct { inner }
        | Reduced { inner }
        | Slice { inner, .. }
        | Group { inner, .. }
        | Service { inner, .. } => pins_subject(inner, subject_var),
        _ => false,
    }
}

fn expr_pins_subject(expr: &spargebra::algebra::Expression, subject_var: &str) -> bool {
    use spargebra::algebra::Expression::{self, *};
    fn is_subject(e: &Expression, subject_var: &str) -> bool {
        matches!(e, Expression::Variable(v) if v.as_str() == subject_var)
    }
    fn is_const(e: &Expression) -> bool {
        matches!(e, Expression::NamedNode(_) | Expression::Literal(_))
    }
    match expr {
        And(a, b) | Or(a, b) => {
            expr_pins_subject(a, subject_var) || expr_pins_subject(b, subject_var)
        }
        Equal(a, b) | SameTerm(a, b) => {
            (is_subject(a, subject_var) && is_const(b))
                || (is_const(a) && is_subject(b, subject_var))
        }
        In(e, list) => is_subject(e, subject_var) && !list.is_empty() && list.iter().all(is_const),
        _ => false,
    }
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
    fn recognizes_get_metadata() {
        let top = "http://synbiohub.org/public/Foo/Foo_collection/1";
        let q = format!("SELECT ?name ?description FROM <{G}> WHERE {{ <{top}> dcterms:title ?name . OPTIONAL {{ <{top}> dcterms:description ?description }} }}");
        match rec(&q) {
            Some(AcceleratedQuery::ObjectMetadata {
                subject,
                projection,
                required,
                ..
            }) => {
                assert_eq!(subject, top);
                assert_eq!(
                    projection,
                    vec![
                        ("name".to_owned(), Field::Name),
                        ("description".to_owned(), Field::Description)
                    ]
                );
                // Title is required (outside OPTIONAL); description is optional.
                assert_eq!(required, vec![true, false]);
            }
            other => panic!("expected ObjectMetadata, got {other:?}"),
        }
    }

    #[test]
    fn metadata_declines_variable_subject() {
        // A variable subject is a listing/search shape, not a single-object fetch.
        let q = format!("SELECT ?name FROM <{G}> WHERE {{ ?s dcterms:title ?name }}");
        assert!(!matches!(
            rec(&q),
            Some(AcceleratedQuery::ObjectMetadata { .. })
        ));
    }

    #[test]
    fn declines_unknown() {
        assert!(rec("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1").is_none());
    }

    // SynBioHub's "manage" / root-collections query pins the subject to a constant
    // type *and* requires it to be top-level. The top-level index is type-agnostic,
    // so serving it as `Scope::TopLevel` would return every top-level object
    // (component definitions, sequences, ...) regardless of type. The recognizer
    // must decline these so the engine evaluates them generically.
    #[test]
    fn declines_toplevel_with_constant_type() {
        let q = format!(
            "SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role \
             FROM <{G}> WHERE {{ ?subject a sbol2:Collection . \
             FILTER NOT EXISTS {{ ?otherCollection sbol2:member ?subject }} \
             ?subject a ?type . ?subject sbh:topLevel ?subject . \
             OPTIONAL{{?subject sbol2:displayId ?displayId}} \
             OPTIONAL{{?subject sbol2:version ?version}} \
             OPTIONAL{{?subject dcterms:title ?name}} \
             OPTIONAL{{?subject dcterms:description ?description}} \
             OPTIONAL{{?subject sbol2:type ?sbolType}} \
             OPTIONAL{{?subject sbol2:role ?role}} }}"
        );
        assert!(
            rec(&q).is_none(),
            "manage/root-collections query must decline"
        );
    }

    // SynBioHub's "sub-collections" query restricts a collection's members to a
    // constant type (`?c a sbol2:Collection . <parent> sbol2:member ?c`). The
    // membership index can't filter members by type, so the recognizer must
    // decline rather than return every member.
    // SynBioHub's public "browse" lists root collections: a by-type listing with
    // a member anti-join (`?s a sbol2:Collection . FILTER NOT EXISTS { ?o member
    // ?s }`). The by-type index has no root anti-join, so the recognizer must
    // decline rather than include nested (member) collections.
    // SynBioHub's download path probes "is <X> a Collection?" with
    // `?c a sbol2:Collection . FILTER(?c = <X>)`. The by-type index can't apply
    // the subject-equality filter, so the recognizer must decline — otherwise it
    // answers for every Collection and wrongly reports <X> as a Collection.
    // SynBioHub's member types/roles query unions three alternatives over a
    // collection's members. Flattening the union would match a single member
    // field and yield null/incorrect rows, so a query with a UNION must decline.
    // SynBioHub's "collections containing <x>" query restricts the enumerated
    // collection to those with <x> as a member: `?s a sbol2:Collection . ?s
    // sbol2:member <x>`. The by-type index can't apply that membership
    // constraint, so the recognizer must decline rather than list every
    // collection.
    #[test]
    fn declines_bytype_with_member_constraint() {
        let q = format!(
            "SELECT ?subject ?displayId FROM <{G}> WHERE {{ ?subject a sbol2:Collection . \
             ?subject sbol2:member <{G}/x/1> . \
             OPTIONAL {{ ?subject sbol2:displayId ?displayId }} }}"
        );
        assert!(
            rec(&q).is_none(),
            "collections-containing-<x> query must decline"
        );
    }

    #[test]
    fn declines_union_member_types_roles() {
        let q = format!(
            "SELECT DISTINCT ?uri FROM <{G}> WHERE {{ <{G}/c/1> sbol2:member ?m . \
             {{ ?m a ?uri }} UNION {{ ?m sbol2:type ?uri }} UNION {{ ?m sbol2:role ?uri }} }}"
        );
        assert!(
            rec(&q).is_none(),
            "union member types/roles query must decline"
        );
    }

    #[test]
    fn declines_bytype_with_subject_equality_filter() {
        let q = format!(
            "SELECT ?c FROM <{G}> WHERE {{ ?c a sbol2:Collection . \
             FILTER(?c = <{G}/part_pIKE_Toggle_1/1>) }}"
        );
        assert!(
            rec(&q).is_none(),
            "is-X-a-Collection probe (subject-equality filter) must decline"
        );
    }

    #[test]
    fn declines_bytype_root_only() {
        let q = format!(
            "SELECT ?object ?name FROM <{G}> WHERE {{ ?object a sbol2:Collection . \
             FILTER NOT EXISTS {{ ?otherCollection sbol2:member ?object }} \
             OPTIONAL{{?object dcterms:title ?name}} }}"
        );
        assert!(
            rec(&q).is_none(),
            "browse root-collections query must decline"
        );
    }

    #[test]
    fn declines_member_with_constant_type() {
        let q = format!(
            "SELECT ?Collection ?name ?displayId FROM <{G}> WHERE {{ \
             ?Collection a sbol2:Collection . \
             <{G}/parent/1> sbol2:member ?Collection . \
             OPTIONAL{{?Collection dcterms:title ?name}} \
             OPTIONAL{{?Collection sbol2:displayId ?displayId}} \
             FILTER NOT EXISTS {{ <{G}/parent/1> sbol2:member ?o . ?o sbol2:member ?Collection }} }}"
        );
        assert!(rec(&q).is_none(), "sub-collections query must decline");
    }

    #[test]
    fn declines_toplevel_with_constant_type_count() {
        let q = format!(
            "SELECT (sum(?tc) AS ?count) FROM <{G}> WHERE {{ \
             {{ SELECT (count(distinct ?subject) AS ?tc) WHERE {{ \
             ?subject a sbol2:Collection . \
             FILTER NOT EXISTS {{ ?otherCollection sbol2:member ?subject }} \
             ?subject a ?type . ?subject sbh:topLevel ?subject }} }} }}"
        );
        assert!(
            rec(&q).is_none(),
            "manage/root-collections count query must decline"
        );
    }
}
