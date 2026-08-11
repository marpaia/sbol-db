//! The derived object view over RocksDB.
//!
//! Objects are keyed by IRI so the column family is already in the order
//! `list` wants (lexicographic IRI), giving keyset pagination by `after_iri`
//! for free. Two secondary families resolve an object by id and enumerate a
//! graph's objects (for graph-scoped listing and cascade delete).

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, GraphId, ObjectId, ObjectSummary, SbolObjectRecord};
use sbol_db_storage::TextSearchQuery;

use crate::db::Db;

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
        self.stage_upsert_with_id(batch, summary, graph_id, None)
            .map(|_| ())
    }

    pub(crate) fn stage_upsert_with_id(
        &self,
        batch: &mut WriteBatch,
        summary: &ObjectSummary,
        graph_id: Option<GraphId>,
        preferred_id: Option<ObjectId>,
    ) -> Result<ObjectId, DomainError> {
        let iri = summary.iri.as_str();
        let existing = self.get_by_iri(iri)?;
        let id = existing
            .as_ref()
            .map(|r| r.id)
            .or(preferred_id)
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
        Ok(id)
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

    /// Offset-paginated substring search over the object view, scanning the
    /// `objects` family (ordered by IRI) and matching case-insensitively on
    /// `name`/`display_id`/`description`. Returns the page plus the total match
    /// count; a `limit` of 0 returns the count only. `property_uri` search has
    /// no supporting index on this backend and is reported as unavailable.
    pub fn search(
        &self,
        query: &TextSearchQuery,
    ) -> Result<(Vec<SbolObjectRecord>, i64), DomainError> {
        if query.property_uri.is_some() {
            return Err(DomainError::Unavailable(
                "property_uri text search is not supported on the RocksDB backend".into(),
            ));
        }
        let needle = query.text.to_lowercase();
        let limit = query.limit.clamp(0, 1000) as usize;
        let offset = query.offset.max(0) as usize;

        let contains = |field: &Option<String>| {
            field
                .as_ref()
                .is_some_and(|v| v.to_lowercase().contains(&needle))
        };

        let mut total: i64 = 0;
        let mut page = Vec::new();
        self.db.for_each("objects", |_, blob| {
            let record = decode(blob)?;
            let class_ok = query
                .sbol_class
                .as_ref()
                .is_none_or(|c| &record.sbol_class == c);
            let text_ok = contains(&record.name)
                || contains(&record.display_id)
                || contains(&record.description);
            if class_ok && text_ok {
                let idx = total as usize;
                if limit > 0 && idx >= offset && page.len() < limit {
                    page.push(record);
                }
                total += 1;
            }
            Ok(true)
        })?;
        Ok((page, total))
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
