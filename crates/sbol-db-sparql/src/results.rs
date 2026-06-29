//! Result serialization helpers.
//!
//! - SELECT solutions and ASK booleans go through [`sparesults`] in JSON / XML
//!   / CSV / TSV.
//! - CONSTRUCT/DESCRIBE triples are first rendered to N-Triples (using oxrdf's
//!   built-in N-Triples-compatible `Display` impl for [`Triple`]) and then,
//!   for non-N-Triples formats, re-parsed and re-serialized through `sbol-rdf`.

use std::str::FromStr;

use oxrdf::{BlankNode, Literal, NamedNode, Term, Triple, Variable};
use sbol_db_core::{DomainError, SerializationFormat};
use sbol_db_storage::{AccelSolutions, TermValue};
use sparesults::{QueryResultsFormat, QueryResultsSerializer, QuerySolution};
use spareval::QuerySolutionIter;

use crate::SparqlError;

/// Output format for SPARQL results. The caller picks this either from a CLI
/// flag, the `Accept` header, or a `?format=` query parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultFormat {
    Json,
    Xml,
    Csv,
    Tsv,
    Turtle,
    NTriples,
    JsonLd,
    RdfXml,
}

impl ResultFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/sparql-results+json",
            Self::Xml => "application/sparql-results+xml",
            Self::Csv => "text/csv",
            Self::Tsv => "text/tab-separated-values",
            Self::Turtle => "text/turtle",
            Self::NTriples => "application/n-triples",
            Self::JsonLd => "application/ld+json",
            Self::RdfXml => "application/rdf+xml",
        }
    }

    pub fn is_solution_format(self) -> bool {
        matches!(self, Self::Json | Self::Xml | Self::Csv | Self::Tsv)
    }

    pub fn is_graph_format(self) -> bool {
        matches!(
            self,
            Self::Turtle | Self::NTriples | Self::JsonLd | Self::RdfXml
        )
    }
}

impl FromStr for ResultFormat {
    type Err = SparqlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" | "srj" | "sparql-results+json" | "application/sparql-results+json" => {
                Ok(Self::Json)
            }
            "xml" | "srx" | "sparql-results+xml" | "application/sparql-results+xml" => {
                Ok(Self::Xml)
            }
            "csv" | "text/csv" => Ok(Self::Csv),
            "tsv" | "tab-separated-values" | "text/tab-separated-values" => Ok(Self::Tsv),
            "turtle" | "ttl" | "text/turtle" => Ok(Self::Turtle),
            // text/plain has no registered RDF meaning, but the de-facto
            // convention (Virtuoso, and what SynBioHub's recursive fetch relies
            // on) is line-based N-Triples — the plain-text RDF serialization.
            "ntriples" | "nt" | "application/n-triples" | "text/plain" => Ok(Self::NTriples),
            "jsonld" | "json-ld" | "application/ld+json" => Ok(Self::JsonLd),
            "rdfxml" | "rdf-xml" | "rdf" | "application/rdf+xml" => Ok(Self::RdfXml),
            other => Err(SparqlError::UnsupportedFormat(other.to_owned())),
        }
    }
}

fn query_results_format(format: ResultFormat) -> Result<QueryResultsFormat, SparqlError> {
    Ok(match format {
        ResultFormat::Json => QueryResultsFormat::Json,
        ResultFormat::Xml => QueryResultsFormat::Xml,
        ResultFormat::Csv => QueryResultsFormat::Csv,
        ResultFormat::Tsv => QueryResultsFormat::Tsv,
        other => return Err(SparqlError::UnsupportedFormat(format!("{other:?}"))),
    })
}

/// Serialize a SELECT solution stream as bytes in the chosen format.
///
/// `max_rows` truncates the result iterator. If the cap is hit, `truncated`
/// in the returned payload is `true`.
pub fn serialize_solutions(
    solutions: QuerySolutionIter<'_>,
    format: ResultFormat,
    max_rows: usize,
) -> Result<ResultPayload, SparqlError> {
    let fmt = query_results_format(format)?;
    let variables = solutions.variables().to_vec();
    let serializer = QueryResultsSerializer::from_format(fmt);
    let mut buffer = Vec::with_capacity(1024);
    let mut writer = serializer
        .serialize_solutions_to_writer(&mut buffer, variables)
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;

    let mut truncated = false;
    for (count, row) in solutions.enumerate() {
        if count >= max_rows {
            truncated = true;
            break;
        }
        let solution: QuerySolution = row.map_err(|e| SparqlError::Evaluation(e.to_string()))?;
        writer
            .serialize(solution.iter().map(|(v, t)| (v.as_ref(), t.as_ref())))
            .map_err(|e| SparqlError::Serialization(e.to_string()))?;
    }
    writer
        .finish()
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;

    Ok(ResultPayload {
        content_type: format.content_type(),
        body: buffer,
        truncated,
    })
}

