//! Versioned, resumable schema migration for the RocksDB backend.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, GraphId, GraphRecord};
use sbol_db_storage::{CorpusStats, MetaRecord, MigrationEntry, Migrator, NamedGraphRecord};
use serde::Deserialize;

use crate::bulk::GraphCatalogImport;
use crate::codec::Term;
use crate::db::{Db, ACC_META_BY_IRI_READY, SEP};
use crate::repo::catalog::CatalogRepository;
use crate::repo::triple::TripleRepository;

const SCHEMA_VERSION: i64 = 2;
const PAGE_SIZE: usize = 10_000;
const COPY_COMPLETE: &[u8] = b"backend-copy:complete";

#[derive(Clone)]
pub struct RocksdbMigrator {
    db: Db,
}

impl RocksdbMigrator {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Migrator for RocksdbMigrator {
    async fn run_migrations(&self) -> Result<(), DomainError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let catalog = CatalogRepository::new(db.clone());
            if !catalog.generation_ready()? {
                rebuild_catalog(&db)?;
            }
            db.put_cf("meta", b"schema_version", &SCHEMA_VERSION.to_be_bytes())
        })
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
    }

    async fn migration_status(&self) -> Result<Vec<MigrationEntry>, DomainError> {
        let db = self.db.clone();
        let applied = tokio::task::spawn_blocking(move || {
            let version = db.get_cf("meta", b"schema_version")?;
            Ok::<_, DomainError>(version.is_some_and(|value| {
                value.len() == 8 && i64::from_be_bytes(value.try_into().unwrap()) >= SCHEMA_VERSION
            }))
        })
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))??;
        Ok(vec![MigrationEntry {
            version: SCHEMA_VERSION,
            description: "universal RDF catalog and exact counters".to_owned(),
            applied,
        }])
    }
}

/// Rebuild every universal catalog keyspace from canonical/legacy durable
/// state. The generation marker is written last; interrupted work is discarded
/// and deterministically rebuilt on the next run.
pub(crate) fn rebuild_catalog(db: &Db) -> Result<(), DomainError> {
    db.delete_cf("meta", b"catalog:generation:v1")?;
    ensure_reverse_resource_index(db)?;
    for cf in [
        "catalog_graph",
        "catalog_graph_by_iri",
        "catalog_resource_refcount",
        "catalog_sequence_refcount",
        "catalog_class_resource_refcount",
        "catalog_class_count",
    ] {
        clear_cf(db, cf)?;
    }

    let mut stats = rebuild_resource_catalog(db)?;
    stats.named_graphs = rebuild_graph_catalog(db)?;
    stats.triples = copied_triple_count(db)?
        .unwrap_or(count_cf(db, "spog")?.saturating_add(count_cf(db, "dspo")?));
    stats.ontologies = count_cf(db, "ont")?;

    let mut batch = WriteBatch::default();
    CatalogRepository::new(db.clone()).seed_stats(&mut batch, &stats)?;
    db.write(batch)
}

fn ensure_reverse_resource_index(db: &Db) -> Result<(), DomainError> {
    if db.exists_cf("meta", ACC_META_BY_IRI_READY)? {
        return Ok(());
    }
    clear_cf(db, "acc_meta_by_iri")?;
    let mut after: Option<Vec<u8>> = None;
    loop {
        let mut rows = Vec::with_capacity(PAGE_SIZE);
        db.for_each_prefix_after("acc_meta", b"", after.as_deref(), |key, value| {
            rows.push((key.to_vec(), value.to_vec()));
            Ok(rows.len() < PAGE_SIZE)
        })?;
        if rows.is_empty() {
            break;
        }
        after = rows.last().map(|(key, _)| key.clone());
        let mut batch = WriteBatch::default();
        for (key, value) in rows {
            let Some(separator) = key.iter().position(|byte| *byte == SEP) else {
                return Err(DomainError::Database(
                    "accelerator metadata key has no graph/IRI separator".into(),
                ));
            };
            let graph = &key[..separator];
            let iri = &key[separator + 1..];
            let mut reverse = Vec::with_capacity(key.len());
            reverse.extend_from_slice(iri);
            reverse.push(SEP);
            reverse.extend_from_slice(graph);
            batch.put_cf(&db.cf("acc_meta_by_iri"), reverse, value);
        }
        db.write(batch)?;
    }
    db.put_cf("meta", ACC_META_BY_IRI_READY, b"1")
}

