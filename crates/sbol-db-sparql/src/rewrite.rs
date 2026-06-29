//! Algebra rewriting applied before evaluation.
//!
//! spareval evaluates `MINUS` over a conjunctive pattern as a hash anti-join
//! (build the right side once, probe the left), but evaluates `FILTER NOT
//! EXISTS` once per candidate row (a nested loop), and does not hash-optimize a
//! `MINUS` whose right side contains a `UNION`. SynBioHub's "top-level members"
//! query is a `FILTER NOT EXISTS` over a member-reference `UNION`, so for a
//! collection with N members it is O(N^2): on a real corpus, tens of seconds.
//!
//! This pass rewrites `FILTER NOT EXISTS { B }` into one `MINUS` per conjunctive
//! disjunct of `B` (distributing any unions), turning each into a hash
//! anti-join. `NOT EXISTS { B }` is equivalent to the conjunction over `B`'s
//! disjuncts of `NOT EXISTS { disjunct }`, and each `NOT EXISTS { conjunct }`
//! equals `MINUS { conjunct }` under the conditions checked in
//! [`safe_as_minus`]. When any condition fails (or the body uses a pattern shape
//! this pass does not analyze), the original `FILTER NOT EXISTS` is kept, so the
//! rewrite never changes a query's results.

use std::collections::HashSet;

use spargebra::algebra::{Expression, GraphPattern};
use spargebra::term::{NamedNodePattern, TermPattern};
use spargebra::Query;

/// Cap on the conjunctive disjuncts produced by distributing unions in a
/// `NOT EXISTS` body. Beyond this the rewrite is skipped, bounding expansion.
const MAX_DISJUNCTS: usize = 16;

/// Rewrite a parsed query for faster evaluation. Currently turns eligible
/// `FILTER NOT EXISTS` filters into `MINUS` anti-joins.
pub fn optimize(query: Query) -> Query {
    match query {
        Query::Select {
            dataset,
            pattern,
            base_iri,
        } => Query::Select {
            dataset,
            pattern: rewrite(pattern),
            base_iri,
        },
        Query::Construct {
            template,
            dataset,
            pattern,
            base_iri,
        } => Query::Construct {
            template,
            dataset,
            pattern: rewrite(pattern),
            base_iri,
        },
        Query::Describe {
            dataset,
            pattern,
            base_iri,
        } => Query::Describe {
            dataset,
            pattern: rewrite(pattern),
            base_iri,
        },
        Query::Ask {
            dataset,
            pattern,
            base_iri,
        } => Query::Ask {
            dataset,
            pattern: rewrite(pattern),
            base_iri,
        },
    }
}

fn rewrite(pattern: GraphPattern) -> GraphPattern {
    use GraphPattern::*;
    match pattern {
        Filter { expr, inner } => rewrite_filter(expr, rewrite(*inner)),
        Join { left, right } => Join {
            left: Box::new(rewrite(*left)),
            right: Box::new(rewrite(*right)),
        },
        LeftJoin {
            left,
            right,
            expression,
        } => LeftJoin {
            left: Box::new(rewrite(*left)),
            right: Box::new(rewrite(*right)),
            expression,
        },
        Union { left, right } => Union {
            left: Box::new(rewrite(*left)),
            right: Box::new(rewrite(*right)),
        },
        Minus { left, right } => Minus {
            left: Box::new(rewrite(*left)),
            right: Box::new(rewrite(*right)),
        },
        Graph { name, inner } => Graph {
            name,
            inner: Box::new(rewrite(*inner)),
        },
        Extend {
            inner,
            variable,
            expression,
        } => Extend {
            inner: Box::new(rewrite(*inner)),
            variable,
            expression,
        },
        OrderBy { inner, expression } => OrderBy {
            inner: Box::new(rewrite(*inner)),
            expression,
        },
        Project { inner, variables } => Project {
            inner: Box::new(rewrite(*inner)),
            variables,
        },
        Distinct { inner } => Distinct {
            inner: Box::new(rewrite(*inner)),
        },
        Reduced { inner } => Reduced {
            inner: Box::new(rewrite(*inner)),
        },
        Slice {
            inner,
            start,
            length,
        } => Slice {
            inner: Box::new(rewrite(*inner)),
            start,
            length,
        },
        Group {
            inner,
            variables,
            aggregates,
        } => Group {
            inner: Box::new(rewrite(*inner)),
            variables,
            aggregates,
        },
        Service {
            name,
            inner,
            silent,
        } => Service {
            name,
            inner: Box::new(rewrite(*inner)),
            silent,
        },
        // Bgp, Path, Values, and any feature-gated variants have no nested
        // pattern this pass rewrites.
        other => other,
    }
}

