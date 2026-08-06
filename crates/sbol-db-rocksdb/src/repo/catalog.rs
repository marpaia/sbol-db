//! Transactional universal RDF catalog state for RocksDB.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, GraphId};
use sbol_db_storage::{ClassCount, CorpusStats, CursorPage, NamedGraphQuery, NamedGraphRecord};

use crate::codec::Term;
use crate::db::{Db, SEP};

const STATS_KEY: &[u8] = b"catalog:stats:v1";
const GENERATION_KEY: &[u8] = b"catalog:generation:v1";

#[derive(Clone)]
pub struct CatalogRepository {
    db: Db,
}

pub struct CatalogMutation {
    stats: CorpusStats,
    resources: HashMap<String, u64>,
    sequences: HashMap<String, u64>,
    class_resources: HashMap<(String, String), u64>,
    classes: HashMap<String, u64>,
    known_graphs: HashMap<String, GraphId>,
    changed_resources: HashSet<String>,
    changed_sequences: HashSet<String>,
    changed_class_resources: HashSet<(String, String)>,
    changed_classes: HashSet<String>,
}

impl CatalogRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn begin_mutation(&self) -> Result<CatalogMutation, DomainError> {
        Ok(CatalogMutation {
            stats: self.stats()?,
            resources: HashMap::new(),
            sequences: HashMap::new(),
            class_resources: HashMap::new(),
            classes: HashMap::new(),
            known_graphs: HashMap::new(),
            changed_resources: HashSet::new(),
            changed_sequences: HashSet::new(),
            changed_class_resources: HashSet::new(),
            changed_classes: HashSet::new(),
        })
    }

    pub fn stats(&self) -> Result<CorpusStats, DomainError> {
        self.ensure_ready()?;
        let mut stats: CorpusStats = self
            .db
            .get_cf("meta", STATS_KEY)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(ser_err))
            .transpose()
            .map(|stats| stats.unwrap_or_default())?;
        // Ontologies are operator-managed and normally few; counting their
        // catalog rows avoids coupling ontology writes to an RDF mutation.
        stats.ontologies = 0;
        self.db.for_each("ont", |_, _| {
            stats.ontologies += 1;
            Ok(true)
        })?;
        Ok(stats)
    }

    pub fn generation_ready(&self) -> Result<bool, DomainError> {
        self.db.exists_cf("meta", GENERATION_KEY)
    }

    /// Refuse every universal-catalog read when durable RDF exists but the
    /// versioned projection has not reached its generation commit point.
    pub fn ensure_ready(&self) -> Result<(), DomainError> {
        if !self.generation_ready()? && self.has_catalog_source_data()? {
            return Err(DomainError::Unavailable(
                "the RocksDB RDF catalog is not ready; run `sbol-db db migrate` before serving this database"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn has_catalog_source_data(&self) -> Result<bool, DomainError> {
        for cf in [
            "spog",
            "dspo",
            "acc_meta",
            "graph_meta",
            "verbatim_graph_meta",
            "catalog_graph",
            "catalog_resource_refcount",
        ] {
            let mut found = false;
            self.db.for_each(cf, |_, _| {
                found = true;
                Ok(false)
            })?;
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        self.ensure_ready()?;
        let mut rows = Vec::new();
        self.db.for_each("catalog_class_count", |key, value| {
            let iri = String::from_utf8(key.to_vec())
                .map_err(|_| DomainError::Database("non-utf8 catalog class IRI".into()))?;
            rows.push(ClassCount {
                iri,
                count: decode_count(value)? as i64,
            });
            Ok(true)
        })?;
        rows.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.iri.cmp(&right.iri))
        });
        rows.truncate(limit.max(0) as usize);
        Ok(rows)
    }

    pub fn graph(&self, id: GraphId) -> Result<Option<NamedGraphRecord>, DomainError> {
        self.ensure_ready()?;
        self.db
            .get_cf("catalog_graph", id.0.as_bytes())?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(ser_err))
            .transpose()
    }

    pub fn graph_by_iri(&self, iri: &str) -> Result<Option<NamedGraphRecord>, DomainError> {
        self.ensure_ready()?;
        let Some(id) = self.db.get_cf("catalog_graph_by_iri", iri.as_bytes())? else {
            return Ok(None);
        };
        if id.len() != 16 {
            return Err(DomainError::Database(
                "catalog graph IRI index contains an invalid id".to_owned(),
            ));
        }
        self.graph(GraphId(uuid::Uuid::from_bytes(id.try_into().unwrap())))
    }

    pub fn graphs(
        &self,
        query: &NamedGraphQuery,
    ) -> Result<CursorPage<NamedGraphRecord>, DomainError> {
        self.ensure_ready()?;
        let limit = query.limit.clamp(1, 500) as usize;
        let after = query.after.as_deref();
        let needle = query.text.as_ref().map(|value| value.to_lowercase());
        let mut items = Vec::with_capacity(limit + 1);
        self.db.for_each_prefix_after(
            "catalog_graph_by_iri",
            b"",
            after.map(str::as_bytes),
            |iri, id| {
                let iri = std::str::from_utf8(iri)
                    .map_err(|_| DomainError::Database("non-utf8 catalog graph IRI".into()))?;
                if after.is_some_and(|cursor| iri <= cursor) || id.len() != 16 {
                    return Ok(true);
                }
                let id = GraphId(uuid::Uuid::from_bytes(id.try_into().unwrap()));
                if let Some(record) = self.graph(id)? {
                    let matches = needle.as_ref().is_none_or(|needle| {
                        record.iri.to_lowercase().contains(needle)
                            || record
                                .name
                                .as_ref()
                                .is_some_and(|name| name.to_lowercase().contains(needle))
                    });
                    if matches {
                        items.push(record);
                    }
                }
                Ok(items.len() < limit + 1)
            },
        )?;
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| items.last().map(|record| record.iri.clone()))
            .flatten();
        Ok(CursorPage { items, next_cursor })
    }

    pub fn all_graphs(&self) -> Result<Vec<NamedGraphRecord>, DomainError> {
        let mut rows = Vec::new();
        self.db.for_each("catalog_graph", |_, value| {
            rows.push(serde_json::from_slice(value).map_err(ser_err)?);
            Ok(true)
        })?;
        Ok(rows)
    }

    pub fn stage_ensure_graph(
        &self,
        batch: &mut WriteBatch,
        mutation: &mut CatalogMutation,
        mut record: NamedGraphRecord,
    ) -> Result<GraphId, DomainError> {
        if let Some(id) = mutation.known_graphs.get(&record.iri) {
            record.id = *id;
        } else if let Some(existing) = self.graph_by_iri(&record.iri)? {
            record.id = existing.id;
            record.created_at = existing.created_at;
            mutation.known_graphs.insert(record.iri.clone(), record.id);
        } else {
            mutation.stats.named_graphs += 1;
            mutation.known_graphs.insert(record.iri.clone(), record.id);
        }
        let encoded = serde_json::to_vec(&record).map_err(ser_err)?;
        batch.put_cf(
            &self.db.cf("catalog_graph"),
            record.id.0.as_bytes(),
            encoded,
        );
        batch.put_cf(
            &self.db.cf("catalog_graph_by_iri"),
            record.iri.as_bytes(),
            record.id.0.as_bytes(),
        );
        Ok(record.id)
    }

    pub fn stage_graph_counts(
        &self,
        batch: &mut WriteBatch,
        mutation: &mut CatalogMutation,
        iri: &str,
        triple_count: u64,
        resource_count: u64,
    ) -> Result<(), DomainError> {
        let record = self.graph_by_iri(iri)?.unwrap_or_else(|| NamedGraphRecord {
            id: GraphId(uuid::Uuid::from_bytes(Term::named(iri).id())),
            iri: iri.to_owned(),
            name: None,
            description: None,
            source_uri: None,
            serialization_format: None,
            triple_count: None,
            resource_count: None,
            created_at: Some(Utc::now()),
            updated_at: None,
        });
        self.stage_ensure_graph(
            batch,
            mutation,
            NamedGraphRecord {
                triple_count: Some(triple_count),
                resource_count: Some(resource_count),
                updated_at: Some(Utc::now()),
                ..record
            },
        )?;
        Ok(())
    }

    pub fn stage_delete_graph(
        &self,
        batch: &mut WriteBatch,
        mutation: &mut CatalogMutation,
        iri: &str,
    ) -> Result<(), DomainError> {
        let record = mutation
            .known_graphs
            .get(iri)
            .and_then(|id| self.graph(*id).ok().flatten())
            .or(self.graph_by_iri(iri)?);
        if let Some(record) = record {
            batch.delete_cf(&self.db.cf("catalog_graph"), record.id.0.as_bytes());
            batch.delete_cf(&self.db.cf("catalog_graph_by_iri"), iri.as_bytes());
            mutation.known_graphs.remove(iri);
            mutation.stats.named_graphs = mutation.stats.named_graphs.saturating_sub(1);
        }
        Ok(())
    }

    pub fn stage_triple_delta(&self, mutation: &mut CatalogMutation, delta: i64) {
        mutation.stats.triples = add_signed(mutation.stats.triples, delta);
    }

    pub fn stage_projection_delta(
        &self,
        mutation: &mut CatalogMutation,
        old_resources: &HashSet<String>,
        new_resources: &HashSet<String>,
        old_sequences: &HashSet<String>,
        new_sequences: &HashSet<String>,
        old_types: &HashMap<String, HashSet<String>>,
        new_types: &HashMap<String, HashSet<String>>,
    ) -> Result<(), DomainError> {
        for iri in new_resources.difference(old_resources) {
            self.adjust_resource(mutation, iri, 1)?;
        }
        for iri in old_resources.difference(new_resources) {
            self.adjust_resource(mutation, iri, -1)?;
        }
        for iri in new_sequences.difference(old_sequences) {
            self.adjust_sequence(mutation, iri, 1)?;
        }
        for iri in old_sequences.difference(new_sequences) {
            self.adjust_sequence(mutation, iri, -1)?;
        }
        let resources: HashSet<&String> = old_types.keys().chain(new_types.keys()).collect();
        for iri in resources {
            let empty = HashSet::new();
            let old = old_types.get(iri).unwrap_or(&empty);
            let new = new_types.get(iri).unwrap_or(&empty);
            for class in new.difference(old) {
                self.adjust_class(mutation, iri, class, 1)?;
            }
            for class in old.difference(new) {
                self.adjust_class(mutation, iri, class, -1)?;
            }
        }
        Ok(())
    }

    fn adjust_resource(
        &self,
        mutation: &mut CatalogMutation,
        iri: &str,
        delta: i64,
    ) -> Result<(), DomainError> {
        let old = load_count(
            &self.db,
            "catalog_resource_refcount",
            &mut mutation.resources,
            iri,
        )?;
        let new = add_signed(old, delta);
        mutation.resources.insert(iri.to_owned(), new);
        mutation.changed_resources.insert(iri.to_owned());
        if old == 0 && new > 0 {
            mutation.stats.resources += 1;
        } else if old > 0 && new == 0 {
            mutation.stats.resources = mutation.stats.resources.saturating_sub(1);
        }
        Ok(())
    }

    fn adjust_sequence(
        &self,
        mutation: &mut CatalogMutation,
        iri: &str,
        delta: i64,
    ) -> Result<(), DomainError> {
        let old = load_count(
            &self.db,
            "catalog_sequence_refcount",
            &mut mutation.sequences,
            iri,
        )?;
        let new = add_signed(old, delta);
        mutation.sequences.insert(iri.to_owned(), new);
        mutation.changed_sequences.insert(iri.to_owned());
        if old == 0 && new > 0 {
            mutation.stats.sequences += 1;
        } else if old > 0 && new == 0 {
            mutation.stats.sequences = mutation.stats.sequences.saturating_sub(1);
        }
        Ok(())
    }

    fn adjust_class(
        &self,
        mutation: &mut CatalogMutation,
        iri: &str,
        class: &str,
        delta: i64,
    ) -> Result<(), DomainError> {
        let pair = (iri.to_owned(), class.to_owned());
        let old = if let Some(value) = mutation.class_resources.get(&pair) {
            *value
        } else {
            let value = self
                .db
                .get_cf(
                    "catalog_class_resource_refcount",
                    &class_resource_key(iri, class),
                )?
                .map(|bytes| decode_count(&bytes))
                .transpose()?
                .unwrap_or(0);
            mutation.class_resources.insert(pair.clone(), value);
            value
        };
        let new = add_signed(old, delta);
        mutation.class_resources.insert(pair.clone(), new);
        mutation.changed_class_resources.insert(pair);
        if old == 0 && new > 0 {
            self.adjust_class_total(mutation, class, 1)?;
        } else if old > 0 && new == 0 {
            self.adjust_class_total(mutation, class, -1)?;
        }
        Ok(())
    }

    fn adjust_class_total(
        &self,
        mutation: &mut CatalogMutation,
        class: &str,
        delta: i64,
    ) -> Result<(), DomainError> {
        let old = if let Some(value) = mutation.classes.get(class) {
            *value
        } else {
            let value = self
                .db
                .get_cf("catalog_class_count", class.as_bytes())?
                .map(|bytes| decode_count(&bytes))
                .transpose()?
                .unwrap_or(0);
            mutation.classes.insert(class.to_owned(), value);
            value
        };
        mutation
            .classes
            .insert(class.to_owned(), add_signed(old, delta));
        mutation.changed_classes.insert(class.to_owned());
        Ok(())
    }

    pub fn finish(
        &self,
        batch: &mut WriteBatch,
        mutation: CatalogMutation,
    ) -> Result<(), DomainError> {
        for iri in mutation.changed_resources {
            stage_count(
                &self.db,
                batch,
                "catalog_resource_refcount",
                &iri,
                mutation.resources[&iri],
            );
        }
        for iri in mutation.changed_sequences {
            stage_count(
                &self.db,
                batch,
                "catalog_sequence_refcount",
                &iri,
                mutation.sequences[&iri],
            );
        }
        for pair in mutation.changed_class_resources {
            stage_raw_count(
                &self.db,
                batch,
                "catalog_class_resource_refcount",
                &class_resource_key(&pair.0, &pair.1),
                mutation.class_resources[&pair],
            );
        }
        for class in mutation.changed_classes {
            stage_raw_count(
                &self.db,
                batch,
                "catalog_class_count",
                class.as_bytes(),
                mutation.classes[&class],
            );
        }
        batch.put_cf(
            &self.db.cf("meta"),
            STATS_KEY,
            serde_json::to_vec(&mutation.stats).map_err(ser_err)?,
        );
        batch.put_cf(&self.db.cf("meta"), GENERATION_KEY, b"1");
        Ok(())
    }

    pub fn seed_stats(
        &self,
        batch: &mut WriteBatch,
        stats: &CorpusStats,
    ) -> Result<(), DomainError> {
        batch.put_cf(
            &self.db.cf("meta"),
            STATS_KEY,
            serde_json::to_vec(stats).map_err(ser_err)?,
        );
        batch.put_cf(&self.db.cf("meta"), GENERATION_KEY, b"1");
        Ok(())
    }
}

