//! SPARQL query and update engine for sbol-db.
//!
//! Queries and updates run against any [`sbol_db_storage::TripleSource`] /
//! [`sbol_db_storage::TripleWriter`], so the engine is independent of the
//! storage backend. `TripleDataset` adapts a `TripleSource` to
//! [`spareval::QueryableDataset`], so queries always see the latest committed
//! state with no second index to operate or rebuild. Queries are parsed with
//! `spargebra` (which only accepts SELECT/CONSTRUCT/ASK/DESCRIBE — Updates fail
//! at parse time) and evaluated with `spareval`.
//!
//! Evaluation runs on `spawn_blocking` because the `QueryableDataset` iterators
//! are synchronous and a `TripleSource` may block while fetching per-pattern
//! rows. Each query is bounded by a timeout and a max-row cap.

mod accel;
mod dataset;
mod engine;
mod error;
mod results;
mod rewrite;
mod update;

pub use dataset::TripleDataset;
pub use engine::{
    parse_query, NativeSparql, ParsedQuery, QueryForm, SparqlEngine, SparqlOptions, SparqlOutcome,
};
pub use error::SparqlError;
pub use results::{
    serialize_boolean, serialize_solutions, serialize_triples, ResultFormat, ResultPayload,
};
pub use update::{SparqlUpdateEngine, UpdateOutcome};