/// Serialize accelerator solutions in the chosen solution format, through the
/// same writer as [`serialize_solutions`] so the bytes match what generic
/// evaluation would produce.
pub fn serialize_accel_solutions(
    solutions: AccelSolutions,
    format: ResultFormat,
    max_rows: usize,
) -> Result<ResultPayload, SparqlError> {
    let fmt = query_results_format(format)?;
    let variables: Vec<Variable> = solutions.vars.iter().map(Variable::new_unchecked).collect();
    let serializer = QueryResultsSerializer::from_format(fmt);
    let mut buffer = Vec::with_capacity(1024);
    let mut writer = serializer
        .serialize_solutions_to_writer(&mut buffer, variables.clone())
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;

    let mut truncated = false;
    for (count, row) in solutions.rows.iter().enumerate() {
        if count >= max_rows {
            truncated = true;
            break;
        }
        let bound: Vec<(&Variable, Term)> = row
            .iter()
            .enumerate()
            .filter_map(|(i, cell)| cell.as_ref().map(|v| (&variables[i], term_of(v))))
            .collect();
        writer
            .serialize(bound.iter().map(|(v, t)| (v.as_ref(), t.as_ref())))
            .map_err(|e| SparqlError::Serialization(e.to_string()))?;
    }
    writer
        .finish()
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;

    Ok(ResultPayload {
        content_type: format.content_type(),
        body: buffer,
        truncated,
    })
}

fn term_of(value: &TermValue) -> Term {
    match value {
        TermValue::Iri(iri) => Term::NamedNode(NamedNode::new_unchecked(iri)),
        TermValue::Blank(id) => Term::BlankNode(BlankNode::new_unchecked(id)),
        TermValue::Literal {
            value,
            datatype,
            language,
        } => {
            let literal = if let Some(lang) = language {
                Literal::new_language_tagged_literal_unchecked(value, lang)
            } else {
                Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype))
            };
            Term::Literal(literal)
        }
    }
}

/// Serialize an ASK boolean.
pub fn serialize_boolean(value: bool, format: ResultFormat) -> Result<ResultPayload, SparqlError> {
    let fmt = query_results_format(format)?;
    let serializer = QueryResultsSerializer::from_format(fmt);
    let mut buffer = Vec::with_capacity(64);
    serializer
        .serialize_boolean_to_writer(&mut buffer, value)
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;
    Ok(ResultPayload {
        content_type: format.content_type(),
        body: buffer,
        truncated: false,
    })
}

/// Serialize a CONSTRUCT/DESCRIBE triple stream.
///
/// We collect first to N-Triples (oxrdf's `Triple` `Display` is N-Triples-
/// compatible), then for non-N-Triples formats re-parse via `sbol-rdf`.
pub fn serialize_triples<I>(
    triples: I,
    format: ResultFormat,
    max_rows: usize,
) -> Result<ResultPayload, SparqlError>
where
    I: Iterator<Item = Result<Triple, spareval::QueryEvaluationError>>,
{
    let mut ntriples = String::new();
    let mut truncated = false;
    for (count, triple) in triples.enumerate() {
        if count >= max_rows {
            truncated = true;
            break;
        }
        let t = triple.map_err(|e| SparqlError::Evaluation(e.to_string()))?;
        ntriples.push_str(&t.to_string());
        ntriples.push_str(" .\n");
    }
    if format == ResultFormat::NTriples {
        return Ok(ResultPayload {
            content_type: format.content_type(),
            body: ntriples.into_bytes(),
            truncated,
        });
    }
    let target = match format {
        ResultFormat::Turtle => SerializationFormat::Turtle,
        ResultFormat::JsonLd => SerializationFormat::JsonLd,
        ResultFormat::RdfXml => SerializationFormat::RdfXml,
        _ => return Err(SparqlError::UnsupportedFormat(format!("{format:?}"))),
    };
    let body = reserialize_through_sbol_rdf(&ntriples, target)?;
    Ok(ResultPayload {
        content_type: format.content_type(),
        body: body.into_bytes(),
        truncated,
    })
}

fn reserialize_through_sbol_rdf(
    ntriples: &str,
    target: SerializationFormat,
) -> Result<String, SparqlError> {
    let graph = sbol_rdf::Graph::parse(ntriples, sbol_rdf::RdfFormat::NTriples)
        .map_err(|e| SparqlError::Serialization(e.to_string()))?;
    let rdf_format = match target {
        SerializationFormat::Turtle => sbol_rdf::RdfFormat::Turtle,
        SerializationFormat::JsonLd => sbol_rdf::RdfFormat::JsonLd,
        SerializationFormat::RdfXml => sbol_rdf::RdfFormat::RdfXml,
        SerializationFormat::NTriples => sbol_rdf::RdfFormat::NTriples,
        other => {
            return Err(SparqlError::UnsupportedFormat(format!("{other:?}")));
        }
    };
    graph
        .write(rdf_format)
        .map_err(|e| SparqlError::Serialization(e.to_string()))
}

/// `From<DomainError>` for `SparqlError::Domain` is already derived; this
/// helper is mainly for the route handler that wraps `SparqlError` in the
/// API error type.
impl From<SparqlError> for DomainError {
    fn from(err: SparqlError) -> Self {
        match err {
            SparqlError::Domain(e) => e,
            other => DomainError::Validation(other.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct ResultPayload {
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub truncated: bool,
}
