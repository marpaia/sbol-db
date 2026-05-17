//! Postgres-side query cancellation for the lab bench.
//!
//! Long-running queries are protected by two layers: a per-request
//! `statement_timeout` set inside the transaction, and a best-effort
//! cancellation triggered when the client disconnects (axum drops the
//! request future). The latter is what `CancelGuard` implements: on
//! drop without `disarm()`, it sends `pg_cancel_backend($pid)` from a
//! fresh pool connection so the server doesn't sit on a connection
//! slot for the full timeout.
//!
//! This only matters when the handler is `await`ing the query; if the
//! query has already finished, `disarm()` makes the drop a no-op.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sbol_db_postgres::PgPool;
use tokio::runtime::Handle;

/// RAII guard that cancels the backend PID `pid` on the pool `pool`
/// unless explicitly `disarm()`ed before drop. Cheap to construct; the
/// cancel itself runs on a tokio task so `Drop` stays synchronous.
pub struct CancelGuard {
    inner: Arc<Inner>,
}

struct Inner {
    pool: PgPool,
    pid: i32,
    disarmed: AtomicBool,
    handle: Handle,
}

pub fn install(pool: PgPool, pid: i32) -> CancelGuard {
    CancelGuard {
        inner: Arc::new(Inner {
            pool,
            pid,
            disarmed: AtomicBool::new(false),
            handle: Handle::current(),
        }),
    }
}

impl CancelGuard {
    /// Mark the underlying query as cleanly completed; `Drop` becomes a
    /// no-op. Always call this on a successful execute path.
    pub fn disarm(self) {
        self.inner.disarmed.store(true, Ordering::Release);
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.inner.disarmed.load(Ordering::Acquire) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        // Use the runtime handle captured at install time so this works
        // even if the request future was dropped from outside a tokio
        // context.
        inner.handle.clone().spawn(async move {
            let result = sqlx::query("SELECT pg_cancel_backend($1)")
                .bind(inner.pid)
                .execute(&inner.pool)
                .await;
            match result {
                Ok(_) => tracing::debug!(pid = inner.pid, "lab sql query cancel sent"),
                Err(e) => {
                    tracing::warn!(pid = inner.pid, error = %e, "lab sql query cancel failed")
                }
            }
        });
    }
}
