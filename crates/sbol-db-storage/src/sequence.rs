//! Nucleotide sequence-search inputs and results.

use serde::Serialize;

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

/// One aligned sequence hit, the store-facing result of the banded aligner.
///
/// Carries the target's IRI alongside the alignment the
/// [`align`](sbol_db_search::align) module computes: `percent_match` is the
/// `iddef=2` fractional identity, `strand` is `'+'`/`'-'`, and `cigar` is the
/// M/I/D run-length string over the aligned core. Not `Eq` because it carries an
/// `f64`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SequenceAlignment {
    pub sequence_iri: String,
    pub percent_match: f64,
    pub strand: char,
    pub cigar: String,
    pub score: i32,
}
