//! SPARQL query engine — parses, evaluates, and serializes results.
//!
//! The evaluation runs inside `tokio::task::spawn_blocking` because the
//! [`TripleDataset`]'s `QueryableDataset` iterators are synchronous and a
//! [`TripleSource`] may block while fetching per-pattern rows. The whole
//! spawn_blocking handle is wrapped in `tokio::time::timeout` to bound query
//! time. Sync evaluator code can't be preempted by tokio — past the deadline
//! the task may still run a short while before its next pattern fetch
//! terminates — so the timeout is "best-effort soft cap" rather than a hard
//! kill.

use std::sync::Arc;
use std::time::Duration;

use oxrdf::{GraphName, NamedNode};
use sbol_db_storage::TripleSource;
use spareval::{QueryEvaluator, QueryResults};
use spargebra::SparqlParser;

use crate::dataset::TripleDataset;
use crate::error::SparqlError;
use crate::results::{
    serialize_boolean, serialize_solutions, serialize_triples, ResultFormat, ResultPayload,
};

#[derive(Clone, Debug)]
pub struct SparqlOptions {
    /// Wall-clock cap on a single query (best-effort; see module docs).
    pub timeout: Duration,
    /// Cap on serialized solution/triple rows. If hit, the response carries
    /// `truncated = true`.
    pub max_rows: usize,
    /// Reject query strings exceeding this byte length. Cheap shield against
    /// unbounded posts.
    pub max_query_size: usize,
    /// The SPARQL-protocol `default-graph-uri`: the graph a query treats as
    /// its default graph when it carries no `FROM` clause of its own. `None`
    /// preserves sbol-db's native behavior (default graph = union of all named
    /// graphs). SynBioHub always supplies this, scoping reads to one graph.
    pub default_graph: Option<String>,
}

impl Default for SparqlOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_rows: 100_000,
            max_query_size: 64 * 1024,
            default_graph: None,
        }
    }
}

/// Final serialized output plus enough metadata to render content-type and
/// truncation headers.
#[derive(Debug)]
pub struct SparqlOutcome {
    pub payload: ResultPayload,
    pub query_form: QueryForm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QueryForm {
    Select,
    Ask,
    Construct,
    Describe,
}

impl QueryForm {
    fn default_format(self) -> ResultFormat {
        match self {
            QueryForm::Select | QueryForm::Ask => ResultFormat::Json,
            QueryForm::Construct | QueryForm::Describe => ResultFormat::Turtle,
        }
    }

    fn allows(self, format: ResultFormat) -> bool {
        match self {
            QueryForm::Select | QueryForm::Ask => format.is_solution_format(),
            QueryForm::Construct | QueryForm::Describe => format.is_graph_format(),
        }
    }
}

#[derive(Clone)]
pub struct SparqlEngine {
    source: Arc<dyn TripleSource>,
}

impl SparqlEngine {
    pub fn new(source: Arc<dyn TripleSource>) -> Self {
        Self { source }
    }

