//! Backend-neutral data for the lab dashboard: corpus counts, graph browsing
//! (all kinds, with object/triple counts), and class breakdowns. Every backend
//! provides this; the lab UI's overview and graph-browser pages read it instead
//! of issuing engine-specific SQL.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, GraphId, Triple};
use serde::Serialize;

/// Row counts across the corpus for the dashboard. A backend reports 0 for a
/// surface it does not materialize (e.g. SQLite has no validation-run table).
#[derive(Clone, Debug, Default, Serialize)]
pub struct CorpusCounts {
    pub objects: i64,
    pub graphs: i64,
    pub triples: i64,
    pub sequences: i64,
    pub validation_runs: i64,
    pub ontologies: i64,
}

/// One graph row for the browser: any kind, with its object and triple counts.
#[derive(Clone, Debug, Serialize)]
pub struct GraphOverview {
    pub id: GraphId,
    pub iri: String,
    pub kind: String,
    pub name: Option<String>,
    pub source_uri: Option<String>,
    pub serialization_format: Option<String>,
    pub created_at: DateTime<Utc>,
    pub object_count: i64,
    pub triple_count: i64,
}

/// One `(sbol_class, count)` bucket for the top-classes breakdown.
#[derive(Clone, Debug, Serialize)]
pub struct ClassCount {
    pub iri: String,
    pub count: i64,
}

/// A page of a graph's triples plus the total count for pagination.
pub struct GraphTriplesPage {
    pub total: i64,
    pub triples: Vec<Triple>,
}

/// Dashboard / graph-browser reads for the lab UI.
#[async_trait]
pub trait LabStore: Send + Sync {
    async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError>;
    async fn recent_graphs(&self, limit: i64) -> Result<Vec<GraphOverview>, DomainError>;
    async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError>;
    async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError>;
    async fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError>;
    async fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError>;
    /// One graph's triples (paginated) plus the total, or `None` if the graph
    /// does not exist.
    async fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError>;
}
