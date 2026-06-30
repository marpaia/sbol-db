//! Dashboard / graph-browser reads for the lab UI over RocksDB (the
//! [`LabStore`](sbol_db_storage::LabStore) surface).

use std::collections::BTreeMap;

use sbol_db_core::{DomainError, GraphId, GraphRecord};
use sbol_db_rdf::GRAPH_IRI_PREFIX;
use sbol_db_storage::{ClassCount, CorpusCounts, GraphFilter, GraphOverview, GraphTriplesPage};

use crate::codec::Term;
use crate::db::Db;
use crate::repo::triple::TripleRepository;

/// Document graphs all carry this kind; the lab browser filters on it.
const GRAPH_KIND: &str = "sbol3";

#[derive(Clone)]
pub struct LabRepository {
    db: Db,
    triples: TripleRepository,
}

impl LabRepository {
    pub fn new(db: Db) -> Self {
        Self {
            triples: TripleRepository::new(db.clone()),
            db,
        }
    }

    pub fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        Ok(CorpusCounts {
            objects: self.count("objects")?,
            graphs: self.count("graph_meta")?,
            triples: self.count("dspo")? + self.count("spog")?,
            sequences: self.count("seq")?,
            validation_runs: 0,
            ontologies: self.count("ont")?,
        })
    }

    pub fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        if kind.is_some_and(|k| k != GRAPH_KIND) {
            return Ok(0);
        }
        self.count("graph_meta")
    }

    pub fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        if kind.is_some_and(|k| k != GRAPH_KIND) {
            return Ok(Vec::new());
        }
        let mut records = self.all_graph_records()?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        records
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .map(|r| self.to_overview(r))
            .collect()
    }

    pub fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError> {
        match self.graph_record(id)? {
            Some(record) => Ok(Some(self.to_overview(record)?)),
            None => Ok(None),
        }
    }

    pub fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        let Some(_) = self.graph_record(id)? else {
            return Ok(None);
        };
        let iri = graph_iri(id);
        let gid = Term::named(&iri).id();
        let total = self.count_prefix("gspo", &gid)?;
        let want = offset.max(0).saturating_add(limit.max(0));
        let page = self
            .triples
            .scan_pattern(None, None, None, Some(&GraphFilter::Iri(iri)), want)?
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect();
        Ok(Some(GraphTriplesPage {
            total,
            triples: page,
        }))
    }

    pub fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        self.db.for_each("objects", |_, blob| {
            let record: sbol_db_core::SbolObjectRecord = serde_json::from_slice(blob)
                .map_err(|e| DomainError::Serialization(e.to_string()))?;
            *counts.entry(record.sbol_class).or_insert(0) += 1;
            Ok(true)
        })?;
        let mut rows: Vec<ClassCount> = counts
            .into_iter()
            .map(|(iri, count)| ClassCount { iri, count })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.iri.cmp(&b.iri)));
        rows.truncate(limit.max(0) as usize);
        Ok(rows)
    }

    fn to_overview(&self, record: GraphRecord) -> Result<GraphOverview, DomainError> {
        let iri = graph_iri(record.id);
        let gid = Term::named(&iri).id();
        let triple_count = self.count_prefix("gspo", &gid)?;
        let object_count = self.count_prefix("obj_by_graph", record.id.0.as_bytes())?;
        Ok(GraphOverview {
            id: record.id,
            iri,
            kind: GRAPH_KIND.to_owned(),
            name: record.name,
            source_uri: record.source_uri,
            serialization_format: Some(record.serialization_format.as_db_str().to_owned()),
            created_at: record.created_at,
            object_count,
            triple_count,
        })
    }

    fn graph_record(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        match self.db.get_cf("graph_meta", id.0.as_bytes())? {
            Some(blob) => Ok(Some(
                serde_json::from_slice(&blob)
                    .map_err(|e| DomainError::Serialization(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    fn all_graph_records(&self) -> Result<Vec<GraphRecord>, DomainError> {
        let mut out = Vec::new();
        self.db.for_each("graph_meta", |_, blob| {
            out.push(
                serde_json::from_slice(blob)
                    .map_err(|e| DomainError::Serialization(e.to_string()))?,
            );
            Ok(true)
        })?;
        Ok(out)
    }

    fn count(&self, cf: &str) -> Result<i64, DomainError> {
        let mut n = 0i64;
        self.db.for_each(cf, |_, _| {
            n += 1;
            Ok(true)
        })?;
        Ok(n)
    }

    fn count_prefix(&self, cf: &str, prefix: &[u8]) -> Result<i64, DomainError> {
        let mut n = 0i64;
        self.db.for_each_prefix(cf, prefix, |_, _| {
            n += 1;
            Ok(true)
        })?;
        Ok(n)
    }
}

fn graph_iri(id: GraphId) -> String {
    format!("{GRAPH_IRI_PREFIX}{}", id.0)
}