/// Reconstruct a `Filter`, peeling each `NOT EXISTS` conjunct into `MINUS`
/// anti-joins where that is provably equivalent. Conjuncts that are not an
/// eligible `NOT EXISTS` stay in the residual filter expression.
fn rewrite_filter(expr: Expression, inner: GraphPattern) -> GraphPattern {
    let outer = in_scope(&inner);
    let mut current = inner;
    let mut keep: Vec<Expression> = Vec::new();

    for conjunct in split_and(expr) {
        match as_not_exists(&conjunct) {
            Some(body) => match try_anti_join(current, body, &outer) {
                Ok(rewritten) => current = rewritten,
                Err(unchanged) => {
                    current = unchanged;
                    keep.push(conjunct);
                }
            },
            None => keep.push(conjunct),
        }
    }

    match combine_and(keep) {
        Some(expr) => GraphPattern::Filter {
            expr,
            inner: Box::new(current),
        },
        None => current,
    }
}

/// Turn `left` into `MINUS`-anti-joins against each conjunctive disjunct of a
/// `NOT EXISTS` body. Returns `Err(left)` unchanged if the body cannot be safely
/// rewritten.
fn try_anti_join(
    left: GraphPattern,
    body: &GraphPattern,
    outer: &HashSet<String>,
) -> Result<GraphPattern, GraphPattern> {
    let disjuncts = match disjuncts_of(body) {
        Some(d) if !d.is_empty() && d.len() <= MAX_DISJUNCTS => d,
        _ => return Err(left),
    };
    if !disjuncts.iter().all(|d| safe_as_minus(d, outer)) {
        return Err(left);
    }
    let mut result = left;
    for disjunct in disjuncts {
        result = GraphPattern::Minus {
            left: Box::new(result),
            right: Box::new(disjunct),
        };
    }
    Ok(result)
}

/// `NOT EXISTS { d }` equals `MINUS { d }` when `d` shares a bound variable with
/// the outer pattern (otherwise `MINUS` is a no-op while `NOT EXISTS` is not),
/// and `d` binds every outer variable it mentions (a variable an outer row binds
/// but `d` only reads in a filter is substituted by `NOT EXISTS` yet treated as
/// independent by `MINUS`).
fn safe_as_minus(disjunct: &GraphPattern, outer: &HashSet<String>) -> bool {
    let bound = in_scope(disjunct);
    let shares_bound = bound.iter().any(|v| outer.contains(v));
    if !shares_bound {
        return false;
    }
    let mut used = HashSet::new();
    if !collect_used(disjunct, &mut used) {
        return false;
    }
    used.iter().all(|v| !outer.contains(v) || bound.contains(v))
}

