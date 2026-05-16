//! SPARQL query engine — parses, evaluates, and serializes results.
//!
//! The evaluation runs inside `tokio::task::spawn_blocking` so the
//! [`PostgresDataset`]'s sync iterators can call `Handle::current().block_on`
//! to await per-pattern sqlx fetches. The whole spawn_blocking handle is
//! wrapped in `tokio::time::timeout` to bound query time. Sync evaluator code
//! can't be preempted by tokio — past the deadline the task may still run a
//! short while before its next pattern fetch terminates — so the timeout is
//! "best-effort soft cap" rather than a hard kill.

use std::sync::Arc;
use std::time::Duration;

use sbol_db_postgres::QuadRepository;
use spareval::{QueryEvaluator, QueryResults};
use spargebra::SparqlParser;

use crate::dataset::PostgresDataset;
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
}

impl Default for SparqlOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_rows: 100_000,
            max_query_size: 64 * 1024,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    quads: Arc<QuadRepository>,
}

impl SparqlEngine {
    pub fn new(quads: Arc<QuadRepository>) -> Self {
        Self { quads }
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

        let query = parse_query_strict(query_str)?;
        let query_form = classify_query(&query);
        let format = requested_format.unwrap_or_else(|| query_form.default_format());
        if !query_form.allows(format) {
            return Err(SparqlError::UnsupportedFormat(format!(
                "{format:?} is not a valid result format for a {query_form:?} query"
            )));
        }

        let dataset = PostgresDataset::new(Arc::clone(&self.quads));
        let max_rows = options.max_rows;

        let blocking = tokio::task::spawn_blocking(move || {
            evaluate_blocking(query, dataset, format, max_rows)
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
    dataset: PostgresDataset,
    format: ResultFormat,
    max_rows: usize,
) -> Result<ResultPayload, SparqlError> {
    let evaluator = QueryEvaluator::new();
    let mut prepared = evaluator.prepare(&query);
    // Default-graph = union of all named graphs. Our writers put every quad
    // in a named graph (`graph:document:{id}`), so without this a plain
    // `SELECT ?s WHERE { ?s ?p ?o }` would see nothing.
    prepared.dataset_mut().set_default_graph_as_union();
    let results = prepared
        .execute(&dataset)
        .map_err(|e| SparqlError::Evaluation(e.to_string()))?;
    match results {
        QueryResults::Solutions(iter) => serialize_solutions(iter, format, max_rows),
        QueryResults::Boolean(b) => serialize_boolean(b, format),
        QueryResults::Graph(iter) => serialize_triples(iter.into_iter(), format, max_rows),
    }
}
