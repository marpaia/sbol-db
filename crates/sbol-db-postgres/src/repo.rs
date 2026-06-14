pub mod document;
pub mod job;
pub mod neighborhood;
pub mod object;
pub mod ontology;
pub mod pg_stats;
pub mod projection;
pub mod projections;
pub mod quad;
pub mod sequence_search;
pub mod validation;

pub use document::{DocumentRepository, ListDocumentsFilter};
pub use job::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobRepository, JobStatus, ListJobsFilter, NewJob,
    OldestQueuedAge, QueueDepthRow, SbolJob, DEFAULT_QUEUE,
};
pub use neighborhood::NeighborhoodRepository;
pub use object::{ListObjectsFilter, SbolObjectRepository};
pub use ontology::{OntologyLoadReport, OntologyRecord, OntologyRepository, OntologyTermRecord};
pub use pg_stats::{
    Activity, BlockingLock, DatabaseSize, IncomingForeignKey, IndexStats, OutgoingForeignKey,
    PgStatsRepository, SlowQuery, TableColumn, TableSchema, TableStats,
};
pub use projection::{ProjectionEvent, ProjectionEventRepository};
pub use projections::{TypedProjectionCounts, TypedProjectionRepository};
pub use quad::{GraphFilter, PatternObject, PatternSubject, QuadRepository};
pub use sequence_search::{
    BatchSequenceMatch, SequenceMatch, SequenceSearchOptions, SequenceSearchRepository,
};
pub use validation::{RecordedValidation, ValidationRepository};

use sbol_db_core::DomainError;

pub(crate) fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}
