//! Pure ranked-search algorithms for sbol-db.
//!
//! This crate is the backend-neutral, dependency-light algorithm layer behind
//! the native (Elasticsearch-free) search that replaces SBOLExplorer. It owns
//! the ranking math and the reference-graph traversal; it depends only on
//! `sbol-db-core` and never on a storage backend, so both the SPARQL
//! accelerator and the search-index rebuild share one implementation of the
//! link graph.
//!
//! [`pagerank`] holds the top-level link graph and the power-iteration PageRank
//! that scores objects. [`keywords`] builds the synthetic keyword field from a
//! display id, its Sequence Ontology role, and its biopax type. [`ranked_text`]
//! is the tantivy ranked-text index that combines a fuzzy multi-field text score
//! with PageRank; it is gated behind the `text-index` feature so the storage
//! core, which reuses only the pure link-graph adjacency here, never builds
//! tantivy. [`align`] holds the k-mer-seeded banded aligner that replaces
//! vsearch and [`cluster`] the cluster identity it feeds; the aligner is gated
//! behind the `align` feature so rust-bio stays out of the storage backends.

pub mod align;
pub mod cluster;
pub mod keywords;
pub mod pagerank;

#[cfg(feature = "text-index")]
pub mod ranked_text;

pub use align::{AlignMode, AlignOptions, Alignment};
pub use cluster::ClusterId;

#[cfg(feature = "align")]
pub use align::align_pair;
#[cfg(feature = "align")]
pub use cluster::cluster_sequences;
