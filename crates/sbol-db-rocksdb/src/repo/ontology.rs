//! Ontology storage over RocksDB.
//!
//! Loading reuses the pure [`build_ontology_plan`] and replaces a prefix's
//! prior terms, aliases, and closure atomically. The transitive closure is
//! keyed `ancestor ++ depth ++ descendant` so `descendants` is one prefix scan
//! already ordered by depth then descendant. Per-prefix "idx" families let a
//! reload enumerate and drop a prefix's rows without a full scan.

use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use sbol_db_core::DomainError;
use sbol_db_derive::build_ontology_plan;
use sbol_db_storage::{OntologyLoadReport, OntologyRecord, OntologyTermRecord};
use serde::{Deserialize, Serialize};

use crate::db::{compose, Db, SEP};

#[derive(Clone)]
pub struct OntologyRepository {
    db: Db,
}

#[derive(Serialize, Deserialize)]
struct OntRec {
    name: String,
    source_url: Option<String>,
    version: Option<String>,
    term_count: i64,
    imported_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct OntTerm {
    prefix: String,
    curie: String,
    name: String,
    definition: Option<String>,
    is_obsolete: bool,
    synonyms: Vec<String>,
}

impl OntologyRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn load_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let plan = build_ontology_plan(prefix, name, source_url, text);
        let p = plan.prefix.as_str();
        let mut batch = WriteBatch::default();

        self.stage_drop_prefix(&mut batch, p)?;

        let rec = OntRec {
            name: plan.name.clone(),
            source_url: plan.source_url.clone(),
            version: plan.version.clone(),
            term_count: plan.terms.len() as i64,
            imported_at: Utc::now(),
        };
        batch.put_cf(&self.db.cf("ont"), p.as_bytes(), encode(&rec)?);

        for term in &plan.terms {
            let stored = OntTerm {
                prefix: p.to_owned(),
                curie: term.curie.clone(),
                name: term.name.clone(),
                definition: term.definition.clone(),
                is_obsolete: term.is_obsolete,
                synonyms: term.synonyms.clone(),
            };
            batch.put_cf(
                &self.db.cf("ont_term"),
                term.canonical_iri.as_bytes(),
                encode(&stored)?,
            );
            batch.put_cf(
                &self.db.cf("ont_term_idx"),
                compose(&[
                    p.as_bytes(),
                    term.curie.as_bytes(),
                    term.canonical_iri.as_bytes(),
                ]),
                [],
            );
        }

        for (alias, canonical) in &plan.aliases {
            batch.put_cf(
                &self.db.cf("ont_alias"),
                alias.as_bytes(),
                canonical.as_bytes(),
            );
            batch.put_cf(
                &self.db.cf("ont_alias_idx"),
                compose(&[p.as_bytes(), alias.as_bytes()]),
                [],
            );
        }

        for (ancestor, descendant, depth) in &plan.closure {
            let dk = depth_key(*depth);
            batch.put_cf(
                &self.db.cf("ont_closure"),
                compose(&[ancestor.as_bytes(), &dk, descendant.as_bytes()]),
                [],
            );
            batch.put_cf(
                &self.db.cf("ont_closure_idx"),
                compose(&[
                    p.as_bytes(),
                    ancestor.as_bytes(),
                    &dk,
                    descendant.as_bytes(),
                ]),
                [],
            );
        }

