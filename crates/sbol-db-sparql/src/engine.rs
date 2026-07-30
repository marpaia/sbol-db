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

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use oxrdf::{GraphName, NamedNode, NamedOrBlankNode};
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
    /// The set of named graphs the caller is authorized to read. This is the
    /// server-enforced ceiling: whatever a query names through `FROM` or the
    /// protocol `default-graph-uri` is intersected with this set, so naming an
    /// unauthorized graph yields no rows rather than its contents.
    pub authorized_graphs: GraphScope,
}

/// The named graphs a caller is authorized to read.
///
/// [`GraphScope::Union`] imposes no restriction: the default graph is the
/// union of all named graphs (sbol-db's native behavior) and any client
/// `FROM`/`default-graph-uri` is honored as-is. [`GraphScope::Only`] caps the
/// queryable graphs to the listed set; the engine intersects the query's
/// requested graphs with it, so a request for a graph outside the set yields
/// no rows from that graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GraphScope {
    #[default]
    Union,
    Only(Vec<String>),
}

impl Default for SparqlOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_rows: 100_000,
            max_query_size: 64 * 1024,
            authorized_graphs: GraphScope::Union,
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
    /// `default_graph_uri` is the SPARQL-protocol `default-graph-uri`: the
    /// graph a query treats as its default graph when it carries no `FROM`
    /// clause of its own. It is intersected with `options.authorized_graphs`,
    /// so it can narrow reads but never widen them beyond the caller's scope.
    pub async fn execute(
        &self,
        query_str: &str,
        requested_format: Option<ResultFormat>,
        default_graph_uri: Option<&str>,
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
        //
        // Setting `SBOL_DB_ACCEL_DISABLED` to `1`/`true` skips the accelerator
        // entirely, forcing generic evaluation for every query — an escape hatch
        // when a query must bypass the indexes.
        if format.is_solution_format() && !accel_disabled() {
            if let Some(plan) = crate::accel::recognize(&parsed, default_graph_uri) {
                // The accelerator answers from one graph's indexes; honor the
                // authorization ceiling before serving it. An unauthorized
                // graph falls through to generic evaluation, which yields the
                // same empty result under the scoped dataset.
                if graph_authorized(plan.graph(), &options.authorized_graphs) {
                    let source = Arc::clone(&self.source);
                    let accel =
                        tokio::task::spawn_blocking(move || source.run_accelerated(&plan)).await;
                    if let Ok(Ok(Some(solutions))) = accel {
                        let payload =
                            serialize_accel_solutions(solutions, format, options.max_rows)?;
                        return Ok(SparqlOutcome {
                            payload,
                            query_form,
                        });
                    }
                }
            }
        }

        let query = crate::rewrite::optimize(parsed);

        let source = Arc::clone(&self.source);
        let use_ids = source.supports_id_scan();
        let max_rows = options.max_rows;
        let default_graph_uri = default_graph_uri.map(str::to_owned);
        let scope = options.authorized_graphs.clone();

        // An id-native backend joins on term ids and materializes terms only at
        // the edges; otherwise fall back to the term-materializing dataset.
        let blocking = tokio::task::spawn_blocking(move || {
            if use_ids {
                let dataset = IdTripleDataset::new(source);
                evaluate_blocking(query, &dataset, format, max_rows, default_graph_uri, scope)
            } else {
                let dataset = TripleDataset::new(source);
                evaluate_blocking(query, &dataset, format, max_rows, default_graph_uri, scope)
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

/// Whether `SBOL_DB_ACCEL_DISABLED` requests that the accelerator be bypassed so
/// every query is evaluated generically.
fn accel_disabled() -> bool {
    matches!(
        std::env::var("SBOL_DB_ACCEL_DISABLED").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn evaluate_blocking<'a, D>(
    query: spargebra::Query,
    dataset: &'a D,
    format: ResultFormat,
    max_rows: usize,
    default_graph_uri: Option<String>,
    scope: GraphScope,
) -> Result<ResultPayload, SparqlError>
where
    &'a D: QueryableDataset<'a, Error = DomainError>,
{
    let evaluator = QueryEvaluator::new();
    let mut prepared = evaluator.prepare(&query);
    scope_dataset(
        prepared.dataset_mut(),
        query.dataset().is_some(),
        default_graph_uri.as_deref(),
        &scope,
    );
    let results = prepared
        .execute(dataset)
        .map_err(|e| SparqlError::Evaluation(e.to_string()))?;
    match results {
        QueryResults::Solutions(iter) => serialize_solutions(iter, format, max_rows),
        QueryResults::Boolean(b) => serialize_boolean(b, format),
        QueryResults::Graph(iter) => serialize_triples(iter.into_iter(), format, max_rows),
    }
}

/// Whether a single-graph read of `graph` is permitted under `scope`.
fn graph_authorized(graph: &str, scope: &GraphScope) -> bool {
    match scope {
        GraphScope::Union => true,
        GraphScope::Only(allowed) => allowed.iter().any(|g| g == graph),
    }
}

/// Resolve the prepared query's dataset against the protocol `default-graph-uri`
/// and the caller's authorization ceiling.
///
/// Dataset selection precedence under [`GraphScope::Union`]:
///   1. The query's own `FROM`/`FROM NAMED` wins (honored by `prepare`); left
///      untouched.
///   2. Else the protocol `default-graph-uri` scopes the default graph to that
///      one graph (SynBioHub/Virtuoso semantics).
///   3. Else the default graph is the union of all named graphs. Our writers
///      put every triple in a named graph, so without this a plain
///      `SELECT ?s WHERE { ?s ?p ?o }` would see nothing.
///
/// Under [`GraphScope::Only`] the queryable graphs are intersected with the
/// authorized set: the default graph and the available named graphs are both
/// clamped to it, so any graph the query names outside the set contributes no
/// rows.
fn scope_dataset(
    ds: &mut spareval::QueryDatasetSpecification,
    query_has_from: bool,
    default_graph_uri: Option<&str>,
    scope: &GraphScope,
) {
    let named = |g: &str| GraphName::NamedNode(NamedNode::new_unchecked(g.to_owned()));
    match scope {
        GraphScope::Union => {
            if !query_has_from {
                match default_graph_uri {
                    Some(g) => ds.set_default_graph(vec![named(g)]),
                    None => ds.set_default_graph_as_union(),
                }
            }
        }
        GraphScope::Only(allowed) => {
            let allowed_set: HashSet<&str> = allowed.iter().map(String::as_str).collect();
            if query_has_from {
                // Intersect the query's `FROM` default graphs with the set.
                let filtered: Vec<GraphName> = ds
                    .default_graph_graphs()
                    .unwrap_or_default()
                    .iter()
                    .filter(|g| {
                        matches!(g, GraphName::NamedNode(n) if allowed_set.contains(n.as_str()))
                    })
                    .cloned()
                    .collect();
                ds.set_default_graph(filtered);
            } else {
                match default_graph_uri {
                    Some(g) if allowed_set.contains(g) => ds.set_default_graph(vec![named(g)]),
                    Some(_) => ds.set_default_graph(Vec::new()),
                    None => ds.set_default_graph(allowed.iter().map(|g| named(g)).collect()),
                }
            }
            // Clamp the available named graphs to the authorized set,
            // intersecting with any `FROM NAMED` the query supplied.
            let allowed_named: Vec<NamedOrBlankNode> = match ds.available_named_graphs() {
                Some(list) => list
                    .iter()
                    .filter(|n| {
                        matches!(n, NamedOrBlankNode::NamedNode(nn) if allowed_set.contains(nn.as_str()))
                    })
                    .cloned()
                    .collect(),
                None => allowed
                    .iter()
                    .map(|g| NamedOrBlankNode::NamedNode(NamedNode::new_unchecked(g.to_owned())))
                    .collect(),
            };
            ds.set_available_named_graphs(allowed_named);
        }
    }
}
