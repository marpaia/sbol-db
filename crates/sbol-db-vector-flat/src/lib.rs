//! Exact, in-process vector search for small deployments, tests, and quality
//! evaluation.
//!
//! This backend implements the same generation lifecycle as remote and
//! approximate-nearest-neighbor adapters. It intentionally performs an exact
//! scan: that makes it a useful correctness oracle when comparing Qdrant,
//! pgvector, FAISS, or another ANN implementation.

mod backend;
mod filter;

pub use backend::ExactFlatVectorBackend;
