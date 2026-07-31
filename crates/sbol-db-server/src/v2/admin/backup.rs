//! Integrity-checked portable registry graph backup and atomic restore.
//!
//! This archive intentionally contains SBOL document graphs and their public
//! metadata only. Accounts, password material, tokens, deployment secrets,
//! blobs, and server configuration are excluded.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, PRAGMA};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, SecondsFormat, Utc};
use sbol_db_app::{AdminAuditOutcome, AdminAuditService};
use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_storage::{EnqueueOutcome, ImportInput, ImportOverwrite, ListGraphsFilter, NewJob};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha3::{Digest, Sha3_256};

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use super::confirmation;
use crate::error::ApiError;
use crate::AppState;

const ARCHIVE_FORMAT: &str = "sbol-db-registry-backup";
const ARCHIVE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct BackupDocument {
    document_iri: Option<String>,
    name: Option<String>,
    description: Option<String>,
    source_uri: Option<String>,
    ntriples: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct BackupArchive {
    format: String,
    version: u32,
    created_at: DateTime<Utc>,
    scope: String,
    documents: Vec<BackupDocument>,
    checksum: String,
}

#[derive(Serialize)]
struct ArchivePayload<'a> {
    format: &'a str,
    version: u32,
    created_at: DateTime<Utc>,
    scope: &'a str,
    documents: &'a [BackupDocument],
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RestoreRequest {
    archive: Option<BackupArchive>,
    confirmation: String,
}

pub(super) async fn export(State(state): State<AppState>) -> Result<Response, V2Error> {
    let graphs = state
        .service
        .list_graphs(&ListGraphsFilter {
            limit: u32::MAX,
            ..ListGraphsFilter::default()
        })
        .await?;
    let mut documents = Vec::with_capacity(graphs.len());
    for graph in graphs {
        let physical_graph = format!("graph:document:{}", graph.id);
        let triples = state.service.graph_store_read(&physical_graph).await?;
        let ntriples = canonical_ntriples(sbol_db_rdf::triples_to_rdf(
            &triples,
            SerializationFormat::NTriples,
        )?);
        documents.push(BackupDocument {
            document_iri: graph.document_iri.map(|iri| iri.to_string()),
            name: graph.name,
            description: graph.description,
            source_uri: graph.source_uri,
            ntriples,
        });
    }
    documents.sort_by(|left, right| {
        left.document_iri
            .cmp(&right.document_iri)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.ntriples.cmp(&right.ntriples))
    });
    let mut archive = BackupArchive {
        format: ARCHIVE_FORMAT.to_owned(),
        version: ARCHIVE_VERSION,
        created_at: Utc::now(),
        scope: "registry_graphs_only".to_owned(),
        documents,
        checksum: String::new(),
    };
    archive.checksum = checksum(&archive)?;
    let stamp = archive
        .created_at
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace([':', '-'], "");
    let mut response = Json(archive).into_response();
    let headers: &mut HeaderMap = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"sbol-db-registry-{stamp}.json\""
        ))
        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
    );
    Ok(response)
}

pub(super) async fn validate(body: Bytes) -> Result<Json<Value>, V2Error> {
    let archive: BackupArchive = parse_json(&body)?;
    let confirmation = validate_archive(&archive)?;
    Ok(Json(json!({
        "valid": true,
        "format": archive.format,
        "version": archive.version,
        "checksum": archive.checksum,
        "documents": archive.documents.len(),
        "confirmation": confirmation,
        "scope": archive.scope,
        "excludes": ["accounts", "tokens", "passwords", "configuration", "secrets", "blobs"]
    })))
}

