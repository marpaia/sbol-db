//! `import_synbiohub_collection` job handler.
//!
//! Mirrors a SynBioHub collection by paging member metadata via SPARQL
//! and enqueueing one `import_remote_document` child job per component.
//! This avoids asking SynBioHub to render a very large recursive
//! collection SBOL response, which can time out at the registry edge.

use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::SerializationFormat;
use sbol_db_postgres::NewJob;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};
use crate::handlers::import_remote_document::{
    validate_public_https_url, ImportRemoteDocumentPayload,
};

pub const KIND: &str = "import_synbiohub_collection";
const DEFAULT_BASE_URL: &str = "https://synbiohub.org";
const DEFAULT_PAGE_SIZE: u32 = 250;
const MAX_PAGE_SIZE: u32 = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportSynBioHubCollectionPayload {
    pub collection_uri: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub format: Option<SerializationFormat>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub max_records: Option<u32>,
    #[serde(default)]
    pub created_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SparqlResponse {
    results: SparqlResults,
}

#[derive(Debug, Deserialize)]
struct SparqlResults {
    bindings: Vec<SparqlBinding>,
}

#[derive(Debug, Deserialize)]
struct SparqlBinding {
    member: Option<SparqlTerm>,
    count: Option<SparqlTerm>,
}

#[derive(Debug, Deserialize)]
struct SparqlTerm {
    value: String,
}

pub struct ImportSynBioHubCollectionHandler;

#[async_trait]
impl JobHandler for ImportSynBioHubCollectionHandler {
    type Payload = ImportSynBioHubCollectionPayload;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        let base =
            validate_public_https_url(payload.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL))?;
        let collection = validate_public_https_url(&payload.collection_uri)?;
        let format = payload.format.unwrap_or(SerializationFormat::RdfXml);
        let suffix = synbiohub_download_suffix(format)?;
        let page_size = payload
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!(
                "sbol-db/",
                env!("CARGO_PKG_VERSION"),
                " synbiohub collection import"
            ))
            .build()
            .map_err(fetch_err)?;

        let total = sparql_count(&client, &base, collection.as_str()).await?;
        let target_total = payload.max_records.map(|m| m.min(total)).unwrap_or(total);
        ctx.log(
            "info",
            "synbiohub collection mirror starting",
            serde_json::json!({
                "collection_uri": collection.as_str(),
                "base_url": base.as_str(),
                "format": format,
                "page_size": page_size,
                "total_components": total,
                "target_components": target_total,
            }),
        )
        .await;

        let mut offset = 0u32;
        let mut processed = 0u32;
        let mut enqueued = 0u32;
        let mut deduplicated = 0u32;
        let correlation_id = Uuid::new_v4();

        while offset < target_total {
            let limit = page_size.min(target_total - offset);
            let members =
                sparql_members(&client, &base, collection.as_str(), limit, offset).await?;
            if members.is_empty() {
                ctx.log(
                    "warn",
                    "synbiohub collection page returned no members",
                    serde_json::json!({ "offset": offset, "limit": limit }),
                )
                .await;
                break;
            }

            for member in members {
                if processed >= target_total {
                    break;
                }
                let child_url = format!("{}/{}", member.trim_end_matches('/'), suffix);
                let child_payload = ImportRemoteDocumentPayload {
                    url: child_url.clone(),
                    format,
                    namespace: payload.namespace.clone(),
                    document_iri: None,
                    name: Some(display_name_from_uri(&member)),
                    description: Some(format!(
                        "Imported from SynBioHub collection {}",
                        collection.as_str()
                    )),
                    created_by: payload.created_by.clone(),
                };
                let mut child = NewJob::new(
                    "import_remote_document",
                    serde_json::to_value(child_payload)?,
                );
                child.max_attempts = Some(3);
                child.parent_job_id = Some(ctx.job_id);
                child.correlation_id = Some(correlation_id);
                child.idempotency_key = Some(format!("synbiohub-member:{format:?}:{member}"));
                match ctx.jobs.enqueue(child).await? {
                    sbol_db_postgres::EnqueueOutcome::Inserted(_) => {
                        enqueued += 1;
                    }
                    sbol_db_postgres::EnqueueOutcome::AlreadyExists(_) => {
                        deduplicated += 1;
                    }
                }
                processed += 1;
            }

            ctx.log(
                "info",
                "synbiohub collection page enqueued",
                serde_json::json!({
                    "offset": offset,
                    "limit": limit,
                    "processed_so_far": processed,
                    "enqueued_so_far": enqueued,
                    "deduplicated_so_far": deduplicated,
                }),
            )
            .await;
            offset += limit;
        }

        ctx.log(
            "info",
            "synbiohub collection mirror completed",
            serde_json::json!({
                "collection_uri": collection.as_str(),
                "processed": processed,
                "enqueued": enqueued,
                "deduplicated": deduplicated,
                "correlation_id": correlation_id,
            }),
        )
        .await;

        Ok(JobOutcome::with_result(serde_json::json!({
            "collection_uri": collection.as_str(),
            "total_components": total,
            "target_components": target_total,
            "processed": processed,
            "enqueued": enqueued,
            "deduplicated": deduplicated,
            "correlation_id": correlation_id,
        })))
    }
}

