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
//! tantivy.

pub mod keywords;
pub mod pagerank;

#[cfg(feature = "text-index")]
pub mod ranked_text;