pub(super) async fn restore(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: RestoreRequest = parse_json(&body)?;
    let archive = request
        .archive
        .ok_or_else(|| ApiError::BadRequest("archive is required".to_owned()))?;
    let expected = validate_archive(&archive)?;
    confirmation(&request.confirmation, &expected)?;
    let actor = identity
        .0
        .as_ref()
        .map(|user| user.username.as_str())
        .unwrap_or("unknown");
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "backup.restore",
            actor,
            &archive.checksum,
            AdminAuditOutcome::Attempted,
            Some(&format!("{} documents", archive.documents.len())),
        )
        .await?;

    let inputs = archive
        .documents
        .iter()
        .map(|document| {
            Ok(ImportInput {
                body: document.ntriples.clone(),
                format: SerializationFormat::NTriples,
                namespace: None,
                source_uri: document.source_uri.clone(),
                document_iri: document
                    .document_iri
                    .as_ref()
                    .map(|iri| IriString::new(iri.clone()))
                    .transpose()?,
                created_by: Some(
                    identity
                        .0
                        .as_ref()
                        .expect("admin middleware")
                        .graph_uri
                        .clone(),
                ),
                name: document.name.clone(),
                description: document.description.clone(),
                overwrite: if document.document_iri.is_some() {
                    ImportOverwrite::Replace
                } else {
                    ImportOverwrite::Fail
                },
            })
        })
        .collect::<Result<Vec<_>, sbol_db_core::IriValidationError>>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    let reports = match state.service.import_documents(inputs).await {
        Ok(reports) => reports,
        Err(error) => {
            record_failed(&audit, actor, &archive.checksum, &error.to_string()).await?;
            return Err(error.into());
        }
    };
    let rebuild = state
        .jobs
        .enqueue(NewJob::new("rebuild_search_index", json!({})))
        .await?;
    let rebuild_job = match rebuild {
        EnqueueOutcome::Inserted(job) | EnqueueOutcome::AlreadyExists(job) => job,
    };
    audit
        .record(
            "backup.restore",
            actor,
            &archive.checksum,
            AdminAuditOutcome::Succeeded,
            Some(&format!(
                "{} documents; rebuild job {}",
                reports.len(),
                rebuild_job.id
            )),
        )
        .await?;
    Ok(Json(json!({
        "status": "restored",
        "checksum": archive.checksum,
        "documents": reports.len(),
        "reports": reports,
        "rebuild_job": rebuild_job,
    })))
}

fn validate_archive(archive: &BackupArchive) -> Result<String, V2Error> {
    if archive.format != ARCHIVE_FORMAT || archive.version != ARCHIVE_VERSION {
        return Err(ApiError::BadRequest(format!(
            "unsupported backup format/version: {}/{}",
            archive.format, archive.version
        ))
        .into());
    }
    if archive.scope != "registry_graphs_only" {
        return Err(ApiError::BadRequest("unsupported backup scope".to_owned()).into());
    }
    let actual = checksum(archive)?;
    if archive.checksum != actual {
        return Err(
            ApiError::BadRequest("backup checksum does not match contents".to_owned()).into(),
        );
    }
    for (index, document) in archive.documents.iter().enumerate() {
        if document.ntriples.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "backup document {index} has an empty RDF body"
            ))
            .into());
        }
        if let Some(iri) = &document.document_iri {
            IriString::new(iri.clone()).map_err(|error| ApiError::BadRequest(error.to_string()))?;
        }
        sbol_rdf::Graph::parse(&document.ntriples, sbol_rdf::RdfFormat::NTriples).map_err(
            |error| {
                ApiError::BadRequest(format!("backup document {index} is invalid RDF: {error}"))
            },
        )?;
    }
    Ok(format!("RESTORE {}", &archive.checksum[..12]))
}

fn checksum(archive: &BackupArchive) -> Result<String, V2Error> {
    let payload = ArchivePayload {
        format: &archive.format,
        version: archive.version,
        created_at: archive.created_at,
        scope: &archive.scope,
        documents: &archive.documents,
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|error| ApiError::BadRequest(format!("cannot encode backup: {error}")))?;
    Ok(hex::encode(Sha3_256::digest(bytes)))
}

fn canonical_ntriples(body: String) -> String {
    let mut lines: Vec<&str> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.sort_unstable();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

async fn record_failed(
    audit: &AdminAuditService,
    actor: &str,
    target: &str,
    detail: &str,
) -> Result<(), V2Error> {
    audit
        .record(
            "backup.restore",
            actor,
            target,
            AdminAuditOutcome::Failed,
            Some(detail),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> BackupArchive {
        let mut archive = BackupArchive {
            format: ARCHIVE_FORMAT.to_owned(),
            version: ARCHIVE_VERSION,
            created_at: Utc::now(),
            scope: "registry_graphs_only".to_owned(),
            documents: vec![BackupDocument {
                document_iri: Some("https://example.org/design".to_owned()),
                name: Some("Design".to_owned()),
                description: None,
                source_uri: None,
                ntriples: "<https://example.org/design> <http://example.org/p> \"v\" .\n"
                    .to_owned(),
            }],
            checksum: String::new(),
        };
        archive.checksum = checksum(&archive).expect("checksum");
        archive
    }

    #[test]
    fn integrity_check_detects_a_mutated_document() {
        let mut archive = archive();
        assert!(validate_archive(&archive).is_ok());
        archive.documents[0].name = Some("Changed".to_owned());
        assert!(validate_archive(&archive).is_err());
    }

    #[test]
    fn confirmation_is_derived_from_the_verified_checksum() {
        let archive = archive();
        assert_eq!(
            validate_archive(&archive).expect("valid"),
            format!("RESTORE {}", &archive.checksum[..12])
        );
    }
}
