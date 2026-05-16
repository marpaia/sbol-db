use async_trait::async_trait;
use sbol_db_core::DomainError;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

use crate::context::JobContext;

/// Result a handler returns into the queue. The opaque JSON `result` is
/// persisted in `sbol_jobs.result` so callers polling the job get a
/// typed payload back.
#[derive(Clone, Debug, Default)]
pub struct JobOutcome {
    pub result: Option<Value>,
}

impl JobOutcome {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_result(result: Value) -> Self {
        Self {
            result: Some(result),
        }
    }
}

#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("invalid job payload: {0}")]
    InvalidPayload(String),

    #[error("{0}")]
    Domain(#[from] DomainError),

    #[error("{0}")]
    Other(String),
}

impl From<serde_json::Error> for HandlerError {
    fn from(e: serde_json::Error) -> Self {
        HandlerError::InvalidPayload(e.to_string())
    }
}

/// One typed job handler. Implementors pick the payload shape and the
/// runtime decodes the row's JSON payload before calling `run`.
#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    type Payload: DeserializeOwned + Send + Sync;

    /// Stable identifier persisted as `sbol_jobs.kind`. Used to dispatch
    /// rows to the right handler in the registry.
    fn kind(&self) -> &'static str;

    async fn run(
        &self,
        ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError>;
}

/// Boxed, type-erased shape stored in the [`crate::JobRegistry`]. Each
/// concrete [`JobHandler`] is wrapped in a `TypedHandler<H>` that decodes
/// the row's JSON payload before delegating.
#[async_trait]
pub trait ErasedHandler: Send + Sync + 'static {
    fn kind(&self) -> &'static str;
    async fn dispatch(&self, ctx: JobContext, payload: Value) -> Result<JobOutcome, HandlerError>;
}

pub(crate) struct TypedHandler<H: JobHandler> {
    pub inner: H,
}

#[async_trait]
impl<H: JobHandler> ErasedHandler for TypedHandler<H> {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    async fn dispatch(&self, ctx: JobContext, payload: Value) -> Result<JobOutcome, HandlerError> {
        let parsed: H::Payload = serde_json::from_value(payload)?;
        self.inner.run(ctx, parsed).await
    }
}
