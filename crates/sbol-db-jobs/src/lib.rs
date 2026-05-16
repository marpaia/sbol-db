//! Async job runtime for sbol-db.
//!
//! Jobs are rows in `sbol_jobs`. Workers dequeue with `FOR UPDATE SKIP
//! LOCKED`, run a registered [`JobHandler`] for the row's `kind`, and
//! finalize the lifecycle (success / retry / dead). The runtime is
//! intentionally minimal — no distributed coordination beyond Postgres,
//! no leader election, no broker. Multiple [`Worker`] instances run safely
//! against the same database, including the embedded worker spawned by
//! `sbol-db serve`.

mod context;
mod handler;
pub mod handlers;
mod registry;
mod worker;

pub use context::JobContext;
pub use handler::{ErasedHandler, HandlerError, JobHandler, JobOutcome};
pub use registry::JobRegistry;
pub use worker::{Worker, WorkerConfig};

/// Default registry, suitable for the embedded worker that ships with
/// `sbol-db serve`. Carries every built-in handler. Library consumers
/// building a bespoke registry can call [`JobRegistry::new()`] and
/// register only what they need.
pub fn default_registry() -> JobRegistry {
    JobRegistry::new().register(handlers::ImportDocumentHandler)
}