async fn sparql_count(
    client: &reqwest::Client,
    base: &reqwest::Url,
    collection: &str,
) -> Result<u32, HandlerError> {
    let query = format!(
        "PREFIX sbol: <http://sbols.org/v2#> SELECT (COUNT(DISTINCT ?member) AS ?count) WHERE {{ <{collection}> sbol:member ?member . ?member a sbol:ComponentDefinition . }}"
    );
    let response = sparql(client, base, &query).await?;
    let count = response
        .results
        .bindings
        .first()
        .and_then(|b| b.count.as_ref())
        .ok_or_else(|| {
            HandlerError::Other("SynBioHub count response did not include count".to_owned())
        })?
        .value
        .parse::<u32>()
        .map_err(|e| HandlerError::Other(format!("invalid SynBioHub count: {e}")))?;
    Ok(count)
}

async fn sparql_members(
    client: &reqwest::Client,
    base: &reqwest::Url,
    collection: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<String>, HandlerError> {
    let query = format!(
        "PREFIX sbol: <http://sbols.org/v2#> SELECT DISTINCT ?member WHERE {{ <{collection}> sbol:member ?member . ?member a sbol:ComponentDefinition . }} ORDER BY ?member LIMIT {limit} OFFSET {offset}"
    );
    let response = sparql(client, base, &query).await?;
    Ok(response
        .results
        .bindings
        .into_iter()
        .filter_map(|b| b.member.map(|m| m.value))
        .collect())
}

async fn sparql(
    client: &reqwest::Client,
    base: &reqwest::Url,
    query: &str,
) -> Result<SparqlResponse, HandlerError> {
    let url = base
        .join("/sparql")
        .map_err(|e| HandlerError::InvalidPayload(format!("invalid SynBioHub base URL: {e}")))?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/sparql-results+json")
        .query(&[("query", query)])
        .send()
        .await
        .map_err(fetch_err)?
        .error_for_status()
        .map_err(fetch_err)?;
    let body = response.text().await.map_err(fetch_err)?;
    serde_json::from_str::<SparqlResponse>(&body).map_err(|e| {
        HandlerError::Other(format!(
            "SynBioHub collection import returned invalid JSON: {e}"
        ))
    })
}

fn synbiohub_download_suffix(format: SerializationFormat) -> Result<&'static str, HandlerError> {
    match format {
        SerializationFormat::RdfXml => Ok("sbol"),
        SerializationFormat::GenBank => Ok("gb"),
        SerializationFormat::Fasta => Ok("fasta"),
        other => Err(HandlerError::InvalidPayload(format!(
            "SynBioHub collection import supports rdfxml, genbank, and fasta, not {other:?}"
        ))),
    }
}

fn display_name_from_uri(uri: &str) -> String {
    uri.trim_end_matches('/')
        .rsplit('/')
        .nth(1)
        .filter(|s| !s.is_empty())
        .unwrap_or(uri)
        .to_owned()
}

fn fetch_err(e: reqwest::Error) -> HandlerError {
    HandlerError::Other(format!("SynBioHub collection import failed: {e}"))
}
