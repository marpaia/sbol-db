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
use sbol_db_core::DomainError;
use sbol_db_storage::TripleSource;
use spareval::{QueryEvaluator, QueryResults, QueryableDataset};
use spargebra::SparqlParser;

use crate::dataset::{IdTripleDataset, TripleDataset};
use crate::error::SparqlError;
use crate::results::{
    serialize_accel_solutions, serialize_boolean, serialize_solutions, serialize_triples,
    ResultFormat, ResultPayload,
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

/// A native SPARQL evaluator owned by a backend that has its own id-native query
/// engine (Oxigraph). When present, recognized queries still short-circuit
/// through the accelerator first; everything else is handed to this evaluator
/// instead of the workspace's term-materializing dataset path, and the
/// `NOT EXISTS`→`MINUS` rewrite is skipped (the native engine evaluates the
/// query as parsed). The method is synchronous and driven inside the engine's
/// `spawn_blocking` task.
pub trait NativeSparql: Send + Sync {
    /// Evaluate a parsed query against the backend store and serialize the
    /// result. `default_graph` is the protocol `default-graph-uri`, applied with
    /// the same precedence as [`evaluate_blocking`].
    fn evaluate(
        &self,
        query: &spargebra::Query,
        format: ResultFormat,
        max_rows: usize,
        default_graph: Option<&str>,
    ) -> Result<ResultPayload, SparqlError>;
}

#[derive(Clone)]
pub struct SparqlEngine {
    source: Arc<dyn TripleSource>,
    native: Option<Arc<dyn NativeSparql>>,
}

impl SparqlEngine {
    pub fn new(source: Arc<dyn TripleSource>) -> Self {
        Self {
            source,
            native: None,
        }
    }

    /// Build an engine backed by a native query evaluator. The accelerator
    /// short-circuit still runs first; unrecognized queries go to `native`.
    pub fn with_native(source: Arc<dyn TripleSource>, native: Arc<dyn NativeSparql>) -> Self {
        Self {
            source,
            native: Some(native),
        }
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

        let parsed = parse_query_strict(query_str)?;
        let query_form = classify_query(&parsed);
        let format = requested_format.unwrap_or_else(|| query_form.default_format());
        if !query_form.allows(format) {
            return Err(SparqlError::UnsupportedFormat(format!(
                "{format:?} is not a valid result format for a {query_form:?} query"
            )));
        }

        // SynBioHub query accelerator: if the query matches a known template and
        // the backend can answer it from its purpose-built indexes, serve it
        // directly. This runs on the original parse, before the NOT EXISTS->MINUS
        // rewrite, so the recognizer still sees the template's shape. Anything not
        // recognized, not supported, or failing falls through to generic
        // evaluation, so results never depend on this path.
        if format.is_solution_format() {
            if let Some(plan) = crate::accel::recognize(&parsed, options.default_graph.as_deref()) {
                let source = Arc::clone(&self.source);
                let accel =
                    tokio::task::spawn_blocking(move || source.run_accelerated(&plan)).await;
                if let Ok(Ok(Some(solutions))) = accel {
                    let payload = serialize_accel_solutions(solutions, format, options.max_rows)?;
                    return Ok(SparqlOutcome {
                        payload,
                        query_form,
                    });
                }
            }
        }

        // A backend with a native id-native engine (Oxigraph) evaluates the
        // query directly, at full strength, without the workspace's
        // `NOT EXISTS`→`MINUS` rewrite (the native engine handles the query as
        // parsed). The accelerator short-circuit above still applies first.
        if let Some(native) = &self.native {
            let native = Arc::clone(native);
            let parsed = parsed.clone();
            let max_rows = options.max_rows;
            let default_graph = options.default_graph.clone();
            let blocking = tokio::task::spawn_blocking(move || {
                native.evaluate(&parsed, format, max_rows, default_graph.as_deref())
            });
            let payload = match tokio::time::timeout(options.timeout, blocking).await {
                Ok(Ok(Ok(payload))) => payload,
                Ok(Ok(Err(e))) => return Err(e),
                Ok(Err(join)) => return Err(SparqlError::Join(join.to_string())),
                Err(_) => return Err(SparqlError::Timeout),
            };
            return Ok(SparqlOutcome {
                payload,
                query_form,
            });
        }

        let query = crate::rewrite::optimize(parsed);

        let source = Arc::clone(&self.source);
        let use_ids = source.supports_id_scan();
        let max_rows = options.max_rows;
        let default_graph = options.default_graph.clone();

        // An id-native backend joins on term ids and materializes terms only at
        // the edges; otherwise fall back to the term-materializing dataset.
        let blocking = tokio::task::spawn_blocking(move || {
            if use_ids {
                let dataset = IdTripleDataset::new(source);
                evaluate_blocking(query, &dataset, format, max_rows, default_graph)
            } else {
                let dataset = TripleDataset::new(source);
                evaluate_blocking(query, &dataset, format, max_rows, default_graph)
            }
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

fn evaluate_blocking<'a, D>(
    query: spargebra::Query,
    dataset: &'a D,
    format: ResultFormat,
    max_rows: usize,
    default_graph: Option<String>,
) -> Result<ResultPayload, SparqlError>
where
    &'a D: QueryableDataset<'a, Error = DomainError>,
{
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
        .execute(dataset)
        .map_err(|e| SparqlError::Evaluation(e.to_string()))?;
    match results {
        QueryResults::Solutions(iter) => serialize_solutions(iter, format, max_rows),
        QueryResults::Boolean(b) => serialize_boolean(b, format),
        QueryResults::Graph(iter) => serialize_triples(iter.into_iter(), format, max_rows),
    }
}