    /// Run a SPARQL query and serialize the result.
    ///
    /// `requested_format = None` picks the form's natural default (JSON for
    /// SELECT/ASK, Turtle for CONSTRUCT/DESCRIBE). Mismatches between the
    /// requested format and the query form (e.g. CSV for CONSTRUCT) return
    /// [`SparqlError::UnsupportedFormat`].
    pub async fn execute(
        &self,
        query_str: &str,
        requested_format: Option<ResultFormat>,
        options: &SparqlOptions,
    ) -> Result<SparqlOutcome, SparqlError> {
        if query_str.len() > options.max_query_size {
            return Err(SparqlError::QueryTooLarge);
        }

        let query = crate::rewrite::optimize(parse_query_strict(query_str)?);
        let query_form = classify_query(&query);
        let format = requested_format.unwrap_or_else(|| query_form.default_format());
        if !query_form.allows(format) {
            return Err(SparqlError::UnsupportedFormat(format!(
                "{format:?} is not a valid result format for a {query_form:?} query"
            )));
        }

        let dataset = TripleDataset::new(Arc::clone(&self.source));
        let max_rows = options.max_rows;
        let default_graph = options.default_graph.clone();

        let blocking = tokio::task::spawn_blocking(move || {
            evaluate_blocking(query, dataset, format, max_rows, default_graph)
        });

        let payload = match tokio::time::timeout(options.timeout, blocking).await {
            Ok(Ok(Ok(payload))) => payload,
            Ok(Ok(Err(e))) => return Err(e),
            Ok(Err(join)) => return Err(SparqlError::Join(join.to_string())),
            Err(_) => return Err(SparqlError::Timeout),
        };

        Ok(SparqlOutcome {
            payload,
            query_form,
        })
    }
}

/// Result of parsing without executing a query — what the `sbol-db explain`
/// CLI subcommand prints and what HTTP clients can use for client-side
/// validation. Holds the structural classification plus the AST's `Debug`
/// rendering so callers can drill in without pulling in `spargebra`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParsedQuery {
    /// Which of the four read-only forms this is.
    pub form: QueryForm,
    /// Byte length of the input query string.
    pub query_size_bytes: usize,
    /// Debug-formatted AST. Useful for inspection; not a stable format.
    pub ast: String,
}

/// Parse a SPARQL query string without executing it. Rejects `UPDATE`
/// queries the same way `SparqlEngine::execute` does so client-side
/// validation surfaces the same error as the server would.
pub fn parse_query(query_str: &str) -> Result<ParsedQuery, SparqlError> {
    let query = parse_query_strict(query_str)?;
    Ok(ParsedQuery {
        form: classify_query(&query),
        query_size_bytes: query_str.len(),
        ast: format!("{query:#?}"),
    })
}

fn parse_query_strict(query_str: &str) -> Result<spargebra::Query, SparqlError> {
    match SparqlParser::new().parse_query(query_str) {
        Ok(q) => Ok(q),
        Err(e) => {
            // If the same string parses as an Update, the user almost
            // certainly meant a write — surface a clearer error than the
            // generic "unexpected token".
            if SparqlParser::new().parse_update(query_str).is_ok() {
                return Err(SparqlError::UpdateNotAllowed);
            }
            Err(SparqlError::Parse(e.to_string()))
        }
    }
}

fn classify_query(query: &spargebra::Query) -> QueryForm {
    match query {
        spargebra::Query::Select { .. } => QueryForm::Select,
        spargebra::Query::Ask { .. } => QueryForm::Ask,
        spargebra::Query::Construct { .. } => QueryForm::Construct,
        spargebra::Query::Describe { .. } => QueryForm::Describe,
    }
}

fn evaluate_blocking(
    query: spargebra::Query,
    dataset: TripleDataset,
    format: ResultFormat,
    max_rows: usize,
    default_graph: Option<String>,
) -> Result<ResultPayload, SparqlError> {
    let evaluator = QueryEvaluator::new();
    let mut prepared = evaluator.prepare(&query);
    // Dataset selection precedence:
    //   1. The query's own `FROM`/`FROM NAMED` clause wins (honored by
    //      `prepare`); we leave it untouched.
    //   2. Else the protocol `default-graph-uri` scopes the default graph to
    //      that one graph (SynBioHub/Virtuoso semantics).
    //   3. Else the default graph is the union of all named graphs. Our writers
    //      put every triple in a named graph, so without this a plain
    //      `SELECT ?s WHERE { ?s ?p ?o }` would see nothing.
    if query.dataset().is_none() {
        match default_graph {
            Some(g) => prepared
                .dataset_mut()
                .set_default_graph(vec![GraphName::NamedNode(NamedNode::new_unchecked(g))]),
            None => prepared.dataset_mut().set_default_graph_as_union(),
        }
    }
    let results = prepared
        .execute(&dataset)
        .map_err(|e| SparqlError::Evaluation(e.to_string()))?;
    match results {
        QueryResults::Solutions(iter) => serialize_solutions(iter, format, max_rows),
        QueryResults::Boolean(b) => serialize_boolean(b, format),
        QueryResults::Graph(iter) => serialize_triples(iter.into_iter(), format, max_rows),
    }
}
