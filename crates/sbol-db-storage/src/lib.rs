//! Backend-neutral storage contract for sbol-db.
//!
//! Holds the request/response types that describe what a persistence backend
//! must store and return, independent of how it does so. The Postgres backend
//! (`sbol-db-postgres`) implements this contract; the types here never name a
//! concrete database.

mod accel;
mod capabilities;
mod graph;
mod import;
mod job;
mod lab;
mod lsm;
mod migrate;
mod object;
mod ontology;
mod pagerank;
mod sequence;
mod sql_console;
mod stats;
mod traits;
mod triple;
mod update;

pub use accel::{
    build_accel_index, generate_metadata_rows, generate_rows, integer, AccelIndex, AccelObject,
    AccelSolutions, AcceleratedQuery, FacetKind, Field, LitVal, MetaRecord, Scope, BIOPAX_PREFIX,
    SO_PREFIX,
};
pub use capabilities::{BackendKind, Capabilities, MaintenanceStyle};
pub use graph::ListGraphsFilter;
pub use import::{GraphWriteMode, ImportInput, ImportOverwrite};
pub use job::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobStatus, ListJobsFilter, NewJob, OldestQueuedAge,
    QueueDepthRow, SbolJob, DEFAULT_QUEUE,
};
pub use lab::{ClassCount, CorpusCounts, GraphOverview, GraphTriplesPage, LabStore};
pub use lsm::{ColumnFamilyStats, LevelStats, LsmOverview, LsmStats};
pub use migrate::{MigrationEntry, Migrator};
pub use object::{ListObjectsFilter, TextSearchQuery};
pub use ontology::{OntologyLoadReport, OntologyRecord, OntologyTermRecord};
pub use pagerank::RankRow;
pub use sequence::{BatchSequenceMatch, SequenceAlignment, SequenceMatch, SequenceSearchOptions};
pub use sql_console::{
    SqlConsole, SqlConsoleColumn, SqlExecuteRequest, SqlExecuteResult, SqlValidateError,
};
pub use stats::{
    Activity, BlockingLock, DatabaseSize, DbStats, IncomingForeignKey, IndexStats,
    OutgoingForeignKey, RelationalColumn, RelationalSchema, RelationalTable, SlowQuery,
    TableColumn, TableSchema, TableStats,
};
pub use traits::{
    distinct_graph_iris, distinct_object_iris, AclStore, BlobStore, ClusterStore, ConfigStore,
    GraphStore, JobQueue, NeighborhoodStore, ObjectStore, OntologyStore, PageRankStore, SbolStore,
    SequenceSearchStore, SketchStore, TextSearchStore, TokenStore, TripleSource, TripleWriter,
    UserStore, SBH_CAN_VIEW, SBH_OWNED_BY,
};

/// Re-exported so backends and consumers name the cluster id and similarity
/// sketch through the storage contract without also depending on
/// `sbol-db-search` directly.
pub use sbol_db_search::{ClusterId, Signature};
pub use triple::{
    GraphFilter, IdGraphFilter, IdQuad, PatternObject, PatternSubject, TermId, TermKey, TermValue,
};
pub use update::{TripleChange, UpdateOutcome};
