//! Bounded, resumable primitives for copying a reconciled backend into RocksDB.
//!
//! Interactive Graph Store writes deliberately rebuild per-graph derived
//! indexes atomically. A production corpus cannot use that path chunk by chunk:
//! rebuilding a multi-million-triple graph after every page would be quadratic.
//! This loader writes trusted canonical triples and already-reconciled
//! accelerator rows directly, checkpointing every committed page in the same
//! RocksDB batch.

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, Triple};
use sbol_db_storage::{ClusterId, FacetKind, MetaRecord, RankRow, Signature};

use crate::db::{compose, Db, SEP};
use crate::repo::accel::{
    count_key_member, count_key_role, count_key_root_toplevel_type, count_key_root_type,
    count_key_toplevel, count_key_toplevel_type, count_key_toplevel_type_role, count_key_type,
    AccelRepository,
};
use crate::repo::TripleRepository;

const COPY_SOURCE: &[u8] = b"backend-copy:source";
const CHECKPOINT_PREFIX: &str = "backend-copy:checkpoint:";
const COMPLETE: &[u8] = b"backend-copy:complete";

#[derive(Clone, Debug)]
pub struct AccelObjectImport {
    pub graph: String,
    pub iri: String,
    pub meta: MetaRecord,
}

#[derive(Clone, Debug)]
pub struct AccelMemberImport {
    pub graph: String,
    pub collection: String,
    pub member: String,
    pub sort_key: String,
    pub is_root: bool,
}

#[derive(Clone, Debug)]
pub struct AccelFacetImport {
    pub graph: String,
    pub kind: FacetKind,
    pub value: String,
    pub subject_count: u64,
}

#[derive(Clone, Debug)]
pub enum AccelCountKind {
    TopLevel,
    Type(String),
    TopLevelType(String),
    RootType(String),
    RootTopLevelType(String),
    Role(String),
    TopLevelTypeRole { object_type: String, role: String },
    Member { collection: String, root_only: bool },
}

