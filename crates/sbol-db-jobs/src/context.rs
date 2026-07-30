use std::sync::Arc;

use sbol_db_core::JobId;
use sbol_db_search::{ranked_text::RankedTextIndex, VectorIndexMaintainerRegistry};
use sbol_db_storage::{
    ClusterStore, ConfigStore, JobQueue, PageRankStore, SbolStore, SketchStore, TripleSource,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// The handles the `rebuild_search_index` job needs that live outside the
/// [`SbolStore`] surface: the cluster and PageRank persistence, the shared
/// ranked text index, and a synchronous triple source to compute the link graph
/// over.
///
/// Bundling them here keeps `tantivy` and the ranked-text types out of the
/// storage traits: the storage core stays free of the text index, and only a
/// worker that owns the shared index carries this handle. A worker built
/// without it runs every other job kind unchanged; only `rebuild_search_index`
/// requires it.
#[derive(Clone)]
pub struct SearchIndexHandles {
    /// Sequence cluster persistence, replaced wholesale on each rebuild and read
    /// at search time to apply the cluster-duplicate penalty and answer
    /// `/similar`.
    pub cluster: Arc<dyn ClusterStore>,
    /// Object PageRank persistence, replaced wholesale on each rebuild.
    pub pagerank: Arc<dyn PageRankStore>,
    /// The MinHash/LSH similarity sketch index, replaced wholesale on each
    /// rebuild so the sequence-search align path can generate candidates from it.
    pub sketch: Arc<dyn SketchStore>,
    /// The shared ranked text index the rebuild writes and the search adapters
    /// read.
    pub text_index: Arc<RankedTextIndex>,
    /// Synchronous triple reads, the source of the top-level link graph.
    pub triples: Arc<dyn TripleSource>,
}

/// Context handed to every [`crate::JobHandler::run`] invocation. Carries
/// the typed service for domain operations, the job's id (for logging /
/// child enqueues), and the shutdown token the worker drove this task
/// with.
///
/// Handlers should treat the cancellation token as advisory: when it
/// fires, finish quickly and return an error so the worker can re-queue
/// the work. At-least-once semantics + the lease-expiry reaper mean
/// crashing through cancellation is also safe; it just costs more.
#[derive(Clone)]
pub struct JobContext {
    pub job_id: JobId,
    pub worker_id: Arc<str>,
    pub attempt: i32,
    pub service: Arc<dyn SbolStore>,
    pub jobs: Arc<dyn JobQueue>,
    pub cancel: CancellationToken,
    /// Present only on a worker configured with the shared search index. The
    /// `rebuild_search_index` handler requires it; every other handler ignores
    /// it.
    pub search: Option<SearchIndexHandles>,
    /// Present only on a worker configured with an embedding provider and
    /// vector backend. Vector rebuild and incremental-update handlers use this
    /// coordinator to maintain configured generations.
    pub vector_indexes: Option<Arc<VectorIndexMaintainerRegistry>>,
    /// The durable instance-configuration store. Present only on a worker
    /// configured with it; the `wor_sync` handler requires it to read the joined
    /// Web of Registries URL and persist the pulled prefix map, and every other
    /// handler ignores it.
    pub config: Option<Arc<dyn ConfigStore>>,
}

impl JobContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub async fn log(&self, level: &str, message: &str, fields: Value) {
        if let Err(err) = self
            .jobs
            .append_log(self.job_id, Some(self.attempt), level, message, fields)
            .await
        {
            tracing::warn!(error = %err, job_id = %self.job_id, "failed to append job log");
        }
    }
}