/// The conjunctive disjuncts of a pattern: distribute unions to the top so each
/// result is union-free. Returns `None` for any shape this pass does not
/// distribute or analyze (the caller then keeps the `NOT EXISTS`).
fn disjuncts_of(pattern: &GraphPattern) -> Option<Vec<GraphPattern>> {
    use GraphPattern::*;
    match pattern {
        Bgp { .. } | Path { .. } => Some(vec![pattern.clone()]),
        Union { left, right } => {
            let mut out = disjuncts_of(left)?;
            out.extend(disjuncts_of(right)?);
            Some(out)
        }
        Join { left, right } => {
            let ls = disjuncts_of(left)?;
            let rs = disjuncts_of(right)?;
            let mut out = Vec::with_capacity(ls.len() * rs.len());
            for l in &ls {
                for r in &rs {
                    if out.len() >= MAX_DISJUNCTS {
                        return None;
                    }
                    out.push(Join {
                        left: Box::new(l.clone()),
                        right: Box::new(r.clone()),
                    });
                }
            }
            Some(out)
        }
        Filter { expr, inner } => {
            if expr_has_exists(expr) {
                return None;
            }
            Some(
                disjuncts_of(inner)?
                    .into_iter()
                    .map(|d| Filter {
                        expr: expr.clone(),
                        inner: Box::new(d),
                    })
                    .collect(),
            )
        }
        Graph { name, inner } => Some(
            disjuncts_of(inner)?
                .into_iter()
                .map(|d| Graph {
                    name: name.clone(),
                    inner: Box::new(d),
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Collect every variable a (union-free) disjunct mentions, in triple patterns
/// and in filter expressions. Returns `false` if it meets a shape it cannot
/// fully account for, so the caller treats the rewrite as unsafe.
fn collect_used(pattern: &GraphPattern, used: &mut HashSet<String>) -> bool {
    use GraphPattern::*;
    match pattern {
        Bgp { patterns } => {
            for t in patterns {
                term_var(&t.subject, used);
                if let NamedNodePattern::Variable(v) = &t.predicate {
                    used.insert(v.as_str().to_owned());
                }
                term_var(&t.object, used);
            }
            true
        }
        Path {
            subject, object, ..
        } => {
            term_var(subject, used);
            term_var(object, used);
            true
        }
        Join { left, right } => collect_used(left, used) && collect_used(right, used),
        Filter { expr, inner } => {
            collect_expr_used(expr, used);
            collect_used(inner, used)
        }
        Graph { name, inner } => {
            if let NamedNodePattern::Variable(v) = name {
                used.insert(v.as_str().to_owned());
            }
            collect_used(inner, used)
        }
        _ => false,
    }
}

fn term_var(term: &TermPattern, used: &mut HashSet<String>) {
    if let TermPattern::Variable(v) = term {
        used.insert(v.as_str().to_owned());
    }
}

fn collect_expr_used(expr: &Expression, used: &mut HashSet<String>) {
    use Expression::*;
    match expr {
        Variable(v) | Bound(v) => {
            used.insert(v.as_str().to_owned());
        }
        NamedNode(_) | Literal(_) => {}
        Or(a, b)
        | And(a, b)
        | Equal(a, b)
        | SameTerm(a, b)
        | Greater(a, b)
        | GreaterOrEqual(a, b)
        | Less(a, b)
        | LessOrEqual(a, b)
        | Add(a, b)
        | Subtract(a, b)
        | Multiply(a, b)
        | Divide(a, b) => {
            collect_expr_used(a, used);
            collect_expr_used(b, used);
        }
        UnaryPlus(a) | UnaryMinus(a) | Not(a) => collect_expr_used(a, used),
        In(a, list) => {
            collect_expr_used(a, used);
            list.iter().for_each(|e| collect_expr_used(e, used));
        }
        If(a, b, c) => {
            collect_expr_used(a, used);
            collect_expr_used(b, used);
            collect_expr_used(c, used);
        }
        Coalesce(list) | FunctionCall(_, list) => {
            list.iter().for_each(|e| collect_expr_used(e, used))
        }
        // `disjuncts_of` rejects bodies whose filters contain EXISTS, so this is
        // unreachable for analyzed disjuncts; collecting nothing is safe because
        // such a disjunct is never rewritten.
        Exists(_) => {}
    }
}

fn expr_has_exists(expr: &Expression) -> bool {
    use Expression::*;
    match expr {
        Exists(_) => true,
        Variable(_) | Bound(_) | NamedNode(_) | Literal(_) => false,
        Or(a, b)
        | And(a, b)
        | Equal(a, b)
        | SameTerm(a, b)
        | Greater(a, b)
        | GreaterOrEqual(a, b)
        | Less(a, b)
        | LessOrEqual(a, b)
        | Add(a, b)
        | Subtract(a, b)
        | Multiply(a, b)
        | Divide(a, b) => expr_has_exists(a) || expr_has_exists(b),
        UnaryPlus(a) | UnaryMinus(a) | Not(a) => expr_has_exists(a),
        In(a, list) => expr_has_exists(a) || list.iter().any(expr_has_exists),
        If(a, b, c) => expr_has_exists(a) || expr_has_exists(b) || expr_has_exists(c),
        Coalesce(list) | FunctionCall(_, list) => list.iter().any(expr_has_exists),
    }
}

fn as_not_exists(expr: &Expression) -> Option<&GraphPattern> {
    match expr {
        Expression::Not(inner) => match inner.as_ref() {
            Expression::Exists(pattern) => Some(pattern.as_ref()),
            _ => None,
        },
        _ => None,
    }
}

fn split_and(expr: Expression) -> Vec<Expression> {
    match expr {
        Expression::And(a, b) => {
            let mut out = split_and(*a);
            out.extend(split_and(*b));
            out
        }
        other => vec![other],
    }
}

fn combine_and(mut exprs: Vec<Expression>) -> Option<Expression> {
    let mut combined = exprs.pop()?;
    while let Some(next) = exprs.pop() {
        combined = Expression::And(Box::new(next), Box::new(combined));
    }
    Some(combined)
}

fn in_scope(pattern: &GraphPattern) -> HashSet<String> {
    let mut vars = HashSet::new();
    pattern.on_in_scope_variable(|v| {
        vars.insert(v.as_str().to_owned());
    });
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewritten(query: &str) -> String {
        let q = spargebra::SparqlParser::new()
            .parse_query(query)
            .expect("parse");
        optimize(q).to_string()
    }

    const PREFIXES: &str = "PREFIX : <http://ex/>\n";

    #[test]
    fn not_exists_with_union_becomes_two_minus() {
        let q = format!(
            "{PREFIXES}SELECT ?uri WHERE {{
                :c :member ?uri .
                FILTER NOT EXISTS {{
                    :c :member ?om .
                    {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }}
                    FILTER(?om != ?uri)
                }}
            }}"
        );
        let out = rewritten(&q);
        assert!(out.contains("MINUS"), "expected MINUS, got: {out}");
        assert!(!out.contains("EXISTS"), "EXISTS should be gone: {out}");
        assert_eq!(
            out.matches("MINUS").count(),
            2,
            "one MINUS per branch: {out}"
        );
    }

    #[test]
    fn simple_not_exists_becomes_minus() {
        let q = format!(
            "{PREFIXES}SELECT ?s WHERE {{ ?s a :T . FILTER NOT EXISTS {{ ?s :flag ?f }} }}"
        );
        let out = rewritten(&q);
        assert!(out.contains("MINUS"), "{out}");
        assert!(!out.contains("EXISTS"), "{out}");
    }

    #[test]
    fn no_shared_variable_is_left_alone() {
        // The body shares no variable with the outer pattern, so MINUS would be a
        // no-op while NOT EXISTS is not: must not rewrite.
        let q = format!(
            "{PREFIXES}SELECT ?s WHERE {{ ?s a :T . FILTER NOT EXISTS {{ ?x :flag ?f }} }}"
        );
        let out = rewritten(&q);
        assert!(out.contains("EXISTS"), "must keep NOT EXISTS: {out}");
        assert!(!out.contains("MINUS"), "{out}");
    }

    #[test]
    fn correlated_filter_variable_is_left_alone() {
        // ?s is used only inside the body's FILTER and never bound there, so the
        // substitution semantics of NOT EXISTS differ from MINUS: must not rewrite.
        let q = format!(
            "{PREFIXES}SELECT ?s WHERE {{ ?s :age ?a . FILTER NOT EXISTS {{ ?x :age ?b . FILTER(?b > ?a) }} }}"
        );
        let out = rewritten(&q);
        assert!(out.contains("EXISTS"), "must keep NOT EXISTS: {out}");
    }

    #[test]
    fn residual_filter_is_preserved() {
        let q = format!(
            "{PREFIXES}SELECT ?s WHERE {{ ?s a :T . FILTER(?s != :x && NOT EXISTS {{ ?s :flag ?f }}) }}"
        );
        let out = rewritten(&q);
        assert!(out.contains("MINUS"), "{out}");
        assert!(out.contains("FILTER"), "residual filter kept: {out}");
    }
}
