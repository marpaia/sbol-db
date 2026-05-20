//! Read-only SPARQL query engine for sbol-db.
//!
//! Postgres is canonical. `PostgresDataset` implements [`spareval::QueryableDataset`]
//! against `sbol_quads`, so queries always see the latest committed state with no
//! second index to operate or rebuild. Queries are parsed with `spargebra` (which
//! only accepts SELECT/CONSTRUCT/ASK/DESCRIBE — Updates fail at parse time) and
//! evaluated with `spareval::SparqlEvaluator`.
//!
//! Evaluation runs on `spawn_blocking` so the sync `QueryableDataset` iterators
//! can do `Handle::block_on` to fetch per-pattern rows from sqlx. Each query is
//! bounded by a timeout and a max-row cap.

mod dataset;
mod engine;
mod error;
mod results;

pub use dataset::PostgresDataset;
pub use engine::{parse_query, ParsedQuery, QueryForm, SparqlEngine, SparqlOptions, SparqlOutcome};
pub use error::SparqlError;
pub use results::{ResultFormat, ResultPayload};
