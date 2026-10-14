//! Parsed store queries for SBOLExplorer's private/public search policy.
//!
//! Keep the private FILTER expression intact. Public text is ranked separately,
//! so only the classic lexical predicate is removed from its candidate query;
//! date filters, collection membership and other structural criteria survive.

use std::borrow::Cow;

use oxrdf::Variable;
use spargebra::algebra::{Expression, Function, GraphPattern};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::{Query, SparqlParser};

use crate::SparqlError;

#[derive(Clone, Debug)]
pub struct ExplorerSearchQuery {
    pub private_query: String,
    pub public_query: String,
    pub terms: Vec<String>,
    pub structural: bool,
}

impl ExplorerSearchQuery {
    pub fn parse(source: &str, count: bool) -> Result<Self, SparqlError> {
        // Classic SynBioHub can emit this empty-filter spelling. Explorer
        // treats it as browsing even though it is not valid SPARQL syntax.
        let source = match crate::explorer::search_template_criteria(source) {
            Some(criteria) if criteria.eq_ignore_ascii_case("FILTER ()") => {
                Cow::Owned(source.replacen(criteria, "", 1))
            }
            _ => Cow::Borrowed(source),
        };
        let Query::Select {
            pattern, base_iri, ..
        } = SparqlParser::new()
            .parse_query(&source)
            .map_err(|error| SparqlError::Parse(error.to_string()))?
        else {
            return Err(SparqlError::Unsupported(
                "Explorer search requires SELECT".into(),
            ));
        };
        let private = remove_modifiers(pattern, count);
        let mut terms = Vec::new();
        let public = remove_text_filters(private.clone(), &mut terms);
        let structural = has_structural_criteria(&public);
        let render = |inner| {
            Query::Select {
                dataset: None,
                base_iri: base_iri.clone(),
                pattern: GraphPattern::Distinct {
                    inner: Box::new(GraphPattern::Project {
                        inner: Box::new(inner),
                        variables: [
                            "subject",
                            "displayId",
                            "version",
                            "name",
                            "description",
                            "type",
                            "role",
                            "sbolType",
                        ]
                        .into_iter()
                        .map(Variable::new_unchecked)
                        .collect(),
                    }),
                },
            }
            .to_string()
        };
        Ok(Self {
            private_query: render(private),
            public_query: render(public),
            terms,
            structural,
        })
    }
}

fn remove_modifiers(pattern: GraphPattern, count: bool) -> GraphPattern {
    match pattern {
        GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::OrderBy { inner, .. } => remove_modifiers(*inner, count),
        GraphPattern::Group { inner, .. } | GraphPattern::Extend { inner, .. } if count => {
            remove_modifiers(*inner, count)
        }
        other => other,
    }
}

fn remove_text_filters(pattern: GraphPattern, terms: &mut Vec<String>) -> GraphPattern {
    match pattern {
        GraphPattern::Filter { expr, inner } => {
            let inner = Box::new(remove_text_filters(*inner, terms));
            match remove_text_expression(expr, terms) {
                Some(expr) => GraphPattern::Filter { expr, inner },
                None => *inner,
            }
        }
        GraphPattern::Join { left, right } => GraphPattern::Join {
            left: Box::new(remove_text_filters(*left, terms)),
            right: Box::new(remove_text_filters(*right, terms)),
        },
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => GraphPattern::LeftJoin {
            left: Box::new(remove_text_filters(*left, terms)),
            right,
            expression,
        },
        other => other,
    }
}

fn remove_text_expression(expr: Expression, terms: &mut Vec<String>) -> Option<Expression> {
    if is_text_expression(&expr) {
        collect_terms(&expr, terms);
        return None;
    }
    if let Expression::And(left, right) = expr {
        return match (
            remove_text_expression(*left, terms),
            remove_text_expression(*right, terms),
        ) {
            (Some(left), Some(right)) => Some(Expression::And(Box::new(left), Box::new(right))),
            (left, right) => left.or(right),
        };
    }
    Some(expr)
}

fn lexical_term(expr: &Expression) -> Option<(&str, &str)> {
    let Expression::FunctionCall(Function::Contains, args) = expr else {
        return None;
    };
    let [Expression::FunctionCall(Function::LCase, left), Expression::FunctionCall(Function::LCase, right)] =
        args.as_slice()
    else {
        return None;
    };
    let ([Expression::Variable(variable)], [Expression::Literal(literal)]) =
        (left.as_slice(), right.as_slice())
    else {
        return None;
    };
    matches!(variable.as_str(), "displayId" | "name" | "description")
        .then_some((variable.as_str(), literal.value()))
}

