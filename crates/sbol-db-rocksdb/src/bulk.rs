//! Bounded, resumable primitives for copying a reconciled backend into RocksDB.
//!
//! Interactive Graph Store writes deliberately rebuild per-graph derived
//! indexes atomically. A production corpus cannot use that path chunk by chunk:
//! rebuilding a multi-million-triple graph after every page would be quadratic.
//! This loader writes trusted canonical triples and already-reconciled
//! accelerator rows directly, checkpointing every committed page in the same
//! RocksDB batch.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use sbol_db_core::{
    DomainError, GraphId, IriString, ObjectId, ObjectSummary, SequenceAlphabet, SequenceProjection,
    Triple,
};
use sbol_db_rdf::hash_bytes;
use sbol_db_storage::{ClusterId, FacetKind, MetaRecord, RankRow, Signature};
use serde::{Deserialize, Serialize};

use crate::db::{compose, Db, ACC_META_BY_IRI_READY, COLUMN_FAMILIES, SEP};
use crate::migrate::rebuild_catalog;
use crate::repo::accel::{
    count_key_member, count_key_role, count_key_root_toplevel_type, count_key_root_type,
    count_key_toplevel, count_key_toplevel_type, count_key_toplevel_type_role, count_key_type,
    AccelRepository,
};
use crate::repo::{ObjectRepository, SequenceSearchRepository, TripleRepository};

const COPY_SOURCE: &[u8] = b"backend-copy:source";
const CHECKPOINT_PREFIX: &str = "backend-copy:checkpoint:";
const COMPLETE: &[u8] = b"backend-copy:complete";

#[derive(Clone, Debug)]
pub struct AccelObjectImport {
    pub graph: String,
    pub iri: String,
    pub meta: MetaRecord,
}

/// One exact named-graph registry row from a reconciled verbatim corpus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphCatalogImport {
    pub id: GraphId,
    pub iri: String,
    pub kind: String,
    pub name: Option<String>,
    pub source_uri: Option<String>,
    pub serialization_format: Option<String>,
    pub created_at: DateTime<Utc>,
    pub triple_count: u64,
}

#[derive(Clone, Debug)]
pub struct SequenceProjectionImport {
    pub iri: String,
    pub encoding_iri: Option<String>,
    pub elements: String,
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

#[derive(Clone, Debug)]
pub struct OntologyImport {
    pub prefix: String,
    pub name: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub term_count: i64,
    pub imported_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct OntologyTermImport {
    pub iri: String,
    pub prefix: String,
    pub curie: String,
    pub name: String,
    pub definition: Option<String>,
    pub is_obsolete: bool,
    pub synonyms: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct OntologyAliasImport {
    pub prefix: String,
    pub alias_iri: String,
    pub canonical_iri: String,
}

#[derive(Clone, Debug)]
pub struct OntologyClosureImport {
    pub prefix: String,
    pub ancestor_iri: String,
    pub descendant_iri: String,
    pub depth: i16,
}

#[derive(Clone)]
pub struct RocksdbBulkLoader {
    db: Db,
    triples: TripleRepository,
    accel: AccelRepository,
    objects: ObjectRepository,
    sequences: SequenceSearchRepository,
}

impl RocksdbBulkLoader {
    pub fn new(db: Db) -> Self {
        Self {
            triples: TripleRepository::new(db.clone()),
            accel: AccelRepository::new(db.clone()),
            objects: ObjectRepository::new(db.clone()),
            sequences: SequenceSearchRepository::new(db.clone()),
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
                // A backend copy is a complete database replacement, not an
                // append operation. Reject every populated product/catalog
                // keyspace so an omitted surface cannot silently mix old and
                // new data. `meta` is reserved for schema/copy markers.
                for cf in COLUMN_FAMILIES.iter().copied().filter(|cf| *cf != "meta") {
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

    pub async fn write_graph_catalog(
        &self,
        rows: Vec<GraphCatalogImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let key = row.id.0.as_bytes();
                let value = serde_json::to_vec(&row)
                    .map_err(|error| DomainError::Serialization(error.to_string()))?;
                batch.put_cf(&db.cf("verbatim_graph_meta"), key, value);
            }
            stage_checkpoint(&db, &mut batch, "graph_catalog", &checkpoint);
            db.write(batch)
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
            let mut page_ids: HashMap<String, ObjectId> = HashMap::new();
            for row in rows {
                this.accel
                    .stage_import_object(&mut batch, &row.graph, &row.iri, &row.meta)?;
                let preferred_id = page_ids.get(&row.iri).copied();
                let summary = object_summary(&row)?;
                let id =
                    this.objects
                        .stage_upsert_with_id(&mut batch, &summary, None, preferred_id)?;
                page_ids.insert(row.iri, id);
            }
            // v4 also materializes the globally IRI-ordered accelerator view;
            // it replays v3 copies once so existing converted destinations gain
            // the new index. The backend-neutral object browser view was added
            // in v3 and remains idempotent here.
            // Replaying an older destination is idempotent because the primary
            // keys are content-derived and object ids are preserved on update.
            // v2 stored the object metadata on ordered secondary-index values,
            // eliminating a random acc_meta lookup for every skipped row in a
            // deep page.
            stage_checkpoint(&this.db, &mut batch, "accel_objects_v4", &checkpoint);
            this.db.write(batch)
        })
        .await
    }

    /// Mark the reverse accelerator projection complete after every source
    /// object page has committed. Readers never use a partially copied index.
    pub async fn mark_resource_index_ready(&self) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || db.put_cf("meta", ACC_META_BY_IRI_READY, b"1")).await
    }

