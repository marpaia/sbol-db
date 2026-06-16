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
