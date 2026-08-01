use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use axum_server::accept::Accept;

/// Timeout wrapper for ACME's axum acceptor. This caps resources consumed by
/// clients that connect but never complete a TLS handshake.
#[derive(Clone, Debug)]
pub struct TimeoutAcceptor<A> {
    inner: A,
    timeout: Duration,
}

impl<A> TimeoutAcceptor<A> {
    pub(super) fn new(inner: A, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

impl<I, S, A> Accept<I, S> for TimeoutAcceptor<A>
where
    I: Send + 'static,
    S: Send + 'static,
    A: Accept<I, S> + Clone + Send + Sync + 'static,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = A::Stream;
    type Service = A::Service;
    type Future =
        Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send + 'static>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let future = self.inner.accept(stream, service);
        let timeout = self.timeout;
        Box::pin(async move {
            tokio::time::timeout(timeout, future)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))?
        })
    }
}
