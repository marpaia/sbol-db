//! Backend-neutral storage contract for sbol-db.
//!
//! Holds the request/response types that describe what a persistence backend
//! must store and return, independent of how it does so. The Postgres backend
//! (`sbol-db-postgres`) implements this contract; the types here never name a
//! concrete database.

mod accel;
mod graph;
mod import;
mod job;
mod lab;
mod migrate;
mod object;
mod ontology;
mod sequence;
mod stats;
mod traits;
mod triple;
mod update;

pub use accel::{
    build_accel_index, generate_metadata_rows, generate_rows, integer, AccelIndex, AccelObject,
    AccelSolutions, AcceleratedQuery, FacetKind, Field, LitVal, MetaRecord, Scope, BIOPAX_PREFIX,
    SO_PREFIX,
};
pub use graph::ListGraphsFilter;
pub use import::{GraphWriteMode, ImportInput};
pub use job::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobStatus, ListJobsFilter, NewJob, OldestQueuedAge,
    QueueDepthRow, SbolJob, DEFAULT_QUEUE,
};
pub use lab::{ClassCount, CorpusCounts, GraphOverview, GraphTriplesPage, LabStore};
pub use migrate::{MigrationEntry, Migrator};
pub use object::ListObjectsFilter;
pub use ontology::{OntologyLoadReport, OntologyRecord, OntologyTermRecord};
pub use sequence::{BatchSequenceMatch, SequenceMatch, SequenceSearchOptions};
pub use stats::{
    Activity, BlockingLock, DatabaseSize, DbStats, IncomingForeignKey, IndexStats,
    OutgoingForeignKey, SlowQuery, TableColumn, TableSchema, TableStats,
};
pub use traits::{
    GraphStore, JobQueue, NeighborhoodStore, ObjectStore, OntologyStore, SbolStore,
    SequenceSearchStore, TripleSource, TripleWriter,
};
pub use triple::{
    GraphFilter, IdGraphFilter, IdQuad, PatternObject, PatternSubject, TermId, TermKey, TermValue,
};
pub use update::{TripleChange, UpdateOutcome};
