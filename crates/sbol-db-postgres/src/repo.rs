pub mod accel;
pub mod cluster;
pub mod config;
pub mod graph;
pub mod job;
pub mod lab;
pub mod neighborhood;
pub mod oauth;
pub mod object;
pub mod ontology;
pub mod pagerank;
pub mod pg_stats;
pub mod projection;
pub mod projections;
pub mod sequence_search;
pub mod sketch;
pub mod sql_console;
pub mod tokens;
pub mod triple;
pub mod users;
pub mod validation;

pub use accel::AccelRepository;
pub use cluster::PgClusterStore;
pub use config::PgConfigStore;
pub use graph::GraphRepository;
pub use job::JobRepository;
pub use lab::LabRepository;
pub use neighborhood::NeighborhoodRepository;
pub use oauth::PgOAuthStore;
pub use object::SbolObjectRepository;
pub use ontology::OntologyRepository;
pub use pagerank::PgPageRankStore;
pub use pg_stats::{
    Activity, BlockingLock, DatabaseSize, IncomingForeignKey, IndexStats, OutgoingForeignKey,
    PgStatsRepository, SlowQuery, TableColumn, TableSchema, TableStats,
};
pub use projection::{ProjectionEvent, ProjectionEventRepository};
pub use projections::{TypedProjectionCounts, TypedProjectionRepository};
pub use sequence_search::SequenceSearchRepository;
pub use sketch::PgSketchStore;
pub use sql_console::PgSqlConsole;
pub use tokens::PgTokenStore;
pub use triple::TripleRepository;
pub use users::PgUserStore;
pub use validation::{RecordedValidation, ValidationRepository};

use sbol_db_core::DomainError;

pub(crate) fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}