    pub async fn write_sequence_projections(
        &self,
        rows: Vec<SequenceProjectionImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let alphabet = sequence_alphabet(row.encoding_iri.as_deref());
                let projection = SequenceProjection {
                    iri: IriString::unchecked(row.iri),
                    encoding_iri: row.encoding_iri.map(IriString::unchecked),
                    content_hash: Some(hash_bytes(row.elements.as_bytes())),
                    elements: Some(row.elements),
                    alphabet: Some(alphabet),
                };
                this.sequences.stage_upsert(&mut batch, &projection)?;
            }
            stage_checkpoint(&this.db, &mut batch, "sequence_projections", &checkpoint);
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

    pub async fn write_ontologies(
        &self,
        rows: Vec<OntologyImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let value = serde_json::json!({
                    "name": row.name,
                    "source_url": row.source_url,
                    "version": row.version,
                    "term_count": row.term_count,
                    "imported_at": row.imported_at,
                });
                batch.put_cf(
                    &db.cf("ont"),
                    row.prefix.as_bytes(),
                    serde_json::to_vec(&value).map_err(ser_err)?,
                );
            }
            stage_checkpoint(&db, &mut batch, "ontologies", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_ontology_terms(
        &self,
        rows: Vec<OntologyTermImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let value = serde_json::json!({
                    "prefix": row.prefix,
                    "curie": row.curie,
                    "name": row.name,
                    "definition": row.definition,
                    "is_obsolete": row.is_obsolete,
                    "synonyms": row.synonyms,
                });
                batch.put_cf(
                    &db.cf("ont_term"),
                    row.iri.as_bytes(),
                    serde_json::to_vec(&value).map_err(ser_err)?,
                );
                batch.put_cf(
                    &db.cf("ont_term_idx"),
                    compose(&[
                        row.prefix.as_bytes(),
                        row.curie.as_bytes(),
                        row.iri.as_bytes(),
                    ]),
                    [],
                );
            }
            stage_checkpoint(&db, &mut batch, "ontology_terms", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_ontology_aliases(
        &self,
        rows: Vec<OntologyAliasImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                batch.put_cf(
                    &db.cf("ont_alias"),
                    row.alias_iri.as_bytes(),
                    row.canonical_iri.as_bytes(),
                );
                batch.put_cf(
                    &db.cf("ont_alias_idx"),
                    compose(&[row.prefix.as_bytes(), row.alias_iri.as_bytes()]),
                    [],
                );
            }
            stage_checkpoint(&db, &mut batch, "ontology_aliases", &checkpoint);
            db.write(batch)
        })
        .await
    }

    pub async fn write_ontology_closure(
        &self,
        rows: Vec<OntologyClosureImport>,
        checkpoint: String,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for row in rows {
                let depth = (row.depth.max(0) as u16).to_be_bytes();
                let primary = compose(&[
                    row.ancestor_iri.as_bytes(),
                    &depth,
                    row.descendant_iri.as_bytes(),
                ]);
                batch.put_cf(&db.cf("ont_closure"), &primary, []);
                batch.put_cf(
                    &db.cf("ont_closure_idx"),
                    compose(&[row.prefix.as_bytes(), &primary]),
                    [],
                );
            }
            stage_checkpoint(&db, &mut batch, "ontology_closure", &checkpoint);
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
        blocking(move || {
            // Catalog readiness is the commit point. Never publish a completed
            // copy report for a destination whose universal projections could
            // not be rebuilt successfully.
            rebuild_catalog(&db)?;
            db.put_cf("meta", COMPLETE, report.as_bytes())
        })
        .await
    }
}

