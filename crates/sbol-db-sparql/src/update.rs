//! SPARQL 1.1 Update execution over the verbatim triplestore.
//!
//! Read queries go through [`crate::SparqlEngine`]; writes go here. SynBioHub
//! issues updates against Virtuoso's authenticated endpoint (the update string
//! arrives in the `query=` parameter), e.g. `DELETE WHERE {...}`, compound
//! `;`-separated operations, and `INSERT DATA {...}`.
//!
//! `spareval` is query-only, so we drive its [`QueryEvaluator::prepare_delete_insert`]
//! helper to evaluate a `DELETE/INSERT ... WHERE` and yield fully instantiated
//! delete/insert triples, then apply them through [`TripleRepository`]. `INSERT DATA`
//! and `DELETE DATA` carry ground triples and skip evaluation entirely.
//!
//! Writes are verbatim (`source = "sparql-update"`, no SBOL interpretation) and
//! land in the graph named by `default-graph-uri` when a template has no
//! explicit `GRAPH`/`WITH` (Virtuoso semantics, which SynBioHub relies on).
//!
//! Transaction model: all operations of one update commit atomically in a
//! single transaction. KNOWN LIMITATION (revisit in Phase 3): `WHERE` clauses
//! are evaluated against the committed snapshot, so a later operation does not
//! observe the uncommitted effects of an earlier one in the same request. This
//! is correct for SynBioHub's updates (their compound operations are
//! independent) but is not full SPARQL sequential-visibility semantics.

use std::sync::Arc;

use oxrdf::{GraphName, NamedNode, NamedOrBlankNode, Term};
use sbol_db_core::{DomainError, IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_postgres::{PgPool, TripleRepository};
use spareval::{DeleteInsertQuad, QueryEvaluator};
use spargebra::algebra::{GraphPattern, GraphTarget, QueryDataset};
use spargebra::term::{GraphName as AstGraphName, GroundQuad, GroundTerm, Quad as AstQuad};
use spargebra::{GraphUpdateOperation, SparqlParser};

use crate::dataset::PostgresDataset;
use crate::engine::SparqlOptions;
use crate::error::SparqlError;

const UPDATE_SOURCE: &str = "sparql-update";

/// Executes SPARQL 1.1 Update against the triplestore.
#[derive(Clone)]
pub struct SparqlUpdateEngine {
    triples: Arc<TripleRepository>,
    pool: PgPool,
}

/// Tally of what an update changed.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct UpdateOutcome {
    pub inserted: usize,
    pub deleted: usize,
}

/// One resolved operation, ready to apply inside the transaction.
enum Step {
    Change {
        deletes: Vec<Triple>,
        inserts: Vec<Triple>,
    },
    /// Clear a graph: `Some(iri)` is a named graph, `None` the default graph.
    Clear(Option<IriString>),
}

impl SparqlUpdateEngine {
    pub fn new(triples: Arc<TripleRepository>, pool: PgPool) -> Self {
        Self { triples, pool }
    }