fn rebuild_resource_catalog(db: &Db) -> Result<CorpusStats, DomainError> {
    let mut stats = CorpusStats::default();
    let mut class_totals: HashMap<String, u64> = HashMap::new();
    let mut current_iri: Option<String> = None;
    let mut occurrences = 0_u64;
    let mut sequence_occurrences = 0_u64;
    let mut class_occurrences: HashMap<String, u64> = HashMap::new();
    let mut batch = WriteBatch::default();
    let mut pending = 0_usize;

    db.for_each("acc_meta_by_iri", |key, value| {
        let Some(separator) = key.iter().position(|byte| *byte == SEP) else {
            return Err(DomainError::Database(
                "resource catalog key has no IRI/graph separator".into(),
            ));
        };
        let iri = std::str::from_utf8(&key[..separator])
            .map_err(|_| DomainError::Database("non-utf8 resource IRI".into()))?;
        if current_iri.as_deref().is_some_and(|current| current != iri) {
            flush_resource(
                db,
                &mut batch,
                current_iri.as_deref().unwrap(),
                occurrences,
                sequence_occurrences,
                &class_occurrences,
                &mut class_totals,
                &mut stats,
            );
            pending += 1;
            if pending >= PAGE_SIZE {
                db.write(std::mem::take(&mut batch))?;
                pending = 0;
            }
            occurrences = 0;
            sequence_occurrences = 0;
            class_occurrences.clear();
        }
        current_iri = Some(iri.to_owned());
        occurrences += 1;
        let meta: MetaRecord = serde_json::from_slice(value).map_err(ser_err)?;
        if is_sequence(&meta) {
            sequence_occurrences += 1;
        }
        for class in meta.types {
            *class_occurrences.entry(class).or_default() += 1;
        }
        Ok(true)
    })?;
    if let Some(iri) = current_iri {
        flush_resource(
            db,
            &mut batch,
            &iri,
            occurrences,
            sequence_occurrences,
            &class_occurrences,
            &mut class_totals,
            &mut stats,
        );
    }
    for (class, count) in class_totals {
        batch.put_cf(
            &db.cf("catalog_class_count"),
            class.as_bytes(),
            count.to_le_bytes(),
        );
    }
    db.write(batch)?;
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn flush_resource(
    db: &Db,
    batch: &mut WriteBatch,
    iri: &str,
    occurrences: u64,
    sequence_occurrences: u64,
    classes: &HashMap<String, u64>,
    class_totals: &mut HashMap<String, u64>,
    stats: &mut CorpusStats,
) {
    batch.put_cf(
        &db.cf("catalog_resource_refcount"),
        iri.as_bytes(),
        occurrences.to_le_bytes(),
    );
    stats.resources += 1;
    if sequence_occurrences > 0 {
        batch.put_cf(
            &db.cf("catalog_sequence_refcount"),
            iri.as_bytes(),
            sequence_occurrences.to_le_bytes(),
        );
        stats.sequences += 1;
    }
    for (class, count) in classes {
        let mut key = Vec::with_capacity(iri.len() + class.len() + 1);
        key.extend_from_slice(iri.as_bytes());
        key.push(SEP);
        key.extend_from_slice(class.as_bytes());
        batch.put_cf(
            &db.cf("catalog_class_resource_refcount"),
            key,
            count.to_le_bytes(),
        );
        *class_totals.entry(class.clone()).or_default() += 1;
    }
}

fn rebuild_graph_catalog(db: &Db) -> Result<u64, DomainError> {
    let triples = TripleRepository::new(db.clone());
    let graph_iris = triples.distinct_named_graphs_blocking()?;
    let graph_set: HashSet<&str> = graph_iris.iter().map(String::as_str).collect();
    let mut known: HashMap<String, NamedGraphRecord> = HashMap::new();

    db.for_each("verbatim_graph_meta", |_, value| {
        let row: GraphCatalogImport = serde_json::from_slice(value).map_err(ser_err)?;
        if graph_set.contains(row.iri.as_str()) {
            known.insert(
                row.iri.clone(),
                NamedGraphRecord {
                    id: row.id,
                    iri: row.iri,
                    name: row.name,
                    description: None,
                    source_uri: row.source_uri,
                    serialization_format: row.serialization_format,
                    triple_count: Some(row.triple_count),
                    resource_count: None,
                    created_at: Some(row.created_at),
                    updated_at: None,
                },
            );
        }
        Ok(true)
    })?;
    db.for_each("graph_meta", |_, value| {
        let row: GraphRecord = serde_json::from_slice(value).map_err(ser_err)?;
        let iri = format!("{}{}", sbol_db_rdf::GRAPH_IRI_PREFIX, row.id.0);
        if graph_set.contains(iri.as_str()) {
            known.insert(
                iri.clone(),
                NamedGraphRecord {
                    id: row.id,
                    iri,
                    name: row.name,
                    description: row.description,
                    source_uri: row.source_uri,
                    serialization_format: Some(row.serialization_format.as_db_str().to_owned()),
                    triple_count: None,
                    resource_count: None,
                    created_at: Some(row.created_at),
                    updated_at: Some(row.updated_at),
                },
            );
        }
        Ok(true)
    })?;

    let mut batch = WriteBatch::default();
    for (index, iri) in graph_iris.iter().enumerate() {
        let gid = Term::named(iri).id();
        let mut record = known.remove(iri).unwrap_or_else(|| NamedGraphRecord {
            id: GraphId(uuid::Uuid::from_bytes(gid)),
            iri: iri.clone(),
            name: None,
            description: None,
            source_uri: None,
            serialization_format: None,
            triple_count: None,
            resource_count: None,
            created_at: None,
            updated_at: None,
        });
        record.triple_count = Some(count_prefix(db, "gspo", &gid)?);
        let mut resource_prefix = iri.as_bytes().to_vec();
        resource_prefix.push(SEP);
        record.resource_count = Some(count_prefix(db, "acc_meta", &resource_prefix)?);
        batch.put_cf(
            &db.cf("catalog_graph"),
            record.id.0.as_bytes(),
            serde_json::to_vec(&record).map_err(ser_err)?,
        );
        batch.put_cf(
            &db.cf("catalog_graph_by_iri"),
            iri.as_bytes(),
            record.id.0.as_bytes(),
        );
        if (index + 1) % PAGE_SIZE == 0 {
            db.write(std::mem::take(&mut batch))?;
        }
    }
    db.write(batch)?;
    Ok(graph_iris.len() as u64)
}

fn clear_cf(db: &Db, cf: &str) -> Result<(), DomainError> {
    let mut after: Option<Vec<u8>> = None;
    loop {
        let mut keys = Vec::with_capacity(PAGE_SIZE);
        db.for_each_prefix_after(cf, b"", after.as_deref(), |key, _| {
            keys.push(key.to_vec());
            Ok(keys.len() < PAGE_SIZE)
        })?;
        if keys.is_empty() {
            return Ok(());
        }
        after = keys.last().cloned();
        let mut batch = WriteBatch::default();
        for key in keys {
            batch.delete_cf(&db.cf(cf), key);
        }
        db.write(batch)?;
    }
}

fn count_cf(db: &Db, cf: &str) -> Result<u64, DomainError> {
    let mut count = 0;
    db.for_each(cf, |_, _| {
        count += 1;
        Ok(true)
    })?;
    Ok(count)
}

fn count_prefix(db: &Db, cf: &str, prefix: &[u8]) -> Result<u64, DomainError> {
    let mut count = 0;
    db.for_each_prefix(cf, prefix, |_, _| {
        count += 1;
        Ok(true)
    })?;
    Ok(count)
}

fn is_sequence(meta: &MetaRecord) -> bool {
    meta.types
        .iter()
        .any(|ty| ty == "http://sbols.org/v2#Sequence" || ty == "http://sbols.org/v3#Sequence")
}

#[derive(Deserialize)]
struct CopyReport {
    status: String,
    counts: CopyCounts,
}

#[derive(Deserialize)]
struct CopyCounts {
    triples: u64,
}

fn copied_triple_count(db: &Db) -> Result<Option<u64>, DomainError> {
    let Some(value) = db.get_cf("meta", COPY_COMPLETE)? else {
        return Ok(None);
    };
    let report: CopyReport = serde_json::from_slice(&value).map_err(ser_err)?;
    Ok((report.status == "ready").then_some(report.counts.triples))
}

fn ser_err(error: serde_json::Error) -> DomainError {
    DomainError::Serialization(error.to_string())
}
