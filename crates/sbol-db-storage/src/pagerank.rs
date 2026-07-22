//! Persistence types for object PageRank scores.
//!
//! A backend stores one `(iri, score)` pair per ranked top-level object in an
//! `object_pagerank` table (or column family). The scores are recomputed
//! wholesale by the search-index rebuild and replaced atomically, so a read
//! never sees a half-updated ranking.

/// One object's stored PageRank score.
#[derive(Clone, Debug, PartialEq)]
pub struct RankRow {
    pub iri: String,
    pub score: f64,
}