fn is_text_expression(expr: &Expression) -> bool {
    match expr {
        Expression::And(left, right) | Expression::Or(left, right) => {
            is_text_expression(left) && is_text_expression(right)
        }
        Expression::Not(inner) => is_text_expression(inner),
        _ => lexical_term(expr).is_some(),
    }
}

fn collect_terms(expr: &Expression, terms: &mut Vec<String>) {
    match expr {
        Expression::And(left, right) | Expression::Or(left, right) => {
            collect_terms(left, terms);
            collect_terms(right, terms);
        }
        Expression::Not(inner) => collect_terms(inner, terms),
        _ => {
            if let Some(("displayId", term)) = lexical_term(expr) {
                if !term.is_empty() {
                    terms.push(term.to_owned());
                }
            }
        }
    }
}

fn has_structural_criteria(pattern: &GraphPattern) -> bool {
    match pattern {
        GraphPattern::Bgp { patterns } => patterns.iter().any(|triple| !is_base_triple(triple)),
        GraphPattern::Join { left, right } => {
            has_structural_criteria(left) || has_structural_criteria(right)
        }
        GraphPattern::LeftJoin { left, .. } => has_structural_criteria(left),
        _ => true,
    }
}

fn is_base_triple(triple: &TriplePattern) -> bool {
    let (
        TermPattern::Variable(subject),
        NamedNodePattern::NamedNode(predicate),
        TermPattern::Variable(object),
    ) = (&triple.subject, &triple.predicate, &triple.object)
    else {
        return false;
    };
    subject.as_str() == "subject"
        && matches!(
            (predicate.as_str(), object.as_str()),
            ("http://www.w3.org/1999/02/22-rdf-syntax-ns#type", "type")
                | (
                    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel",
                    "subject"
                )
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(criteria: &str, count: bool) -> String {
        let body = format!("{criteria} ?subject a ?type . ?subject <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> ?subject . OPTIONAL {{ ?subject <http://sbols.org/v2#displayId> ?displayId }}");
        if count {
            format!("SELECT (SUM(?tempcount) AS ?count) WHERE {{ {{ SELECT (COUNT(DISTINCT ?subject) AS ?tempcount) WHERE {{ {body} }} }} }} LIMIT 1 OFFSET 7")
        } else {
            format!("SELECT DISTINCT ?subject ?displayId WHERE {{ {body} }} LIMIT 1 OFFSET 7")
        }
    }

    #[test]
    fn count_and_rows_keep_same_unpaged_private_predicate() {
        let criteria = "FILTER(CONTAINS(LCASE(?displayId), LCASE('LacI_Inv')) && !CONTAINS(LCASE(?name), LCASE('excluded')))";
        let rows = ExplorerSearchQuery::parse(&query(criteria, false), false).unwrap();
        let count = ExplorerSearchQuery::parse(&query(criteria, true), true).unwrap();
        assert_eq!(rows.private_query, count.private_query);
        assert_eq!(rows.public_query, count.public_query);
        assert!(rows.private_query.contains("CONTAINS"));
        assert!(!rows.public_query.contains("CONTAINS"));
        assert!(!rows.private_query.contains("OFFSET"));
        assert_eq!(rows.terms, ["LacI_Inv"]);
        assert!(!rows.structural);
    }

    #[test]
    fn public_candidate_query_retains_facets_and_non_text_filters() {
        let source = query("<http://example/collection> <http://sbols.org/v2#member> ?subject . ?subject <http://purl.org/dc/terms/created> ?date . FILTER(?date > 1 && CONTAINS(LCASE(?displayId), LCASE('promoter')))", false);
        let plan = ExplorerSearchQuery::parse(&source, false).unwrap();
        assert!(plan.structural);
        assert!(plan.public_query.contains("http://example/collection"));
        assert!(plan.public_query.contains("?date >"));
        assert!(!plan.public_query.contains("CONTAINS"));
    }

    #[test]
    fn quoted_keywords_are_parsed_as_literals() {
        let plan = ExplorerSearchQuery::parse(&query(r#"FILTER(CONTAINS(LCASE(?displayId), LCASE('O\'Brien')) || CONTAINS(LCASE(?description), LCASE('O\'Brien')))"#, false), false).unwrap();
        assert_eq!(plan.terms, ["O'Brien"]);
        assert!(!plan.structural);
        SparqlParser::new()
            .parse_query(&plan.private_query)
            .unwrap();
    }

    #[test]
    fn classic_empty_filter_is_browsing() {
        let source = "PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> SELECT ?subject WHERE { FILTER () ?subject a ?type . ?subject sbh:topLevel ?subject }";
        let plan = ExplorerSearchQuery::parse(source, false).unwrap();
        assert!(plan.terms.is_empty());
        assert!(!plan.structural);
    }
}
