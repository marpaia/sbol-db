//! Nucleotide substring + reverse-complement search over `sbol_sequences`.
//!
//! The seed index is `sequence_kmers`: one row per observed canonical
//! 8-mer position. At query time we pick the first k-mer of the query and
//! of its reverse complement, ask Postgres for the candidate sequence ids,
//! then verify each candidate by direct substring match on
//! `sbol_sequences.elements` in Rust. Queries shorter than K fall back to
//! an `ILIKE` candidate scan (no seed possible at that length).
//!
//! Indexing is invoked from [`crate::repo::projections::upsert_sequence`]
//! during import so the seed index stays in lockstep with the typed
//! projection.

use sbol_db_core::{
    kmer::{
        canonical, canonical_kmers, encode_kmer, reverse_complement_string, KmerStrand, KMER_K,
    },
    DomainError,
};
use serde::Serialize;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct SequenceSearchRepository {
    pool: PgPool,
}

#[derive(Clone, Debug, Default)]
pub struct SequenceSearchOptions {
    pub max_hits: Option<u32>,
    /// When `Some(false)`, restrict the match to the forward strand only.
    /// Default (`None`) is reverse-complement-aware.
    pub forward_only: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SequenceMatch {
    pub sequence_iri: String,
    pub start: i32,
    pub length: i32,
    pub strand: char,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BatchSequenceMatch {
    pub pattern: String,
    pub matches: Vec<SequenceMatch>,
}

impl SequenceSearchRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Rebuild the k-mer index for one sequence. Idempotent: deletes all
    /// existing rows then inserts the current set. Skips sequences whose
    /// `alphabet` is not DNA/RNA, whose `elements` is empty, or whose length
    /// is shorter than `K`.
    pub async fn reindex(
        &self,
        conn: &mut PgConnection,
        object_id: Uuid,
        elements: Option<&str>,
        alphabet: Option<&str>,
    ) -> Result<usize, DomainError> {
        reindex_kmers(conn, object_id, elements, alphabet).await
    }

    /// Exact substring search with reverse-complement awareness. Returns
    /// every (sequence, start, strand) where the pattern (or its reverse
    /// complement) appears in `sbol_sequences.elements`.
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
        let q_rc: String = reverse_complement_string(&q_upper).to_ascii_uppercase();
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

    /// Run [`search`](Self::search) for each pattern. Patterns are independent;
    /// the implementation is a simple loop. Use this to amortise HTTP / CLI
    /// round-trips when scanning N motifs against the same corpus.
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
        let mut seeds: Vec<i32> = Vec::new();
        if let Some(packed) = encode_kmer(&q_upper[..KMER_K]) {
            seeds.push(canonical(packed).0 as i32);
        }
        if include_rc && q_upper != q_rc {
            if let Some(packed) = encode_kmer(&q_rc[..KMER_K]) {
                let c = canonical(packed).0 as i32;
                if !seeds.contains(&c) {
                    seeds.push(c);
                }
            }
        }
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            r#"
            SELECT s.iri::text AS iri, s.elements AS elements
            FROM sbol_sequences s
            WHERE s.elements IS NOT NULL
              AND s.alphabet IN ('DNA', 'RNA')
              AND s.object_id IN (
                SELECT DISTINCT sequence_object_id FROM sequence_kmers
                WHERE kmer = ANY($1::int[])
              )
            "#,
        )
        .bind(&seeds)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let iri: String = row.try_get("iri").map_err(db_err)?;
            let elements: String = row.try_get("elements").map_err(db_err)?;
            out.push((iri, elements));
        }
        Ok(out)
    }

    async fn candidates_short(
        &self,
        q_upper: &str,
        q_rc: &str,
        include_rc: bool,
    ) -> Result<Vec<(String, String)>, DomainError> {
        let sql = if include_rc && q_upper != q_rc {
            r#"
            SELECT iri::text AS iri, elements
            FROM sbol_sequences
            WHERE elements IS NOT NULL
              AND alphabet IN ('DNA', 'RNA')
              AND (UPPER(elements) LIKE '%' || $1 || '%'
                   OR UPPER(elements) LIKE '%' || $2 || '%')
            "#
        } else {
            r#"
            SELECT iri::text AS iri, elements
            FROM sbol_sequences
            WHERE elements IS NOT NULL
              AND alphabet IN ('DNA', 'RNA')
              AND UPPER(elements) LIKE '%' || $1 || '%'
            "#
        };
        let q = sqlx::query(sql).bind(q_upper);
        let q = if include_rc && q_upper != q_rc {
            q.bind(q_rc)
        } else {
            q
        };
        let rows = q.fetch_all(&self.pool).await.map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let iri: String = row.try_get("iri").map_err(db_err)?;
            let elements: String = row.try_get("elements").map_err(db_err)?;
            out.push((iri, elements));
        }
        Ok(out)
    }
}

/// Free-function form of the k-mer reindex, callable from
/// `TypedProjectionRepository::upsert_sequence` without holding a repository
/// instance. The caller passes the pooled connection used for the rest of
/// the import transaction.
pub(crate) async fn reindex_kmers(
    conn: &mut PgConnection,
    object_id: Uuid,
    elements: Option<&str>,
    alphabet: Option<&str>,
) -> Result<usize, DomainError> {
    sqlx::query("DELETE FROM sequence_kmers WHERE sequence_object_id = $1")
        .bind(object_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    let Some(elements) = elements else {
        return Ok(0);
    };
    if !matches!(alphabet, Some("DNA") | Some("RNA")) {
        return Ok(0);
    }
    let normalised: String = elements.chars().filter(|c| !c.is_whitespace()).collect();
    if normalised.len() < KMER_K {
        return Ok(0);
    }

    let hits: Vec<_> = canonical_kmers(&normalised).collect();
    if hits.is_empty() {
        return Ok(0);
    }

    // Bulk insert via UNNEST -- one round trip even for long sequences.
    let mut kmers: Vec<i32> = Vec::with_capacity(hits.len());
    let mut positions: Vec<i32> = Vec::with_capacity(hits.len());
    let mut strands: Vec<String> = Vec::with_capacity(hits.len());
    for h in &hits {
        kmers.push(h.canonical as i32);
        positions.push(h.position as i32);
        strands.push(h.strand.as_char().to_string());
    }

    sqlx::query(
        r#"
        INSERT INTO sequence_kmers (sequence_object_id, kmer, position, strand)
        SELECT $1, k, p, s
        FROM UNNEST($2::int[], $3::int[], $4::text[]) AS u(k, p, s)
        "#,
    )
    .bind(object_id)
    .bind(&kmers)
    .bind(&positions)
    .bind(&strands)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;

    Ok(hits.len())
}
