//! Qdrant adapter for self-hosted and managed deployments.
//!
//! Search strategies only depend on `sbol-db-search-sdk`; this crate maps that
//! portable contract onto Qdrant collections. Each immutable index generation
//! is a physical collection and the artifact name is a collection alias, so a
//! generation activation or rollback is atomic.

mod backend;
mod filter;

pub use backend::{QdrantRemoteBackend, QdrantRemoteConfig};