    /// Parse and execute a SPARQL Update. `default_graph` is the
    /// `default-graph-uri` the client supplied (the graph that template
    /// operations without an explicit `GRAPH` target).
    pub async fn execute(
        &self,
        update_str: &str,
        default_graph: Option<&str>,
        options: &SparqlOptions,
    ) -> Result<UpdateOutcome, SparqlError> {
        if update_str.len() > options.max_query_size {
            return Err(SparqlError::QueryTooLarge);
        }
        let update = SparqlParser::new()
            .parse_update(update_str)
            .map_err(|e| SparqlError::Parse(e.to_string()))?;

        // Resolve every operation to a concrete Step first (DELETE/INSERT WHERE
        // needs the evaluator), then apply atomically.
        let mut steps = Vec::with_capacity(update.operations.len());
        for op in update.operations {
            match op {
                GraphUpdateOperation::InsertData { data } => {
                    let inserts = data
                        .iter()
                        .map(|q| ast_to_triple(q, default_graph))
                        .collect();
                    steps.push(Step::Change {
                        deletes: Vec::new(),
                        inserts,
                    });
                }
                GraphUpdateOperation::DeleteData { data } => {
                    let deletes = data
                        .iter()
                        .map(|q| ground_to_triple(q, default_graph))
                        .collect();
                    steps.push(Step::Change {
                        deletes,
                        inserts: Vec::new(),
                    });
                }
                GraphUpdateOperation::DeleteInsert {
                    delete,
                    insert,
                    using,
                    pattern,
                } => {
                    let (deletes, inserts) = self
                        .eval_delete_insert(delete, insert, using, *pattern, default_graph, options)
                        .await?;
                    steps.push(Step::Change { deletes, inserts });
                }
                GraphUpdateOperation::Clear { graph, .. }
                | GraphUpdateOperation::Drop { graph, .. } => {
                    steps.push(Step::Clear(clear_target(&graph, default_graph)?));
                }
                // Graphs are implicit in our store (a graph exists iff it has a
                // triple), so CREATE is a no-op.
                GraphUpdateOperation::Create { .. } => {}
                GraphUpdateOperation::Load { .. } => {
                    return Err(SparqlError::Unsupported("LOAD".to_owned()));
                }
            }
        }

        let mut outcome = UpdateOutcome::default();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        for step in &steps {
            match step {
                Step::Change { deletes, inserts } => {
                    outcome.deleted += self.triples.delete_triples(&mut tx, deletes).await?;
                    // Register any named graph these inserts target before
                    // writing: a triple's named graph owns it (FK), so the graph
                    // row must exist first.
                    let mut ensured = std::collections::HashSet::new();
                    for triple in inserts {
                        if let Some(graph) = &triple.graph_iri {
                            if ensured.insert(graph.as_str().to_owned()) {
                                self.triples
                                    .ensure_graph(&mut tx, graph.as_str(), "verbatim")
                                    .await?;
                            }
                        }
                    }
                    outcome.inserted += self
                        .triples
                        .insert_triples(&mut tx, inserts, UPDATE_SOURCE)
                        .await?;
                }
                Step::Clear(graph) => {
                    outcome.deleted += self
                        .triples
                        .clear_graph(&mut tx, graph.as_ref().map(|i| i.as_str()))
                        .await?;
                }
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok(outcome)
    }

    /// Evaluate a `DELETE/INSERT ... WHERE` to its concrete delete/insert triples.
    ///
    /// Runs inside `spawn_blocking` because [`PostgresDataset`]'s sync iterators
    /// `block_on` per-pattern fetches (same constraint as the read engine), and
    /// is bounded by the configured timeout.
    async fn eval_delete_insert(
        &self,
        delete: Vec<spargebra::term::GroundQuadPattern>,
        insert: Vec<spargebra::term::QuadPattern>,
        using: Option<QueryDataset>,
        pattern: GraphPattern,
        default_graph: Option<&str>,
        options: &SparqlOptions,
    ) -> Result<(Vec<Triple>, Vec<Triple>), SparqlError> {
        let dataset = PostgresDataset::new(Arc::clone(&self.triples));
        let default_graph = default_graph.map(|s| s.to_owned());

        let blocking = tokio::task::spawn_blocking(move || {
            let evaluator = QueryEvaluator::new();
            let mut prepared =
                evaluator.prepare_delete_insert(delete, insert, None, using, &pattern);
            // Match SynBioHub/Virtuoso semantics: the `default-graph-uri` is the
            // default graph the WHERE clause queries; absent it, fall back to
            // the read engine's union-of-named-graphs behavior.
            match &default_graph {
                Some(g) => prepared
                    .dataset_mut()
                    .set_default_graph(vec![GraphName::NamedNode(NamedNode::new_unchecked(
                        g.clone(),
                    ))]),
                None => prepared.dataset_mut().set_default_graph_as_union(),
            }
            let iter = prepared
                .execute(&dataset)
                .map_err(|e| SparqlError::Evaluation(e.to_string()))?;

            let mut deletes = Vec::new();
            let mut inserts = Vec::new();
            for item in iter {
                match item.map_err(|e| SparqlError::Evaluation(e.to_string()))? {
                    DeleteInsertQuad::Delete(q) => {
                        deletes.push(ox_to_triple(&q, default_graph.as_deref())?)
                    }
                    DeleteInsertQuad::Insert(q) => {
                        inserts.push(ox_to_triple(&q, default_graph.as_deref())?)
                    }
                }
            }
            Ok::<_, SparqlError>((deletes, inserts))
        });

        match tokio::time::timeout(options.timeout, blocking).await {
            Ok(Ok(Ok(pair))) => Ok(pair),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(join)) => Err(SparqlError::Join(join.to_string())),
            Err(_) => Err(SparqlError::Timeout),
        }
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> SparqlError {
    SparqlError::Domain(DomainError::Database(e.to_string()))
}

fn iri(s: &str) -> IriString {
    IriString::unchecked(s)
}

fn subject_to_domain(s: &NamedOrBlankNode) -> SubjectTerm {
    match s {
        NamedOrBlankNode::NamedNode(n) => SubjectTerm::Iri(iri(n.as_str())),
        NamedOrBlankNode::BlankNode(b) => SubjectTerm::BlankNode(b.as_str().to_owned()),
    }
}

fn term_to_object(t: &Term) -> ObjectTerm {
    match t {
        Term::NamedNode(n) => ObjectTerm::Iri(iri(n.as_str())),
        Term::BlankNode(b) => ObjectTerm::BlankNode(b.as_str().to_owned()),
        Term::Literal(l) => ObjectTerm::Literal {
            value: l.value().to_owned(),
            datatype: iri(l.datatype().as_str()),
            language: l.language().map(|s| s.to_owned()),
        },
    }
}

fn ground_term_to_object(t: &GroundTerm) -> ObjectTerm {
    match t {
        GroundTerm::NamedNode(n) => ObjectTerm::Iri(iri(n.as_str())),
        GroundTerm::Literal(l) => ObjectTerm::Literal {
            value: l.value().to_owned(),
            datatype: iri(l.datatype().as_str()),
            language: l.language().map(|s| s.to_owned()),
        },
    }
}

/// Resolve the spargebra (parse-time) graph name for ground triples. `DefaultGraph`
/// maps to the request `default-graph-uri` (or the true default if absent).
fn ast_graph_to_domain(g: &AstGraphName, default_graph: Option<&str>) -> Option<IriString> {
    match g {
        AstGraphName::NamedNode(n) => Some(iri(n.as_str())),
        AstGraphName::DefaultGraph => default_graph.map(iri),
    }
}

/// Resolve the oxrdf graph name produced by evaluation. Blank-node graph names
/// are not representable in our store.
fn ox_graph_to_domain(
    g: &GraphName,
    default_graph: Option<&str>,
) -> Result<Option<IriString>, SparqlError> {
    match g {
        GraphName::NamedNode(n) => Ok(Some(iri(n.as_str()))),
        GraphName::DefaultGraph => Ok(default_graph.map(iri)),
        GraphName::BlankNode(_) => {
            Err(SparqlError::Unsupported("blank node graph name".to_owned()))
        }
    }
}

fn ast_to_triple(q: &AstQuad, default_graph: Option<&str>) -> Triple {
    Triple {
        graph_iri: ast_graph_to_domain(&q.graph_name, default_graph),
        subject: subject_to_domain(&q.subject),
        predicate: iri(q.predicate.as_str()),
        object: term_to_object(&q.object),
    }
}

fn ground_to_triple(q: &GroundQuad, default_graph: Option<&str>) -> Triple {
    Triple {
        graph_iri: ast_graph_to_domain(&q.graph_name, default_graph),
        subject: SubjectTerm::Iri(iri(q.subject.as_str())),
        predicate: iri(q.predicate.as_str()),
        object: ground_term_to_object(&q.object),
    }
}

fn ox_to_triple(q: &oxrdf::Quad, default_graph: Option<&str>) -> Result<Triple, SparqlError> {
    Ok(Triple {
        graph_iri: ox_graph_to_domain(&q.graph_name, default_graph)?,
        subject: subject_to_domain(&q.subject),
        predicate: iri(q.predicate.as_str()),
        object: term_to_object(&q.object),
    })
}

/// Resolve a `CLEAR`/`DROP` target to the graph to clear. Whole-store targets
/// (`NamedGraphs`/`AllGraphs`) are refused rather than risk wiping unrelated
/// data; SynBioHub does not use them.
fn clear_target(
    target: &GraphTarget,
    default_graph: Option<&str>,
) -> Result<Option<IriString>, SparqlError> {
    match target {
        GraphTarget::NamedNode(n) => Ok(Some(iri(n.as_str()))),
        GraphTarget::DefaultGraph => Ok(default_graph.map(iri)),
        GraphTarget::NamedGraphs | GraphTarget::AllGraphs => Err(SparqlError::Unsupported(
            "CLEAR/DROP of all graphs".to_owned(),
        )),
    }
}
