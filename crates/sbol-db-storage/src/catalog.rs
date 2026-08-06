//! Backend-neutral RDF catalog contracts.
//!
//! Canonical RDF quads are the source of truth. These records are rebuildable
//! projections used by product and administrator APIs; they deliberately do
//! not describe how a document entered the database or which engine stores it.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, GraphId, Triple};
use serde::{Deserialize, Serialize};

use crate::{ClassCount, CorpusCounts, GraphOverview, MetaRecord};

/// One stable keyset page. `next_cursor` is opaque to callers and belongs to
/// the backend that produced it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Exact, backend-independent corpus statistics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusStats {
    pub resources: u64,
    pub named_graphs: u64,
    pub triples: u64,
    pub sequences: u64,
    pub ontologies: u64,
}

impl From<CorpusCounts> for CorpusStats {
    fn from(value: CorpusCounts) -> Self {
        Self {
            resources: value.objects.max(0) as u64,
            named_graphs: value.graphs.max(0) as u64,
            triples: value.triples.max(0) as u64,
            sequences: value.sequences.max(0) as u64,
            ontologies: value.ontologies.max(0) as u64,
        }
    }
}

/// A named graph in the canonical RDF dataset.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamedGraphRecord {
    pub id: GraphId,
    pub iri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub source_uri: Option<String>,
    pub serialization_format: Option<String>,
    pub triple_count: Option<u64>,
    pub resource_count: Option<u64>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl From<GraphOverview> for NamedGraphRecord {
    fn from(value: GraphOverview) -> Self {
        Self {
            id: value.id,
            iri: value.iri,
            name: value.name,
            description: None,
            source_uri: value.source_uri,
            serialization_format: value.serialization_format,
            triple_count: value.triple_count.map(|count| count.max(0) as u64),
            resource_count: value.object_count.map(|count| count.max(0) as u64),
            created_at: value.created_at,
            updated_at: None,
        }
    }
}

/// One resource occurrence in one named graph. Metadata is occurrence-scoped:
/// the same resource IRI may legitimately have different assertions in two
/// graphs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceOccurrence {
    pub graph_iri: String,
    pub resource_iri: String,
    pub meta: MetaRecord,
}

/// Corpus-wide identity for one resource IRI. The metadata is a deterministic
/// representative of its occurrences; `graph_count` reports the exact number
/// of named graphs containing the resource.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub iri: String,
    pub graph_count: u64,
    pub meta: MetaRecord,
}

/// One sequence projected from canonical RDF.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogSequenceRecord {
    pub iri: String,
    pub graph_count: u64,
    pub encoding_iri: Option<String>,
    pub elements: Option<String>,
    pub alphabet: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ResourceQuery {
    pub after: Option<String>,
    pub limit: u32,
    pub text: Option<String>,
    pub class: Option<String>,
    pub role: Option<String>,
    pub graph_iri: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NamedGraphQuery {
    pub after: Option<String>,
    pub limit: u32,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SequenceQuery {
    pub after: Option<String>,
    pub limit: u32,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TriplePageQuery {
    pub after: Option<String>,
    pub limit: u32,
}

#[async_trait]
pub trait ResourceCatalogStore: Send + Sync {
    async fn catalog_resource(&self, iri: &str) -> Result<Option<ResourceRecord>, DomainError>;

    async fn catalog_resource_occurrences(
        &self,
        iri: &str,
    ) -> Result<Vec<ResourceOccurrence>, DomainError>;

    async fn catalog_resources(
        &self,
        query: &ResourceQuery,
    ) -> Result<CursorPage<ResourceRecord>, DomainError>;

    /// Resolve a bounded caller-provided identity list in its original order.
    /// Backends may batch the expansion rather than issuing one query per IRI.
    async fn catalog_resources_by_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<ResourceRecord>, DomainError>;

    async fn catalog_top_classes(&self, limit: u32) -> Result<Vec<ClassCount>, DomainError>;
}

#[async_trait]
pub trait NamedGraphCatalogStore: Send + Sync {
    async fn catalog_graph(&self, id: GraphId) -> Result<Option<NamedGraphRecord>, DomainError>;

    async fn catalog_graphs(
        &self,
        query: &NamedGraphQuery,
    ) -> Result<CursorPage<NamedGraphRecord>, DomainError>;

    async fn catalog_graph_triples(
        &self,
        id: GraphId,
        query: &TriplePageQuery,
    ) -> Result<Option<CursorPage<Triple>>, DomainError>;
}

#[async_trait]
pub trait SequenceCatalogStore: Send + Sync {
    async fn catalog_sequence(
        &self,
        iri: &str,
    ) -> Result<Option<CatalogSequenceRecord>, DomainError>;

    async fn catalog_sequences(
        &self,
        query: &SequenceQuery,
    ) -> Result<CursorPage<CatalogSequenceRecord>, DomainError>;
}

#[async_trait]
pub trait CorpusStatsStore: Send + Sync {
    async fn catalog_stats(&self) -> Result<CorpusStats, DomainError>;
}

/// Fold occurrence-scoped metadata into the deterministic global resource
/// representation used by catalog lists. Graph lexical order selects the
/// representative first values; all multi-valued vocabulary assertions are
/// then unioned without losing information from another occurrence.
pub fn merge_resource_occurrences(
    iri: &str,
    occurrences: &[ResourceOccurrence],
) -> Option<ResourceRecord> {
    let mut ordered = occurrences.to_vec();
    ordered.sort_by(|left, right| left.graph_iri.cmp(&right.graph_iri));
    let first = ordered.first()?;
    let mut meta = first.meta.clone();
    for occurrence in ordered.iter().skip(1) {
        extend_unique(&mut meta.display_id, &occurrence.meta.display_id);
        extend_unique(&mut meta.name, &occurrence.meta.name);
        extend_unique(&mut meta.description, &occurrence.meta.description);
        extend_unique(&mut meta.version, &occurrence.meta.version);
        extend_unique(&mut meta.types, &occurrence.meta.types);
        extend_unique(&mut meta.sbol_types, &occurrence.meta.sbol_types);
        extend_unique(&mut meta.roles, &occurrence.meta.roles);
        extend_unique(&mut meta.creators, &occurrence.meta.creators);
        meta.top_level |= occurrence.meta.top_level;
    }
    Some(ResourceRecord {
        iri: iri.to_owned(),
        graph_count: ordered.len() as u64,
        meta,
    })
}

fn extend_unique<T: Clone + PartialEq>(target: &mut Vec<T>, values: &[T]) {
    for value in values {
        if !target.contains(value) {
            target.push(value.clone());
        }
    }
}