fn load_count(
    db: &Db,
    cf: &str,
    cache: &mut HashMap<String, u64>,
    iri: &str,
) -> Result<u64, DomainError> {
    if let Some(value) = cache.get(iri) {
        return Ok(*value);
    }
    let count = db
        .get_cf(cf, iri.as_bytes())?
        .map(|bytes| {
            (bytes.len() == 8)
                .then(|| u64::from_le_bytes(bytes.try_into().unwrap()))
                .ok_or_else(|| DomainError::Database("invalid catalog refcount".to_owned()))
        })
        .transpose()?
        .unwrap_or(0);
    cache.insert(iri.to_owned(), count);
    Ok(count)
}

fn stage_count(db: &Db, batch: &mut WriteBatch, cf: &str, iri: &str, count: u64) {
    stage_raw_count(db, batch, cf, iri.as_bytes(), count);
}

fn stage_raw_count(db: &Db, batch: &mut WriteBatch, cf: &str, key: &[u8], count: u64) {
    if count == 0 {
        batch.delete_cf(&db.cf(cf), key);
    } else {
        batch.put_cf(&db.cf(cf), key, count.to_le_bytes());
    }
}

fn class_resource_key(iri: &str, class: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(iri.len() + class.len() + 1);
    key.extend_from_slice(iri.as_bytes());
    key.push(SEP);
    key.extend_from_slice(class.as_bytes());
    key
}

fn decode_count(bytes: &[u8]) -> Result<u64, DomainError> {
    (bytes.len() == 8)
        .then(|| u64::from_le_bytes(bytes.try_into().unwrap()))
        .ok_or_else(|| DomainError::Database("invalid catalog count".to_owned()))
}

fn add_signed(value: u64, delta: i64) -> u64 {
    if delta >= 0 {
        value.saturating_add(delta as u64)
    } else {
        value.saturating_sub(delta.unsigned_abs())
    }
}

fn ser_err(error: serde_json::Error) -> DomainError {
    DomainError::Serialization(error.to_string())
}
