//! Nucleotide sequence search over RocksDB.
//!
//! Sequences are stored by IRI; a k-mer seed index (`seq_kmer`, keyed
//! `kmer ++ iri`) narrows long queries to candidate sequences, which are then
//! verified by direct substring match on both strands. Short queries (under
//! the k-mer width) fall back to a full scan. A per-IRI mirror family
//! (`seq_kmer_by_iri`) lets a re-index drop a sequence's old seeds.

use std::collections::HashSet;

use rocksdb::WriteBatch;
use sbol_db_core::kmer::{
    canonical, canonical_kmers, encode_kmer, reverse_complement_string, KMER_K,
};
use sbol_db_core::{DomainError, SequenceProjection};
use sbol_db_storage::{BatchSequenceMatch, SequenceMatch, SequenceSearchOptions};
use serde::{Deserialize, Serialize};

use crate::db::{Db, SEP};

const DEFAULT_MAX_HITS: usize = 1024;

#[derive(Clone)]
pub struct SequenceSearchRepository {
    db: Db,
}

#[derive(Serialize, Deserialize)]
struct SeqRec {
    encoding_iri: Option<String>,
    elements: Option<String>,
    alphabet: Option<String>,
    content_hash: Option<Vec<u8>>,
}

impl SequenceSearchRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn stage_upsert(
        &self,
        batch: &mut WriteBatch,
        p: &SequenceProjection,
    ) -> Result<(), DomainError> {
        let iri = p.iri.as_str();
        let alphabet = p.alphabet.map(|a| a.as_db_str().to_owned());

        self.stage_drop_kmers(batch, iri)?;

        let rec = SeqRec {
            encoding_iri: p.encoding_iri.as_ref().map(|i| i.as_str().to_owned()),
            elements: p.elements.clone(),
            alphabet: alphabet.clone(),
            content_hash: p.content_hash.clone(),
        };
        batch.put_cf(
            &self.db.cf("seq"),
            iri.as_bytes(),
            serde_json::to_vec(&rec).map_err(|e| DomainError::Serialization(e.to_string()))?,
        );

        if let (Some(elements), Some(alpha)) = (&p.elements, &alphabet) {
            if alpha == "DNA" || alpha == "RNA" {
                let normalized: String = elements.split_whitespace().collect();
                if normalized.len() >= KMER_K {
                    let mut seen = HashSet::new();
                    for hit in canonical_kmers(&normalized) {
                        if !seen.insert(hit.canonical) {
                            continue;
                        }
                        let kb = hit.canonical.to_be_bytes();
                        batch.put_cf(&self.db.cf("seq_kmer"), kmer_key(&kb, iri), []);
                        batch.put_cf(&self.db.cf("seq_kmer_by_iri"), by_iri_key(iri, &kb), []);
                    }
                }
            }
        }
        Ok(())
    }

    fn stage_drop_kmers(&self, batch: &mut WriteBatch, iri: &str) -> Result<(), DomainError> {
        let mut prefix = iri.as_bytes().to_vec();
        prefix.push(SEP);
        let mut keys: Vec<[u8; 4]> = Vec::new();
        self.db
            .for_each_prefix("seq_kmer_by_iri", &prefix, |key, _| {
                let kb = &key[key.len() - 4..];
                keys.push([kb[0], kb[1], kb[2], kb[3]]);
                Ok(true)
            })?;
        for kb in keys {
            batch.delete_cf(&self.db.cf("seq_kmer"), kmer_key(&kb, iri));
            batch.delete_cf(&self.db.cf("seq_kmer_by_iri"), by_iri_key(iri, &kb));
        }
        Ok(())
    }

    pub fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError> {
        let max = options
            .max_hits
            .map(|m| m as usize)
            .unwrap_or(DEFAULT_MAX_HITS);
        if max == 0 {
            return Ok(Vec::new());
        }
        let q_upper: String = pattern
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_uppercase();
        if q_upper.is_empty() {
            return Ok(Vec::new());
        }
        let q_rc = reverse_complement_string(&q_upper).to_uppercase();
        let include_rc = !matches!(options.forward_only, Some(true));

        let candidates = if q_upper.len() < KMER_K {
            self.candidates_short(&q_upper, &q_rc, include_rc)?
        } else {
            self.candidates_seeded(&q_upper, &q_rc, include_rc)?
        };

        let mut matches = Vec::new();
        'outer: for (iri, elements) in candidates {
            let up = elements.to_uppercase();
            for (start, _) in up.match_indices(&q_upper) {
                matches.push(SequenceMatch {
                    sequence_iri: iri.clone(),
                    start: start as i32,
                    length: q_upper.len() as i32,
                    strand: '+',
                });
                if matches.len() >= max {
                    break 'outer;
                }
            }
            if include_rc && q_upper != q_rc {
                for (start, _) in up.match_indices(&q_rc) {
                    matches.push(SequenceMatch {
                        sequence_iri: iri.clone(),
                        start: start as i32,
                        length: q_rc.len() as i32,
                        strand: '-',
                    });
                    if matches.len() >= max {
                        break 'outer;
                    }
                }
            }
        }
        Ok(matches)
    }

    pub fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError> {
        let mut out = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            out.push(BatchSequenceMatch {
                pattern: pattern.clone(),
                matches: self.search(pattern, options.clone())?,
            });
        }
        Ok(out)
    }

    /// Long-query path: seed the first k-mer of each strand, gather the
    /// sequences whose seed index contains it.
    fn candidates_seeded(
        &self,
        q_upper: &str,
        q_rc: &str,
        include_rc: bool,
    ) -> Result<Vec<(String, String)>, DomainError> {
        let mut seeds: Vec<u32> = Vec::new();
        if let Some(k) = encode_kmer(&q_upper[..KMER_K]) {
            seeds.push(canonical(k).0);
        }
        if include_rc && q_rc != q_upper {
            if let Some(k) = encode_kmer(&q_rc[..KMER_K]) {
                seeds.push(canonical(k).0);
            }
        }
        seeds.sort_unstable();
        seeds.dedup();

        let mut iris: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for seed in seeds {
            let prefix = seed.to_be_bytes();
            self.db.for_each_prefix("seq_kmer", &prefix, |key, _| {
                let iri = std::str::from_utf8(&key[4..])
                    .map_err(|_| DomainError::Database("non-utf8 sequence iri".into()))?;
                if seen.insert(iri.to_owned()) {
                    iris.push(iri.to_owned());
                }
                Ok(true)
            })?;
        }
        self.collect_candidates(iris)
    }

    /// Short-query path: scan every nucleotide sequence (no usable seed).
    fn candidates_short(
        &self,
        q_upper: &str,
        q_rc: &str,
        include_rc: bool,
    ) -> Result<Vec<(String, String)>, DomainError> {
        let mut out = Vec::new();
        self.db.for_each("seq", |key, blob| {
            let rec: SeqRec = serde_json::from_slice(blob)
                .map_err(|e| DomainError::Serialization(e.to_string()))?;
            let (Some(elements), Some(alpha)) = (rec.elements, rec.alphabet) else {
                return Ok(true);
            };
            if alpha != "DNA" && alpha != "RNA" {
                return Ok(true);
            }
            let up = elements.to_uppercase();
            let hit = up.contains(q_upper) || (include_rc && q_upper != q_rc && up.contains(q_rc));
            if hit {
                let iri = String::from_utf8(key.to_vec())
                    .map_err(|_| DomainError::Database("non-utf8 sequence iri".into()))?;
                out.push((iri, elements));
            }
            Ok(true)
        })?;
        Ok(out)
    }

    fn collect_candidates(&self, iris: Vec<String>) -> Result<Vec<(String, String)>, DomainError> {
        let mut out = Vec::new();
        for iri in iris {
            if let Some(blob) = self.db.get_cf("seq", iri.as_bytes())? {
                let rec: SeqRec = serde_json::from_slice(&blob)
                    .map_err(|e| DomainError::Serialization(e.to_string()))?;
                if let (Some(elements), Some(alpha)) = (rec.elements, rec.alphabet) {
                    if alpha == "DNA" || alpha == "RNA" {
                        out.push((iri, elements));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// `kmer (4 bytes) ++ iri` — fixed-width seed prefix for the candidate scan.
fn kmer_key(kmer: &[u8; 4], iri: &str) -> Vec<u8> {
    let mut key = kmer.to_vec();
    key.extend_from_slice(iri.as_bytes());
    key
}

/// `iri ++ SEP ++ kmer (4 bytes)` — per-sequence mirror for re-index deletes.
fn by_iri_key(iri: &str, kmer: &[u8; 4]) -> Vec<u8> {
    let mut key = iri.as_bytes().to_vec();
    key.push(SEP);
    key.extend_from_slice(kmer);
    key
}
