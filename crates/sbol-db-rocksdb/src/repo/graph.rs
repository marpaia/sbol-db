//! The document-graph registry over RocksDB.
//!
//! `graph_meta` maps a [`GraphId`] to its record; `graph_hash` maps a content
//! hash to its graph id so re-imports can be detected. Listing scans the small
//! registry and sorts in memory (the corpus has one row per imported document).

use chrono::Utc;
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, GraphId, GraphRecord, NewGraph};
use sbol_db_storage::ListGraphsFilter;

use crate::db::Db;

#[derive(Clone)]
pub struct GraphRepository {
    db: Db,
}

impl GraphRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Build the registry record for a freshly imported graph and stage its
    /// registry + content-hash writes.
    pub fn stage_insert(
        &self,
        batch: &mut WriteBatch,
        id: GraphId,
        input: &NewGraph,
    ) -> Result<GraphRecord, DomainError> {
        let now = Utc::now();
        let record = GraphRecord {
            id,
            document_iri: input.document_iri.clone(),
            name: input.name.clone(),
            description: input.description.clone(),
            serialization_format: input.serialization_format,
            source_uri: input.source_uri.clone(),
            content_hash: input.content_hash.clone(),
            created_at: now,
            updated_at: now,
        };
        let blob = encode(&record)?;
        batch.put_cf(&self.db.cf("graph_meta"), id.0.as_bytes(), blob);
        batch.put_cf(
            &self.db.cf("graph_hash"),
            &record.content_hash,
            id.0.as_bytes(),
        );
        Ok(record)
    }

    pub fn get(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        match self.db.get_cf("graph_meta", id.0.as_bytes())? {
            Some(blob) => Ok(Some(decode(&blob)?)),
            None => Ok(None),
        }
    }

    pub fn exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        self.db.exists_cf("graph_hash", hash)
    }

    pub fn list(&self, filter: &ListGraphsFilter) -> Result<Vec<GraphRecord>, DomainError> {
        let name = filter.name.as_ref().map(|n| n.to_lowercase());
        let mut out = Vec::new();
        self.db.for_each("graph_meta", |_, blob| {
            let record = decode(blob)?;
            let name_ok = name.as_ref().is_none_or(|n| {
                record
                    .name
                    .as_deref()
                    .is_some_and(|rn| rn.to_lowercase().contains(n))
            });
            let format_ok = filter
                .format
                .is_none_or(|f| record.serialization_format == f);
            if name_ok && format_ok {
                out.push(record);
            }
            Ok(true)
        })?;
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        out.truncate(filter.limit as usize);
        Ok(out)
    }

    /// Stage deletion of a graph's registry + content-hash entries. Returns the
    /// record (so the caller can cascade its triples and objects), or `None`
    /// if the graph does not exist.
    pub fn stage_delete(
        &self,
        batch: &mut WriteBatch,
        id: GraphId,
    ) -> Result<Option<GraphRecord>, DomainError> {
        let Some(record) = self.get(id)? else {
            return Ok(None);
        };
        batch.delete_cf(&self.db.cf("graph_meta"), id.0.as_bytes());
        batch.delete_cf(&self.db.cf("graph_hash"), &record.content_hash);
        Ok(Some(record))
    }
}

fn encode(record: &GraphRecord) -> Result<Vec<u8>, DomainError> {
    serde_json::to_vec(record).map_err(|e| DomainError::Serialization(e.to_string()))
}

fn decode(blob: &[u8]) -> Result<GraphRecord, DomainError> {
    serde_json::from_slice(blob).map_err(|e| DomainError::Serialization(e.to_string()))
}
