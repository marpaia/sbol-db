//! The derived object view over RocksDB.
//!
//! Objects are keyed by IRI so the column family is already in the order
//! `list` wants (lexicographic IRI), giving keyset pagination by `after_iri`
//! for free. Two secondary families resolve an object by id and enumerate a
//! graph's objects (for graph-scoped listing and cascade delete).

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, GraphId, ObjectId, ObjectSummary, SbolObjectRecord};
use sbol_db_storage::ListObjectsFilter;

use crate::db::Db;

const LIST_LIMIT_MAX: u32 = 5000;

#[derive(Clone)]
pub struct ObjectRepository {
    db: Db,
}

impl ObjectRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Stage an upsert. The object's id is minted on first insert and preserved
    /// across updates (matching the `ON CONFLICT(iri)` behavior of the SQL
    /// backends); a graph change rewrites the graph membership entry.
    pub fn stage_upsert(
        &self,
        batch: &mut WriteBatch,
        summary: &ObjectSummary,
        graph_id: Option<GraphId>,
    ) -> Result<(), DomainError> {
        let iri = summary.iri.as_str();
        let existing = self.get_by_iri(iri)?;
        let id = existing
            .as_ref()
            .map(|r| r.id)
            .unwrap_or_else(ObjectId::new);

        // Drop a stale graph-membership entry when the owning graph changes.
        if let Some(prev) = &existing {
            if prev.graph_id != graph_id {
                if let Some(g) = prev.graph_id {
                    batch.delete_cf(&self.db.cf("obj_by_graph"), graph_member_key(g, iri));
                }
            }
        }

        let record = SbolObjectRecord {
            id,
            iri: summary.iri.clone(),
            sbol_class: summary.sbol_class.clone(),
            display_id: summary.display_id.clone(),
            name: summary.name.clone(),
            description: summary.description.clone(),
            graph_id,
            types: summary.types.clone(),
            roles: summary.roles.clone(),
            data: summary.data.clone(),
            content_hash: summary.content_hash.clone(),
        };

        let blob =
            serde_json::to_vec(&record).map_err(|e| DomainError::Serialization(e.to_string()))?;
        batch.put_cf(&self.db.cf("objects"), iri.as_bytes(), blob);
        batch.put_cf(&self.db.cf("obj_by_id"), id.0.as_bytes(), iri.as_bytes());
        if let Some(g) = graph_id {
            batch.put_cf(&self.db.cf("obj_by_graph"), graph_member_key(g, iri), []);
        }
        Ok(())
    }

    /// Stage deletion of every object owned by a graph, used by cascade delete.
    pub fn stage_delete_for_graph(
        &self,
        batch: &mut WriteBatch,
        graph_id: GraphId,
    ) -> Result<(), DomainError> {
        let prefix = graph_id.0.as_bytes().to_vec();
        let mut victims: Vec<String> = Vec::new();
        self.db.for_each_prefix("obj_by_graph", &prefix, |key, _| {
            // key = graph-uuid (16 bytes) + iri
            let iri = String::from_utf8(key[16..].to_vec())
                .map_err(|_| DomainError::Database("non-utf8 object iri".into()))?;
            victims.push(iri);
            Ok(true)
        })?;
        for iri in &victims {
            if let Some(record) = self.get_by_iri(iri)? {
                batch.delete_cf(&self.db.cf("obj_by_id"), record.id.0.as_bytes());
            }
            batch.delete_cf(&self.db.cf("objects"), iri.as_bytes());
            batch.delete_cf(&self.db.cf("obj_by_graph"), graph_member_key(graph_id, iri));
        }
        Ok(())
    }

    pub fn get_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        match self.db.get_cf("objects", iri.as_bytes())? {
            Some(blob) => Ok(Some(decode(&blob)?)),
            None => Ok(None),
        }
    }

    pub fn get_by_iris(&self, iris: &[&str]) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let mut out = Vec::new();
        for iri in iris {
            if let Some(record) = self.get_by_iri(iri)? {
                out.push(record);
            }
        }
        Ok(out)
    }

    pub fn get_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        match self.db.get_cf("obj_by_id", id.0.as_bytes())? {
            Some(bytes) => {
                Ok(Some(String::from_utf8(bytes).map_err(|_| {
                    DomainError::Database("non-utf8 object iri".into())
                })?))
            }
            None => Ok(None),
        }
    }

    pub fn list(&self, filter: &ListObjectsFilter) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let limit = filter.limit.clamp(1, LIST_LIMIT_MAX) as usize;
        let after = filter.after_iri.as_deref();
        let mut out = Vec::new();

        let matches = |r: &SbolObjectRecord| {
            filter
                .sbol_class
                .as_ref()
                .is_none_or(|c| &r.sbol_class == c)
                && filter
                    .role
                    .as_ref()
                    .is_none_or(|role| r.roles.contains(role))
        };

        match filter.graph_id {
            // Graph-scoped: obj_by_graph already orders by IRI within the graph.
            Some(g) => {
                let prefix = g.0.as_bytes().to_vec();
                self.db.for_each_prefix("obj_by_graph", &prefix, |key, _| {
                    let iri = std::str::from_utf8(&key[16..])
                        .map_err(|_| DomainError::Database("non-utf8 object iri".into()))?;
                    if after.is_some_and(|a| iri <= a) {
                        return Ok(true);
                    }
                    if let Some(record) = self.get_by_iri(iri)? {
                        if matches(&record) {
                            out.push(record);
                        }
                    }
                    Ok(out.len() < limit)
                })?;
            }
            // Whole corpus: the objects family is keyed by IRI in order.
            None => {
                let start: Vec<u8> = after.map(|a| a.as_bytes().to_vec()).unwrap_or_default();
                self.db.for_each_prefix("objects", &start, |key, blob| {
                    let iri = std::str::from_utf8(key)
                        .map_err(|_| DomainError::Database("non-utf8 object iri".into()))?;
                    if after.is_some_and(|a| iri <= a) {
                        return Ok(true);
                    }
                    let record = decode(blob)?;
                    if matches(&record) {
                        out.push(record);
                    }
                    Ok(out.len() < limit)
                })?;
            }
        }
        Ok(out)
    }
}

/// `graph-uuid (16 bytes) ++ iri` — fixed-width prefix gives a clean range
/// scan of one graph's objects, ordered by IRI.
fn graph_member_key(graph_id: GraphId, iri: &str) -> Vec<u8> {
    let mut key = graph_id.0.as_bytes().to_vec();
    key.extend_from_slice(iri.as_bytes());
    key
}

fn decode(blob: &[u8]) -> Result<SbolObjectRecord, DomainError> {
    serde_json::from_slice(blob).map_err(|e| DomainError::Serialization(e.to_string()))
}