#[derive(Clone, Debug)]
pub struct AccelCountImport {
    pub graph: String,
    pub kind: AccelCountKind,
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct SketchImport {
    pub iri: String,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SketchBandImport {
    pub iri: String,
    /// PostgreSQL stores the same 64 bits in a signed `bigint`.
    pub band_hash: i64,
}

#[derive(Clone)]
pub struct RocksdbBulkLoader {
    db: Db,
    triples: TripleRepository,
    accel: AccelRepository,
}

impl RocksdbBulkLoader {
    pub fn new(db: Db) -> Self {
        Self {
            triples: TripleRepository::new(db.clone()),
            accel: AccelRepository::new(db.clone()),
            db,
        }
    }

    /// Bind an empty destination to one immutable source, or resume a prior
    /// copy of that same source. A different source is never allowed to append
    /// into the existing keyspaces.
    pub async fn prepare(&self, source: &str) -> Result<(), DomainError> {
        let db = self.db.clone();
        let source = source.to_owned();
        blocking(move || match db.get_cf("meta", COPY_SOURCE)? {
            Some(existing) if existing == source.as_bytes() => Ok(()),
            Some(existing) => Err(DomainError::Database(format!(
                "RocksDB destination belongs to source `{}` rather than `{source}`",
                String::from_utf8_lossy(&existing)
            ))),
            None => {
                for cf in [
                    "gspo",
                    "users",
                    "api_tokens",
                    "app_config",
                    "acc_meta",
                    "object_pagerank",
                    "sequence_cluster",
                    "seq_sketch",
                ] {
                    if has_any(&db, cf)? {
                        return Err(DomainError::Database(format!(
                            "RocksDB destination is not empty (`{cf}` already has data)"
                        )));
                    }
                }
                db.put_cf("meta", COPY_SOURCE, source.as_bytes())
            }
        })
        .await
    }

    pub async fn checkpoint(&self, stage: &str) -> Result<Option<String>, DomainError> {
        let db = self.db.clone();
        let key = checkpoint_key(stage);
        blocking(move || {
            db.get_cf("meta", &key)?
                .map(|value| {
                    String::from_utf8(value).map_err(|_| {
                        DomainError::Database("backend-copy checkpoint is not UTF-8".into())
                    })
                })
                .transpose()
        })
        .await
    }

    pub async fn write_triples(
        &self,
        triples: Vec<Triple>,
        checkpoint: String,
    ) -> Result<usize, DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let inserted = this.triples.stage_bulk_insert(&mut batch, &triples)?;
            stage_checkpoint(&this.db, &mut batch, "triples", &checkpoint);
            this.db.write(batch)?;
            Ok(inserted)
        })
        .await
    }

    pub async fn write_accel_objects(
        &self,
        rows: Vec<AccelObjectImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                this.accel
                    .stage_import_object(&mut batch, &row.graph, &row.iri, &row.meta)?;
            }
            // v2 stores the object metadata on ordered secondary-index values,
            // eliminating a random acc_meta lookup for every skipped row in a
            // deep page. Replaying a v1 destination is idempotent because all
            // primary and secondary keys are content-derived.
            stage_checkpoint(&this.db, &mut batch, "accel_objects_v2", &checkpoint);
            this.db.write(batch)
        })
        .await
    }

    pub async fn write_accel_members(
        &self,
        rows: Vec<AccelMemberImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                this.accel.stage_import_member(
                    &mut batch,
                    &row.graph,
                    &row.collection,
                    &row.member,
                    &row.sort_key,
                    row.is_root,
                );
            }
            stage_checkpoint(&this.db, &mut batch, "accel_members", &checkpoint);
            this.db.write(batch)
        })
        .await
    }

    pub async fn write_accel_facets(&self, rows: Vec<AccelFacetImport>) -> Result<(), DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                this.accel.stage_import_facet(
                    &mut batch,
                    &row.graph,
                    row.kind,
                    &row.value,
                    row.subject_count,
                );
            }
            this.db.write(batch)
        })
        .await
    }

    pub async fn write_accel_counts(&self, rows: Vec<AccelCountImport>) -> Result<(), DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let key = match row.kind {
                    AccelCountKind::TopLevel => count_key_toplevel(&row.graph),
                    AccelCountKind::Type(value) => count_key_type(&row.graph, &value),
                    AccelCountKind::TopLevelType(value) => {
                        count_key_toplevel_type(&row.graph, &value)
                    }
                    AccelCountKind::RootType(value) => count_key_root_type(&row.graph, &value),
                    AccelCountKind::RootTopLevelType(value) => {
                        count_key_root_toplevel_type(&row.graph, &value)
                    }
                    AccelCountKind::Role(value) => count_key_role(&row.graph, &value),
                    AccelCountKind::TopLevelTypeRole { object_type, role } => {
                        count_key_toplevel_type_role(&row.graph, &object_type, &role)
                    }
                    AccelCountKind::Member {
                        collection,
                        root_only,
                    } => count_key_member(&row.graph, &collection, root_only),
                };
                this.accel.stage_import_count(&mut batch, key, row.count);
            }
            this.db.write(batch)
        })
        .await
    }

    pub async fn write_ranks(
        &self,
        rows: Vec<RankRow>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                batch.put_cf(
                    &db.cf("object_pagerank"),
                    row.iri.as_bytes(),
                    row.score.to_le_bytes(),
                );
            }
            stage_checkpoint(&db, &mut batch, "pagerank", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_clusters(
        &self,
        rows: Vec<(String, ClusterId)>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for (iri, cluster) in rows {
                let id = cluster.0.to_be_bytes();
                batch.put_cf(&db.cf("sequence_cluster"), iri.as_bytes(), id);
                batch.put_cf(
                    &db.cf("sequence_cluster_by_id"),
                    compose(&[&id, iri.as_bytes()]),
                    [],
                );
            }
            stage_checkpoint(&db, &mut batch, "clusters", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_sketches(
        &self,
        rows: Vec<SketchImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                if Signature::from_bytes(&row.signature).is_none() {
                    return Err(DomainError::Database(format!(
                        "invalid sketch signature for {}",
                        row.iri
                    )));
                }
                batch.put_cf(&db.cf("seq_sketch"), row.iri.as_bytes(), row.signature);
            }
            stage_checkpoint(&db, &mut batch, "sketches", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_sketch_bands(
        &self,
        rows: Vec<SketchBandImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let band = (row.band_hash as u64).to_be_bytes();
                let mut by_band = band.to_vec();
                by_band.extend_from_slice(row.iri.as_bytes());
                let mut by_iri = row.iri.as_bytes().to_vec();
                by_iri.push(SEP);
                by_iri.extend_from_slice(&band);
                batch.put_cf(&db.cf("seq_lsh_band"), by_band, []);
                batch.put_cf(&db.cf("seq_lsh_band_by_iri"), by_iri, []);
            }
            // v1 started the signed Postgres keyset at zero, which skipped
            // every negative band hash. Keep a versioned checkpoint so a
            // destination produced by that loader is repaired by replaying
            // the complete key range; RocksDB puts make the replay
            // idempotent for the positive half already present.
            stage_checkpoint(&db, &mut batch, "sketch_bands_v2", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn count(&self, cf: &'static str) -> Result<u64, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut count = 0_u64;
            db.for_each(cf, |_, _| {
                count += 1;
                Ok(true)
            })?;
            Ok(count)
        })
        .await
    }

    pub async fn mark_complete(&self, report: &str) -> Result<(), DomainError> {
        let db = self.db.clone();
        let report = report.to_owned();
        blocking(move || db.put_cf("meta", COMPLETE, report.as_bytes())).await
    }
}

fn checkpoint_key(stage: &str) -> Vec<u8> {
    format!("{CHECKPOINT_PREFIX}{stage}").into_bytes()
}

fn stage_checkpoint(db: &Db, batch: &mut WriteBatch, stage: &str, value: &str) {
    batch.put_cf(&db.cf("meta"), checkpoint_key(stage), value.as_bytes());
}

fn has_any(db: &Db, cf: &str) -> Result<bool, DomainError> {
    let mut any = false;
    db.for_each(cf, |_, _| {
        any = true;
        Ok(false)
    })?;
    Ok(any)
}

async fn blocking<T, F>(f: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
}
