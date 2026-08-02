use std::io;
use std::sync::Arc;

use anyhow::{bail, Result};
use futures::StreamExt;
use rustls_acme::{AcmeState, EventError, EventOk};
use tokio_util::sync::CancellationToken;

use super::cache::CertificateState;

/// Poll ACME forever. rustls-acme performs issuance and renewal only while its
/// state stream is being polled, so this task is a required part of the server
/// lifecycle rather than detached best-effort work.
pub async fn run_acme(
    mut state: AcmeState<io::Error>,
    certificate_state: CertificateState,
    metrics: Arc<sbol_db_server::Metrics>,
    cancel: CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            event = state.next() => {
                let Some(event) = event else {
                    bail!("ACME lifecycle stream ended unexpectedly");
                };
                match event {
                    Ok(event) => {
                        let kind = event_ok_label(&event);
                        metrics::counter!("sbol_db_acme_events_total", "event" => kind, "result" => "success")
                            .increment(1);
                        match event {
                            EventOk::DeployedCachedCert => {
                                metrics.mark_tls_ready(certificate_state.not_after_unix()?);
                                tracing::info!(event = kind, "ACME certificate deployed");
                            }
                            EventOk::DeployedNewCert => {
                                tracing::info!(
                                    event = kind,
                                    "ACME certificate deployed; waiting for durable cache"
                                );
                            }
                            EventOk::CertCacheStore => {
                                metrics.mark_tls_ready(certificate_state.not_after_unix()?);
                                tracing::info!(event = kind, "ACME certificate persisted and ready");
                            }
                            EventOk::AccountCacheStore => {
                                tracing::info!(event = kind, "ACME state persisted");
                            }
                        }
                    }
                    Err(error) => {
                        let kind = event_error_label(&error);
                        metrics::counter!("sbol_db_acme_events_total", "event" => kind, "result" => "error")
                            .increment(1);
                        tracing::error!(event = kind, error = %error, "ACME lifecycle event failed");
                    }
                }
            }
        }
    }
}

fn event_ok_label(event: &EventOk) -> &'static str {
    match event {
        EventOk::DeployedCachedCert => "certificate_deployed_cached",
        EventOk::DeployedNewCert => "certificate_deployed_new",
        EventOk::CertCacheStore => "certificate_cache_store",
        EventOk::AccountCacheStore => "account_cache_store",
    }
}

fn event_error_label(error: &EventError<io::Error, io::Error>) -> &'static str {
    match error {
        EventError::CertCacheLoad(_) => "certificate_cache_load",
        EventError::AccountCacheLoad(_) => "account_cache_load",
        EventError::CertCacheStore(_) => "certificate_cache_store",
        EventError::AccountCacheStore(_) => "account_cache_store",
        EventError::CachedCertParse(_) => "certificate_cached_parse",
        EventError::Order(_) => "certificate_order",
        EventError::NewCertParse(_) => "certificate_new_parse",
    }
}
