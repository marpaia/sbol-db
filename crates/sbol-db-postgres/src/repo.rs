pub mod accel;
pub mod graph;
pub mod job;
pub mod lab;
pub mod neighborhood;
pub mod object;
pub mod ontology;
pub mod pg_stats;
pub mod projection;
pub mod projections;
pub mod sequence_search;
pub mod sql_console;
pub mod triple;
pub mod validation;

pub use accel::AccelRepository;
pub use graph::GraphRepository;
pub use job::JobRepository;
pub use lab::LabRepository;
pub use neighborhood::NeighborhoodRepository;
pub use object::SbolObjectRepository;
pub use ontology::OntologyRepository;
pub use pg_stats::{
    Activity, BlockingLock, DatabaseSize, IncomingForeignKey, IndexStats, OutgoingForeignKey,
    PgStatsRepository, SlowQuery, TableColumn, TableSchema, TableStats,
};
pub use projection::{ProjectionEvent, ProjectionEventRepository};
pub use projections::{TypedProjectionCounts, TypedProjectionRepository};
pub use sequence_search::SequenceSearchRepository;
pub use sql_console::PgSqlConsole;
pub use triple::TripleRepository;
pub use validation::{RecordedValidation, ValidationRepository};

use sbol_db_core::DomainError;

pub(crate) fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}
