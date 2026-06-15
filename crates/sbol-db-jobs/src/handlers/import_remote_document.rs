//! `import_remote_document` job handler.
//!
//! Fetches a public HTTPS URL server-side and imports the response body
//! through the same service path as `import_document`. This is the right
//! surface for UI-driven public corpus onboarding because browsers do not
//! need CORS access to each registry and workers can retry transient
//! network or database failures.

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_postgres::ImportInput;
use serde::{Deserialize, Serialize};

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "import_remote_document";
const REMOTE_IMPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const REMOTE_IMPORT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportRemoteDocumentPayload {
    pub url: String,
    pub format: SerializationFormat,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub document_iri: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
}

pub struct ImportRemoteDocumentHandler;

#[async_trait]
impl JobHandler for ImportRemoteDocumentHandler {
    type Payload = ImportRemoteDocumentPayload;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        let url = validate_public_https_url(&payload.url)?;
        let document_iri = payload
            .document_iri
            .map(IriString::new)
            .transpose()
            .map_err(|e| HandlerError::InvalidPayload(e.to_string()))?;
        ctx.log(
            "info",
            "remote fetch starting",
            serde_json::json!({
                "url": url.as_str(),
                "format": payload.format,
                "timeout_secs": REMOTE_IMPORT_TIMEOUT.as_secs(),
            }),
        )
        .await;
        let client = reqwest::Client::builder()
            .connect_timeout(REMOTE_IMPORT_CONNECT_TIMEOUT)
            .timeout(REMOTE_IMPORT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                "sbol-db/",
                env!("CARGO_PKG_VERSION"),
                " remote import"
            ))
            .build()
            .map_err(fetch_err)?;

        let response = client.get(url.clone()).send().await.map_err(fetch_err)?;
        let status = response.status();
        ctx.log(
            "info",
            "remote response received",
            serde_json::json!({
                "url": url.as_str(),
                "status": status.as_u16(),
            }),
        )
        .await;
        if status.is_redirection() {
            return Err(HandlerError::Other(format!(
                "remote import redirects are not followed ({status}); enqueue the final public HTTPS URL"
            )));
        }
        let response = response.error_for_status().map_err(fetch_err)?;
        let body = response.text().await.map_err(fetch_err)?;
        let byte_count = body.len();
        ctx.log(
            "info",
            "remote body fetched",
            serde_json::json!({
                "url": url.as_str(),
                "bytes": byte_count,
            }),
        )
        .await;

        ctx.log(
            "info",
            "remote import starting",
            serde_json::json!({
                "format": payload.format,
                "namespace": payload.namespace.as_deref(),
                "name": payload.name.as_deref(),
            }),
        )
        .await;
        let report = ctx
            .service
            .import_document(ImportInput {
                body,
                format: payload.format,
                namespace: payload.namespace,
                source_uri: Some(url.as_str().to_owned()),
                document_iri,
                created_by: payload.created_by,
                name: payload.name,
                description: payload.description,
            })
            .await?;
        ctx.log(
            "info",
            "remote import completed",
            serde_json::json!({
                "graph_id": report.graph_id,
                "object_count": report.object_count,
                "triple_count": report.triple_count,
                "validation_status": report.validation_status,
                "validation_issue_count": report.validation_issue_count,
            }),
        )
        .await;

        Ok(JobOutcome::with_result(serde_json::json!({
            "url": url.as_str(),
            "bytes": byte_count,
            "report": report,
        })))
    }
}

fn fetch_err(e: reqwest::Error) -> HandlerError {
    HandlerError::Other(format!("remote import fetch failed: {e}"))
}

pub(crate) fn validate_public_https_url(raw: &str) -> Result<Url, HandlerError> {
    let url = Url::parse(raw)
        .map_err(|e| HandlerError::InvalidPayload(format!("invalid remote import URL: {e}")))?;
    if url.scheme() != "https" {
        return Err(HandlerError::InvalidPayload(
            "remote import URL must use https".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HandlerError::InvalidPayload(
            "remote import URL must not contain credentials".to_owned(),
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        HandlerError::InvalidPayload("remote import URL must include a host".to_owned())
    })?;
    validate_public_host(host)?;
    Ok(url)
}

fn validate_public_host(host: &str) -> Result<(), HandlerError> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip_host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host.as_str());
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(HandlerError::InvalidPayload(format!(
            "remote import host `{host}` is not public"
        )));
    }
    if let Ok(ip) = ip_host.parse::<IpAddr>() {
        validate_public_ip(ip)?;
    }
    Ok(())
}

fn validate_public_ip(ip: IpAddr) -> Result<(), HandlerError> {
    let private = match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
        }
    };
    if private {
        return Err(HandlerError::InvalidPayload(format!(
            "remote import IP `{ip}` is not public"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_public_https_url;

    #[test]
    fn accepts_public_https_urls() {
        let url = validate_public_https_url("https://synbiohub.org/public/igem/BBa_B0034/1/sbol")
            .expect("public URL");
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn rejects_non_https_urls() {
        let err = validate_public_https_url("http://synbiohub.org/public/igem/BBa_B0034/1/sbol")
            .expect_err("http must be rejected");
        assert!(err.to_string().contains("https"));
    }

    #[test]
    fn rejects_local_hosts() {
        for raw in [
            "https://localhost:8080/private",
            "https://service.local/private",
            "https://10.0.0.1/private",
            "https://127.0.0.1/private",
            "https://[::1]/private",
        ] {
            validate_public_https_url(raw).expect_err(raw);
        }
    }
}