        self.db.write(batch)?;
        Ok(plan.report())
    }

    /// Stage deletion of every row a prefix owns, via its "idx" enumeration
    /// families.
    fn stage_drop_prefix(&self, batch: &mut WriteBatch, prefix: &str) -> Result<(), DomainError> {
        let p = compose(&[prefix.as_bytes(), b""]); // "PREFIX\x1f"
        let p = p.as_slice();

        // Terms.
        let mut term_keys: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        self.db.for_each_prefix("ont_term_idx", p, |key, _| {
            let canonical = last_field(key);
            term_keys.push((key.to_vec(), canonical.to_vec()));
            Ok(true)
        })?;
        for (idx_key, canonical) in term_keys {
            batch.delete_cf(&self.db.cf("ont_term"), &canonical);
            batch.delete_cf(&self.db.cf("ont_term_idx"), &idx_key);
        }

        // Aliases.
        let mut alias_keys: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        self.db.for_each_prefix("ont_alias_idx", p, |key, _| {
            let alias = last_field(key);
            alias_keys.push((key.to_vec(), alias.to_vec()));
            Ok(true)
        })?;
        for (idx_key, alias) in alias_keys {
            batch.delete_cf(&self.db.cf("ont_alias"), &alias);
            batch.delete_cf(&self.db.cf("ont_alias_idx"), &idx_key);
        }

        // Closure: the idx key is `prefix ++ closurekey`; strip the prefix to
        // recover the primary closure key.
        let mut closure_keys: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        self.db.for_each_prefix("ont_closure_idx", p, |key, _| {
            let primary = key[p.len()..].to_vec();
            closure_keys.push((key.to_vec(), primary));
            Ok(true)
        })?;
        for (idx_key, primary) in closure_keys {
            batch.delete_cf(&self.db.cf("ont_closure"), &primary);
            batch.delete_cf(&self.db.cf("ont_closure_idx"), &idx_key);
        }

        batch.delete_cf(&self.db.cf("ont"), prefix.as_bytes());
        Ok(())
    }

    pub fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        let mut out = Vec::new();
        self.db.for_each("ont", |key, blob| {
            let prefix = String::from_utf8(key.to_vec())
                .map_err(|_| DomainError::Database("non-utf8 ontology prefix".into()))?;
            let rec: OntRec = decode(blob)?;
            out.push(OntologyRecord {
                prefix,
                name: rec.name,
                source_url: rec.source_url,
                version: rec.version,
                term_count: rec.term_count as i32,
                imported_at: rec.imported_at,
            });
            Ok(true)
        })?;
        out.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        Ok(out)
    }

    pub fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        if self.db.exists_cf("ont_term", iri.as_bytes())? {
            return Ok(Some(iri.to_owned()));
        }
        match self.db.get_cf("ont_alias", iri.as_bytes())? {
            Some(bytes) => {
                Ok(Some(String::from_utf8(bytes).map_err(|_| {
                    DomainError::Database("non-utf8 canonical iri".into())
                })?))
            }
            None => Ok(None),
        }
    }

    pub fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        let canonical = self.canonicalize(iri)?.unwrap_or_else(|| iri.to_owned());
        let prefix = compose(&[canonical.as_bytes(), b""]); // "canonical\x1f"
        let prefix = prefix.as_slice();
        let mut out = Vec::new();
        self.db.for_each_prefix("ont_closure", prefix, |key, _| {
            // key = canonical ++ SEP ++ depth(2) ++ SEP ++ descendant
            let rest = &key[prefix.len()..];
            let depth = i16::from_be_bytes([rest[0], rest[1]]);
            let descendant = std::str::from_utf8(&rest[3..])
                .map_err(|_| DomainError::Database("non-utf8 descendant iri".into()))?;
            out.push((descendant.to_owned(), depth));
            Ok(true)
        })?;
        Ok(out)
    }

    pub fn list_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        let prefix = prefix.to_uppercase();
        let needle = search.map(|s| s.trim().to_lowercase());
        let scan_prefix = compose(&[prefix.as_bytes(), b""]);

        let mut matched: Vec<String> = Vec::new();
        self.db
            .for_each_prefix("ont_term_idx", &scan_prefix, |key, _| {
                let canonical = std::str::from_utf8(last_field(key))
                    .map_err(|_| DomainError::Database("non-utf8 canonical iri".into()))?
                    .to_owned();
                if let Some(n) = &needle {
                    if let Some(term) = self.get_term_raw(&canonical)? {
                        if !term.curie.to_lowercase().contains(n)
                            && !term.name.to_lowercase().contains(n)
                        {
                            return Ok(true);
                        }
                    } else {
                        return Ok(true);
                    }
                }
                matched.push(canonical);
                Ok(true)
            })?;

        let total = matched.len() as i64;
        let mut out = Vec::new();
        for canonical in matched
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
        {
            if let Some(record) = self.get_term(&canonical)? {
                out.push(record);
            }
        }
        Ok((out, total))
    }

    pub fn get_term(&self, iri: &str) -> Result<Option<OntologyTermRecord>, DomainError> {
        let canonical = match self.canonicalize(iri)? {
            Some(c) => c,
            None => return Ok(None),
        };
        match self.get_term_raw(&canonical)? {
            Some(term) => Ok(Some(OntologyTermRecord {
                iri: canonical,
                prefix: term.prefix,
                curie: term.curie,
                name: term.name,
                definition: term.definition,
                is_obsolete: term.is_obsolete,
                synonyms: term.synonyms,
            })),
            None => Ok(None),
        }
    }

    fn get_term_raw(&self, canonical_iri: &str) -> Result<Option<OntTerm>, DomainError> {
        match self.db.get_cf("ont_term", canonical_iri.as_bytes())? {
            Some(blob) => Ok(Some(decode(&blob)?)),
            None => Ok(None),
        }
    }
}

/// Two-byte big-endian depth; depths are small and non-negative, so unsigned BE
/// sorts ascending.
fn depth_key(depth: i16) -> [u8; 2] {
    (depth.max(0) as u16).to_be_bytes()
}

/// The last `SEP`-delimited field of a composite key.
fn last_field(key: &[u8]) -> &[u8] {
    match key.iter().rposition(|&b| b == SEP) {
        Some(i) => &key[i + 1..],
        None => key,
    }
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    serde_json::to_vec(value).map_err(|e| DomainError::Serialization(e.to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(blob: &[u8]) -> Result<T, DomainError> {
    serde_json::from_slice(blob).map_err(|e| DomainError::Serialization(e.to_string()))
}
