//! Backend-independent dashboard compatibility surface over the universal
//! RocksDB RDF catalog.

use sbol_db_core::{DomainError, GraphId};
use sbol_db_storage::{ClassCount, CorpusCounts, GraphFilter, GraphOverview, GraphTriplesPage};

use crate::db::Db;
use crate::repo::catalog::CatalogRepository;
use crate::repo::triple::TripleRepository;

#[derive(Clone)]
pub struct LabRepository {
    triples: TripleRepository,
    catalog: CatalogRepository,
}

impl LabRepository {
    pub fn new(db: Db) -> Self {
        Self {
            triples: TripleRepository::new(db.clone()),
            catalog: CatalogRepository::new(db),
        }
    }

    pub fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        let stats = self.catalog.stats()?;
        Ok(CorpusCounts {
            objects: as_i64(stats.resources),
            graphs: as_i64(stats.named_graphs),
            triples: as_i64(stats.triples),
            sequences: as_i64(stats.sequences),
            validation_runs: 0,
            ontologies: as_i64(stats.ontologies),
        })
    }

    pub fn count_graphs(&self, _kind: Option<&str>) -> Result<i64, DomainError> {
        Ok(as_i64(self.catalog.stats()?.named_graphs))
    }

    pub fn list_graph_overviews(
        &self,
        _kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        let mut rows: Vec<GraphOverview> = self
            .catalog
            .all_graphs()?
            .into_iter()
            .map(to_overview)
            .collect();
        rows.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.iri.cmp(&right.iri))
        });
        Ok(rows
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect())
    }

    pub fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError> {
        Ok(self.catalog.graph(id)?.map(to_overview))
    }

    pub fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        let Some(graph) = self.catalog.graph(id)? else {
            return Ok(None);
        };
        let want = offset.max(0).saturating_add(limit.max(0));
        let triples = self
            .triples
            .scan_pattern(None, None, None, Some(&GraphFilter::Iri(graph.iri)), want)?
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect();
        Ok(Some(GraphTriplesPage {
            total: graph.triple_count.map(as_i64),
            triples,
        }))
    }

    pub fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        self.catalog.top_classes(limit)
    }
}

fn to_overview(row: sbol_db_storage::NamedGraphRecord) -> GraphOverview {
    GraphOverview {
        id: row.id,
        iri: row.iri,
        kind: "rdf".to_owned(),
        name: row.name,
        source_uri: row.source_uri,
        serialization_format: row.serialization_format,
        created_at: row.created_at,
        object_count: row.resource_count.map(as_i64),
        triple_count: row.triple_count.map(as_i64),
    }
}

fn as_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
