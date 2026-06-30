//! Nucleotide substring + reverse-complement search over `sbol_sequences`,
//! seeded by the canonical 8-mer index `sbol_sequence_kmers`.
//!
//! At query time the first k-mer of the query (and of its reverse complement)
//! seeds a candidate lookup; each candidate is then verified by direct
//! substring match in Rust. Queries shorter than K fall back to a `LIKE` scan.

use sbol_db_core::{
    kmer::{
        canonical, canonical_kmers, encode_kmer, reverse_complement_string, KmerStrand, KMER_K,
    },
    DomainError, SequenceProjection,
};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

use sbol_db_storage::{BatchSequenceMatch, SequenceMatch, SequenceSearchOptions};

use crate::pool::db_err;

/// Rows of `(sequence_iri, kmer, position, strand)` per bulk insert. Four
/// columns per row keeps each batch well under SQLite's bound-variable limit.
const KMER_INSERT_CHUNK: usize = 200;

#[derive(Clone)]
pub struct SequenceSearchRepository {
    pool: SqlitePool,
}

impl SequenceSearchRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Persist a sequence projection and rebuild its k-mer index.
    pub async fn upsert_sequence(
        &self,
        conn: &mut SqliteConnection,
        p: &SequenceProjection,
    ) -> Result<(), DomainError> {
        let alphabet = p.alphabet.map(|a| a.as_db_str());
        sqlx::query(
            "INSERT INTO sbol_sequences (iri, encoding_iri, elements, alphabet, content_hash) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(iri) DO UPDATE SET \
                 encoding_iri = excluded.encoding_iri, \
                 elements = excluded.elements, \
                 alphabet = excluded.alphabet, \
                 content_hash = excluded.content_hash",
        )
        .bind(p.iri.as_str())
        .bind(p.encoding_iri.as_ref().map(|i| i.as_str().to_owned()))
        .bind(p.elements.as_deref())
        .bind(alphabet)
        .bind(p.content_hash.as_deref())
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

        self.reindex(conn, p.iri.as_str(), p.elements.as_deref(), alphabet)
            .await
    }

    /// Rebuild the k-mer index for one sequence. Idempotent. Skips non-DNA/RNA,
    /// empty, or sub-K sequences.
    pub async fn reindex(
        &self,
        conn: &mut SqliteConnection,
        iri: &str,
        elements: Option<&str>,
        alphabet: Option<&str>,
    ) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM sbol_sequence_kmers WHERE sequence_iri = ?")
            .bind(iri)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;

        let Some(elements) = elements else {
            return Ok(());
        };
        if !matches!(alphabet, Some("DNA") | Some("RNA")) {
            return Ok(());
        }
        let normalised: String = elements.chars().filter(|c| !c.is_whitespace()).collect();
        if normalised.len() < KMER_K {
            return Ok(());
        }
        let hits: Vec<_> = canonical_kmers(&normalised).collect();
        for chunk in hits.chunks(KMER_INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO sbol_sequence_kmers (sequence_iri, kmer, position, strand) ",
            );
            qb.push_values(chunk, |mut row, h| {
                row.push_bind(iri)
                    .push_bind(h.canonical as i64)
                    .push_bind(h.position as i64)
                    .push_bind(h.strand.as_char().to_string());
            });
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
        }
        Ok(())
    }

    pub async fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError> {
        let max = options.max_hits.unwrap_or(1024) as usize;
        if max == 0 {
            return Ok(Vec::new());
        }
        let q_upper: String = pattern
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if q_upper.is_empty() {
            return Ok(Vec::new());
        }
        let q_rc = reverse_complement_string(&q_upper).to_ascii_uppercase();
        let include_rc = !matches!(options.forward_only, Some(true));

        let candidates = if q_upper.len() < KMER_K {
            self.candidates_short(&q_upper, &q_rc, include_rc).await?
        } else {
            self.candidates_seeded(&q_upper, &q_rc, include_rc).await?
        };

        let mut matches = Vec::new();
        'outer: for (iri, elements) in candidates {
            let up = elements.to_ascii_uppercase();
            for (start, _) in up.match_indices(q_upper.as_str()) {
                matches.push(SequenceMatch {
                    sequence_iri: iri.clone(),
                    start: start as i32,
                    length: q_upper.len() as i32,
                    strand: KmerStrand::Forward.as_char(),
                });
                if matches.len() >= max {
                    break 'outer;
                }
            }
            if include_rc && q_upper != q_rc {
                for (start, _) in up.match_indices(q_rc.as_str()) {
                    matches.push(SequenceMatch {
                        sequence_iri: iri.clone(),
                        start: start as i32,
                        length: q_rc.len() as i32,
                        strand: KmerStrand::Reverse.as_char(),
                    });
                    if matches.len() >= max {
                        break 'outer;
                    }
                }
            }
        }
        Ok(matches)
    }

    pub async fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError> {
        let mut out = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let matches = self.search(pattern, options.clone()).await?;
            out.push(BatchSequenceMatch {
                pattern: pattern.clone(),
                matches,
            });
        }
        Ok(out)
    }

    async fn candidates_seeded(
        &self,
        q_upper: &str,
        q_rc: &str,
        include_rc: bool,
    ) -> Result<Vec<(String, String)>, DomainError> {
        let mut seeds: Vec<i64> = Vec::new();
        if let Some(packed) = encode_kmer(&q_upper[..KMER_K]) {
            seeds.push(canonical(packed).0 as i64);
        }
        if include_rc && q_upper != q_rc {
            if let Some(packed) = encode_kmer(&q_rc[..KMER_K]) {
                let c = canonical(packed).0 as i64;
                if !seeds.contains(&c) {
                    seeds.push(c);
                }
            }
        }
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT s.iri AS iri, s.elements AS elements FROM sbol_sequences s \
             WHERE s.elements IS NOT NULL AND s.alphabet IN ('DNA', 'RNA') \
             AND s.iri IN (SELECT DISTINCT sequence_iri FROM sbol_sequence_kmers WHERE kmer IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for seed in &seeds {
                sep.push_bind(*seed);
            }
        }
        qb.push("))");
        let rows = qb.build().fetch_all(&self.pool).await.map_err(db_err)?;
        collect_candidates(rows)
    }

    async fn candidates_short(
        &self,
        q_upper: &str,
        q_rc: &str,
        include_rc: bool,
    ) -> Result<Vec<(String, String)>, DomainError> {
        let rows = if include_rc && q_upper != q_rc {
            sqlx::query(
                "SELECT iri, elements FROM sbol_sequences \
                 WHERE elements IS NOT NULL AND alphabet IN ('DNA', 'RNA') \
                 AND (UPPER(elements) LIKE '%' || ?1 || '%' OR UPPER(elements) LIKE '%' || ?2 || '%')",
            )
            .bind(q_upper)
            .bind(q_rc)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
        } else {
            sqlx::query(
                "SELECT iri, elements FROM sbol_sequences \
                 WHERE elements IS NOT NULL AND alphabet IN ('DNA', 'RNA') \
                 AND UPPER(elements) LIKE '%' || ?1 || '%'",
            )
            .bind(q_upper)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
        };
        collect_candidates(rows)
    }
}

fn collect_candidates(
    rows: Vec<sqlx::sqlite::SqliteRow>,
) -> Result<Vec<(String, String)>, DomainError> {
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("iri").map_err(db_err)?,
                row.try_get::<String, _>("elements").map_err(db_err)?,
            ))
        })
        .collect()
}