fn object_summary(row: &AccelObjectImport) -> Result<ObjectSummary, DomainError> {
    let encoded = serde_json::to_vec(&row.meta)
        .map_err(|error| DomainError::Serialization(error.to_string()))?;
    Ok(ObjectSummary {
        iri: IriString::unchecked(row.iri.clone()),
        sbol_class: row.meta.types.first().cloned().unwrap_or_default(),
        display_id: first_literal(&row.meta.display_id),
        name: first_literal(&row.meta.name),
        description: first_literal(&row.meta.description),
        types: row.meta.sbol_types.clone(),
        roles: row.meta.roles.clone(),
        data: serde_json::to_value(&row.meta)
            .map_err(|error| DomainError::Serialization(error.to_string()))?,
        content_hash: hash_bytes(&encoded),
    })
}

fn first_literal(values: &[sbol_db_storage::LitVal]) -> Option<String> {
    values.first().map(|value| value.value.clone())
}

fn sequence_alphabet(encoding: Option<&str>) -> SequenceAlphabet {
    let encoding = encoding.unwrap_or_default().to_ascii_lowercase();
    if encoding.contains("rna") {
        SequenceAlphabet::Rna
    } else {
        // The source rows come from the nucleotide sketch index, which only
        // admits DNA/RNA sequences. Historic SBOL2 DNA encodings do not carry
        // the word `dna`, so DNA is the correct fallback.
        SequenceAlphabet::Dna
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

fn ser_err(error: serde_json::Error) -> DomainError {
    DomainError::Serialization(error.to_string())
}

#[cfg(test)]
mod tests {
    use sbol_db_core::{ObjectTerm, SubjectTerm};
    use sbol_db_storage::LitVal;

    use super::*;
    use crate::repo::{LabRepository, OntologyRepository};

    #[tokio::test]
    async fn compatibility_copy_materializes_admin_views() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let loader = RocksdbBulkLoader::new(db.clone());
        loader.prepare("test-source").await.unwrap();

        let graph_id = GraphId::new();
        let graph_iri = "https://example.org/public";
        let part_iri = "https://example.org/part";
        let sequence_iri = "https://example.org/sequence";
        let class = "http://sbols.org/v2#ComponentDefinition";
        let sequence_class = "http://sbols.org/v2#Sequence";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let triples = vec![
            Triple {
                graph_iri: Some(IriString::unchecked(graph_iri.to_owned())),
                subject: SubjectTerm::Iri(IriString::unchecked(part_iri.to_owned())),
                predicate: IriString::unchecked(rdf_type.to_owned()),
                object: ObjectTerm::Iri(IriString::unchecked(class.to_owned())),
            },
            Triple {
                graph_iri: Some(IriString::unchecked(graph_iri.to_owned())),
                subject: SubjectTerm::Iri(IriString::unchecked(sequence_iri.to_owned())),
                predicate: IriString::unchecked(rdf_type.to_owned()),
                object: ObjectTerm::Iri(IriString::unchecked(sequence_class.to_owned())),
            },
            Triple {
                graph_iri: Some(IriString::unchecked(graph_iri.to_owned())),
                subject: SubjectTerm::Iri(IriString::unchecked(sequence_iri.to_owned())),
                predicate: IriString::unchecked("http://sbols.org/v2#elements".to_owned()),
                object: ObjectTerm::Literal {
                    value: "AACCGGTT".to_owned(),
                    datatype: IriString::unchecked(
                        "http://www.w3.org/2001/XMLSchema#string".to_owned(),
                    ),
                    language: None,
                },
            },
        ];
        loader
            .write_triples(triples.clone(), "triples-ready".to_owned())
            .await
            .unwrap();
        loader
            .write_graph_catalog(
                vec![GraphCatalogImport {
                    id: graph_id,
                    iri: graph_iri.to_owned(),
                    kind: "verbatim".to_owned(),
                    name: Some("Public".to_owned()),
                    source_uri: None,
                    serialization_format: None,
                    created_at: Utc::now(),
                    triple_count: triples.len() as u64,
                }],
                graph_iri.to_owned(),
            )
            .await
            .unwrap();

        loader
            .write_accel_objects(
                vec![
                    AccelObjectImport {
                        graph: graph_iri.to_owned(),
                        iri: part_iri.to_owned(),
                        meta: MetaRecord {
                            display_id: vec![LitVal {
                                value: "part".to_owned(),
                                datatype: "http://www.w3.org/2001/XMLSchema#string".to_owned(),
                                language: None,
                            }],
                            types: vec![class.to_owned()],
                            top_level: true,
                            ..MetaRecord::default()
                        },
                    },
                    AccelObjectImport {
                        graph: graph_iri.to_owned(),
                        iri: sequence_iri.to_owned(),
                        meta: MetaRecord {
                            types: vec![sequence_class.to_owned()],
                            ..MetaRecord::default()
                        },
                    },
                ],
                serde_json::to_string(&(graph_iri, sequence_iri)).unwrap(),
            )
            .await
            .unwrap();
        loader.mark_resource_index_ready().await.unwrap();
        assert!(db.exists_cf("meta", ACC_META_BY_IRI_READY).unwrap());
        assert_eq!(loader.count("acc_meta_by_iri").await.unwrap(), 2);
        loader
            .write_accel_facets(vec![AccelFacetImport {
                graph: graph_iri.to_owned(),
                kind: FacetKind::Types,
                value: class.to_owned(),
                subject_count: 1,
            }])
            .await
            .unwrap();
        loader
            .write_sequence_projections(
                vec![SequenceProjectionImport {
                    iri: sequence_iri.to_owned(),
                    encoding_iri: None,
                    elements: "AACCGGTT".to_owned(),
                }],
                sequence_iri.to_owned(),
            )
            .await
            .unwrap();
        let imported_at = Utc::now();
        loader
            .write_ontologies(
                vec![OntologyImport {
                    prefix: "SO".to_owned(),
                    name: "Sequence Ontology".to_owned(),
                    source_url: Some("https://example.org/so.obo".to_owned()),
                    version: Some("test".to_owned()),
                    term_count: 1,
                    imported_at,
                }],
                "SO".to_owned(),
            )
            .await
            .unwrap();
        loader
            .write_ontology_terms(
                vec![OntologyTermImport {
                    iri: "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
                    prefix: "SO".to_owned(),
                    curie: "SO:0000167".to_owned(),
                    name: "promoter".to_owned(),
                    definition: None,
                    is_obsolete: false,
                    synonyms: Vec::new(),
                }],
                "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
            )
            .await
            .unwrap();
        loader
            .write_ontology_aliases(
                vec![OntologyAliasImport {
                    prefix: "SO".to_owned(),
                    alias_iri: "https://example.org/promoter".to_owned(),
                    canonical_iri: "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
                }],
                "https://example.org/promoter".to_owned(),
            )
            .await
            .unwrap();
        loader
            .write_ontology_closure(
                vec![OntologyClosureImport {
                    prefix: "SO".to_owned(),
                    ancestor_iri: "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
                    descendant_iri: "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
                    depth: 0,
                }],
                serde_json::to_string(&(
                    "http://purl.obolibrary.org/obo/SO_0000167",
                    "http://purl.obolibrary.org/obo/SO_0000167",
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        loader
            .mark_complete(
                r#"{"status":"ready","counts":{"graphs":1,"triples":3,"accelerator_objects":2,"objects":2,"sketches":0,"sequence_projections":1,"ontologies":1,"ontology_terms":1,"ontology_aliases":1,"ontology_closure":1}}"#,
            )
            .await
            .unwrap();

        let lab = LabRepository::new(db.clone());
        let counts = lab.corpus_counts().unwrap();
        assert_eq!((counts.graphs, counts.objects, counts.sequences), (1, 2, 1));
        assert_eq!(counts.ontologies, 1);
        let graph = lab.get_graph_overview(graph_id).unwrap().unwrap();
        assert_eq!(graph.iri, graph_iri);
        assert_eq!(graph.triple_count, Some(3));
        assert!(lab
            .top_classes(10)
            .unwrap()
            .iter()
            .any(|row| row.iri == class));

        let object = ObjectRepository::new(db.clone())
            .get_by_iri(part_iri)
            .unwrap()
            .unwrap();
        assert_eq!(object.display_id.as_deref(), Some("part"));
        let ontologies = OntologyRepository::new(db.clone())
            .list_ontologies()
            .unwrap();
        assert_eq!(ontologies[0].prefix, "SO");
        assert_eq!(
            OntologyRepository::new(db.clone())
                .canonicalize("https://example.org/promoter")
                .unwrap(),
            Some("http://purl.obolibrary.org/obo/SO_0000167".to_owned())
        );
        assert_eq!(
            OntologyRepository::new(db.clone())
                .descendants("https://example.org/promoter")
                .unwrap(),
            vec![("http://purl.obolibrary.org/obo/SO_0000167".to_owned(), 0)]
        );
        let hits = SequenceSearchRepository::new(db)
            .search("AACCGGTT", Default::default())
            .unwrap();
        assert_eq!(hits.len(), 1);
    }
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
