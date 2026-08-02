//! Manifest-gated, resumable classic SynBioHub production loader.
//!
//! This path is deliberately Postgres-only. It verifies every preflight file
//! artifact before touching the target, records durable per-artifact/per-user/
//! per-graph/per-blob progress, writes RDF in bounded batches, and reconciles
//! target graph fingerprints before declaring the canonical migration ready.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use oxrdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Term};
use oxrdfio::RdfParser;
use sbol_db_core::{IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_postgres::{AccelRepository, PgPool, TripleRepository};
use sbol_db_storage::{ConfigStore, JobQueue, NewJob};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::preflight::{
    hash_artifact, merge_json, transform_classic_config, update_user_fingerprint, GraphAccumulator,
    GraphClass, IssueSeverity, PreflightReport, MANIFEST_SCHEMA,
};
use super::rdf_format_for;
use crate::output::print_json;

const IMPORTER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+production-v1");
const SOURCE_TAG: &str = "synbiohub-production";
const REINDEX_KIND: &str = "rebuild_search_index";
const ADVISORY_LOCK: &str = "sbol-db:synbiohub-production-migration";
const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
const SBH_ATTACHMENT_HASH: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash";

#[derive(Debug)]
pub struct ProductionInputs {
    pub manifest: PathBuf,
    pub policy: Option<PathBuf>,
    pub blob_store: PathBuf,
    pub chunk_size: usize,
    pub no_reindex: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationPolicy {
    #[serde(default)]
    waivers: BTreeMap<String, String>,
    #[serde(default)]
    other_graphs: BTreeMap<String, GraphDisposition>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum GraphDisposition {
    Import,
}

#[derive(Debug, Serialize)]
pub struct ProductionReport {
    pub run_id: Uuid,
    pub source_bundle_sha256: String,
    pub status: String,
    pub artifacts_verified: u64,
    pub users_verified: u64,
    pub reset_links_invalidated: u64,
    pub graphs_verified: u64,
    pub triples_verified: u64,
    pub accelerators_verified: u64,
    pub blobs_verified: u64,
    pub config_keys: Vec<String>,
    pub reindex_job: Option<String>,
    pub required_runtime_secrets: Vec<&'static str>,
}

pub async fn run(
    pool: PgPool,
    config: Arc<dyn ConfigStore>,
    jobs: Arc<dyn JobQueue>,
    inputs: ProductionInputs,
) -> Result<()> {
    if inputs.chunk_size == 0 {
        bail!("--chunk-size must be greater than zero");
    }
    let manifest_bytes = std::fs::read(&inputs.manifest)
        .with_context(|| format!("reading manifest {}", inputs.manifest.display()))?;
    let manifest: PreflightReport = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parsing manifest {}", inputs.manifest.display()))?;
    validate_manifest(&manifest)?;
    let policy = load_policy(inputs.policy.as_deref())?;
    validate_policy(&manifest, &policy)?;

    // Source verification intentionally precedes schema migration: a stale or
    // changed bundle must not mutate even an otherwise empty destination.
    let artifacts_verified = verify_artifacts(&manifest)?;
    sbol_db_postgres::run_migrations(&pool)
        .await
        .context("applying Postgres migrations")?;

    let mut lock_conn = pool
        .acquire()
        .await
        .context("acquiring migration lock connection")?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK)
        .fetch_one(&mut *lock_conn)
        .await
        .context("acquiring migration advisory lock")?;
    if !locked {
        bail!("another SynBioHub production migration is already running");
    }

    let result = run_locked(
        &pool,
        config,
        jobs,
        &manifest_bytes,
        &manifest,
        &policy,
        artifacts_verified,
        &inputs,
    )
    .await;
    let unlock =
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(ADVISORY_LOCK)
            .fetch_one(&mut *lock_conn)
            .await;
    if let Err(error) = unlock {
        tracing::error!(%error, "failed to release migration advisory lock");
    }
    let report = result?;
    print_json(&report)
}

#[allow(clippy::too_many_arguments)]
async fn run_locked(
    pool: &PgPool,
    config: Arc<dyn ConfigStore>,
    jobs: Arc<dyn JobQueue>,
    manifest_bytes: &[u8],
    manifest: &PreflightReport,
    policy: &MigrationPolicy,
    artifacts_verified: u64,
    inputs: &ProductionInputs,
) -> Result<ProductionReport> {
    let (run_id, existing_status, is_new) =
        prepare_run(pool, manifest, manifest_bytes, &inputs.blob_store).await?;
    if existing_status == "ready" {
        verify_upload_tree_at(
            &inputs.blob_store.join("uploads"),
            manifest.uploads.as_ref().expect("validated upload report"),
        )?;
        reconcile_users(pool, run_id, manifest).await?;
        reconcile_graphs(pool, run_id, manifest, policy, inputs.chunk_size).await?;
        rebuild_accelerators(pool, run_id).await?;
        return summarize_ready_run(pool, run_id, manifest, artifacts_verified).await;
    }

    if is_new {
        initialize_ledger(pool, run_id, manifest, policy).await?;
    }
    record_verified_artifacts(pool, run_id, manifest, manifest_bytes).await?;
    set_run_status(pool, run_id, "loading").await?;

    let work = async {
        let (users_verified, reset_links_invalidated) =
            load_users(pool, run_id, manifest, inputs.chunk_size).await?;
        let (blobs_verified, staging_uploads) =
            stage_uploads(pool, run_id, manifest, &inputs.blob_store).await?;
        let referenced_blobs = load_rdf(pool, run_id, manifest, policy, inputs.chunk_size).await?;
        mark_referenced_blobs(pool, run_id, &referenced_blobs).await?;

        set_run_status(pool, run_id, "reconciling").await?;
        let (graphs_verified, triples_verified) =
            reconcile_graphs(pool, run_id, manifest, policy, inputs.chunk_size).await?;
        reconcile_users(pool, run_id, manifest).await?;
        reconcile_blobs(pool, run_id, manifest, blobs_verified).await?;
        let accelerators_verified = rebuild_accelerators(pool, run_id).await?;

        let config_keys = load_config(config.as_ref(), manifest).await?;
        promote_uploads(&staging_uploads, &inputs.blob_store)?;
        let reindex_job = if inputs.no_reindex {
            None
        } else {
            let mut job = NewJob::new(REINDEX_KIND, serde_json::json!({"migrationRunId": run_id}));
            job.idempotency_key = Some(reindex_idempotency_key(run_id));
            Some(
                jobs.enqueue(job)
                    .await
                    .context("enqueuing derived-index rebuild")?
                    .into_job()
                    .id
                    .to_string(),
            )
        };

        sqlx::query(
            "UPDATE sbh_migration_run SET status = 'ready', updated_at = now(), \
             completed_at = now() WHERE id = $1",
        )
        .bind(run_id)
        .execute(pool)
        .await
        .context("marking migration ready")?;

        Ok::<_, anyhow::Error>(ProductionReport {
            run_id,
            source_bundle_sha256: manifest.source_bundle_sha256.clone(),
            status: "ready".to_owned(),
            artifacts_verified,
            users_verified,
            reset_links_invalidated,
            graphs_verified,
            triples_verified,
            accelerators_verified,
            blobs_verified,
            config_keys,
            reindex_job,
            required_runtime_secrets: vec!["SBOL_DB_PASSWORD_SALT", "SBOL_DB_SHARE_LINK_SALT"],
        })
    }
    .await;

    if let Err(error) = &work {
        let _ = sqlx::query(
            "UPDATE sbh_migration_run SET status = 'failed', updated_at = now() WHERE id = $1",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        tracing::error!(run_id = %run_id, %error, "production migration failed; rerun resumes the ledger");
    }
    work
}

fn validate_manifest(manifest: &PreflightReport) -> Result<()> {
    if manifest.schema != MANIFEST_SCHEMA {
        bail!(
            "unsupported manifest schema `{}` (expected `{MANIFEST_SCHEMA}`)",
            manifest.schema
        );
    }
    if manifest.source_bundle_sha256.len() != 64
        || !manifest
            .source_bundle_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("manifest source_bundle_sha256 is not a SHA-256 digest");
    }
    if manifest.source.rdf.is_none() || manifest.rdf.is_none() {
        bail!("manifest has no reconciled RDF export");
    }
    if manifest.source.sqlite.is_none() || manifest.sqlite.is_none() {
        bail!("manifest has no inspected account database");
    }
    if manifest
        .sqlite
        .as_ref()
        .is_some_and(|sqlite| sqlite.user_rows_sha256.len() != 64)
    {
        bail!("manifest has no complete logical account fingerprint");
    }
    if manifest.source.uploads.is_none() || manifest.uploads.is_none() {
        bail!("manifest has no inspected uploads tree");
    }
    Ok(())
}

fn load_policy(path: Option<&Path>) -> Result<MigrationPolicy> {
    let Some(path) = path else {
        return Ok(MigrationPolicy::default());
    };
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading migration policy {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing migration policy {}", path.display()))
}

fn validate_policy(manifest: &PreflightReport, policy: &MigrationPolicy) -> Result<()> {
    let blockers = manifest
        .issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Blocker)
        .collect::<Vec<_>>();
    for blocker in blockers {
        let reason = policy.waivers.get(&blocker.code).map(String::as_str);
        if reason.is_none_or(|reason| reason.trim().is_empty()) {
            bail!(
                "manifest blocker `{}` requires a non-empty waiver reason in the policy",
                blocker.code
            );
        }
    }
    if let Some(rdf) = &manifest.rdf {
        for graph in &rdf.graphs {
            if graph.class == GraphClass::Other
                && policy
                    .other_graphs
                    .get(&graph.iri)
                    .or_else(|| policy.other_graphs.get("*"))
                    != Some(&GraphDisposition::Import)
            {
                bail!(
                    "other graph `{}` requires an explicit `import` disposition (an `*` policy is allowed)",
                    graph.iri
                );
            }
        }
    }
    Ok(())
}

fn verify_artifacts(manifest: &PreflightReport) -> Result<u64> {
    for expected in &manifest.artifacts {
        let actual = hash_artifact(&expected.kind, &expected.path)?;
        if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
            bail!(
                "source artifact `{}` changed after preflight: expected {} bytes / {}, got {} bytes / {}",
                expected.kind,
                expected.bytes,
                expected.sha256,
                actual.bytes,
                actual.sha256
            );
        }
    }
    Ok(manifest.artifacts.len() as u64 + 1)
}

async fn prepare_run(
    pool: &PgPool,
    manifest: &PreflightReport,
    manifest_bytes: &[u8],
    blob_root: &Path,
) -> Result<(Uuid, String, bool)> {
    if let Some(row) = sqlx::query(
        "SELECT id, status FROM sbh_migration_run \
         WHERE source_bundle_sha256 = $1 AND importer_version = $2",
    )
    .bind(&manifest.source_bundle_sha256)
    .bind(IMPORTER_VERSION)
    .fetch_optional(pool)
    .await
    .context("looking up resumable migration run")?
    {
        return Ok((row.try_get("id")?, row.try_get("status")?, false));
    }

    let other_runs: i64 = sqlx::query_scalar("SELECT count(*) FROM sbh_migration_run")
        .fetch_one(pool)
        .await?;
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM sbh_user")
        .fetch_one(pool)
        .await?;
    let triples: i64 = sqlx::query_scalar("SELECT count(*) FROM sbol_triples")
        .fetch_one(pool)
        .await?;
    let config: i64 = sqlx::query_scalar("SELECT count(*) FROM sbh_app_config")
        .fetch_one(pool)
        .await?;
    if other_runs != 0 || users != 0 || triples != 0 || config != 0 {
        bail!(
            "target is not empty and has no resumable run for this source bundle \
             (runs={other_runs}, users={users}, triples={triples}, config={config})"
        );
    }
    let uploads = blob_root.join("uploads");
    if directory_has_entries(&uploads)? {
        bail!(
            "target blob directory {} is non-empty and has no resumable run for this bundle",
            uploads.display()
        );
    }

    let run_id = Uuid::new_v4();
    let manifest_json: Value = serde_json::from_slice(manifest_bytes)?;
    sqlx::query(
        "INSERT INTO sbh_migration_run \
         (id, source_bundle_sha256, importer_version, manifest, status) \
         VALUES ($1, $2, $3, $4, 'preparing')",
    )
    .bind(run_id)
    .bind(&manifest.source_bundle_sha256)
    .bind(IMPORTER_VERSION)
    .bind(manifest_json)
    .execute(pool)
    .await
    .context("creating migration run")?;
    Ok((run_id, "preparing".to_owned(), true))
}

fn directory_has_entries(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if !path.is_dir() {
        bail!("expected directory at {}", path.display());
    }
    Ok(std::fs::read_dir(path)?.next().transpose()?.is_some())
}

async fn initialize_ledger(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    policy: &MigrationPolicy,
) -> Result<()> {
    for issue in &manifest.issues {
        let waiver = policy.waivers.get(&issue.code);
        sqlx::query(
            "INSERT INTO sbh_migration_issue \
             (run_id, severity, scope, code, details, waived_at, waiver_reason) \
             VALUES ($1, $2, $3, $4, $5, CASE WHEN $6::text IS NULL THEN NULL ELSE now() END, $6)",
        )
        .bind(run_id)
        .bind(match issue.severity {
            IssueSeverity::Blocker => "blocker",
            IssueSeverity::Warning => "warning",
        })
        .bind(match issue.scope {
            super::preflight::IssueScope::Source => "source",
            super::preflight::IssueScope::Target => "target",
            super::preflight::IssueScope::Policy => "policy",
        })
        .bind(&issue.code)
        .bind(serde_json::to_value(issue)?)
        .bind(waiver)
        .execute(pool)
        .await?;
    }

    let rdf = manifest.rdf.as_ref().expect("validated RDF report");
    for graph in &rdf.graphs {
        sqlx::query(
            "INSERT INTO sbh_migration_graph \
             (run_id, graph_iri, graph_class, expected_quads, expected_fingerprint, status) \
             VALUES ($1, $2, $3, $4, $5, 'pending') ON CONFLICT DO NOTHING",
        )
        .bind(run_id)
        .bind(&graph.iri)
        .bind(graph_class_name(graph.class))
        .bind(i64::try_from(graph.quads).context("graph quad count exceeds bigint")?)
        .bind(&graph.fingerprint)
        .execute(pool)
        .await?;
    }

    let uploads = manifest.uploads.as_ref().expect("validated upload report");
    for blob in &uploads.blobs {
        let sha1 = blob
            .expected_sha1
            .as_deref()
            .context("manifest blob has no content address")?;
        sqlx::query(
            "INSERT INTO sbh_migration_blob \
             (run_id, sha1, compressed_bytes, uncompressed_bytes, compressed_sha256, referenced, status) \
             VALUES ($1, $2, $3, $4, $5, false, 'pending') ON CONFLICT DO NOTHING",
        )
        .bind(run_id)
        .bind(sha1)
        .bind(i64::try_from(blob.compressed_bytes)?)
        .bind(i64::try_from(blob.uncompressed_bytes.unwrap_or_default())?)
        .bind(blob.compressed_sha256.as_deref().context("manifest blob has no SHA-256")?)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn record_verified_artifacts(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    manifest_bytes: &[u8],
) -> Result<()> {
    for artifact in &manifest.artifacts {
        sqlx::query(
            "INSERT INTO sbh_migration_artifact (run_id, kind, bytes, sha256, status) \
             VALUES ($1, $2, $3, $4, 'verified') \
             ON CONFLICT (run_id, kind) DO UPDATE SET bytes = EXCLUDED.bytes, \
             sha256 = EXCLUDED.sha256, status = 'verified', error = NULL, updated_at = now()",
        )
        .bind(run_id)
        .bind(&artifact.kind)
        .bind(i64::try_from(artifact.bytes)?)
        .bind(&artifact.sha256)
        .execute(pool)
        .await?;
    }
    let manifest_sha256 = hex::encode(Sha256::digest(manifest_bytes));
    sqlx::query(
        "INSERT INTO sbh_migration_artifact (run_id, kind, bytes, sha256, status) \
         VALUES ($1, 'manifest', $2, $3, 'verified') \
         ON CONFLICT (run_id, kind) DO UPDATE SET bytes = EXCLUDED.bytes, \
         sha256 = EXCLUDED.sha256, status = 'verified', error = NULL, updated_at = now()",
    )
    .bind(run_id)
    .bind(i64::try_from(manifest_bytes.len())?)
    .bind(manifest_sha256)
    .execute(pool)
    .await?;
    Ok(())
}

async fn set_run_status(pool: &PgPool, run_id: Uuid, status: &str) -> Result<()> {
    sqlx::query("UPDATE sbh_migration_run SET status = $2, updated_at = now() WHERE id = $1")
        .bind(run_id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

async fn load_users(
    target: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    chunk_size: usize,
) -> Result<(u64, u64)> {
    let expected = manifest.sqlite.as_ref().expect("validated sqlite report");
    let already_verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_identity WHERE run_id = $1 AND status = 'verified'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await?;
    if u64::try_from(already_verified)? == expected.users {
        return Ok((expected.users, expected.active_reset_links));
    }

    let source_path = manifest
        .source
        .sqlite
        .as_ref()
        .expect("validated sqlite path");
    let temp = tempfile::tempdir().context("creating private SQLite snapshot directory")?;
    let private_db = temp.path().join("synbiohub.sqlite");
    std::fs::copy(source_path, &private_db)
        .with_context(|| format!("copying SQLite snapshot {}", source_path.display()))?;
    for (source, name) in [
        (manifest.source.sqlite_wal.as_ref(), "synbiohub.sqlite-wal"),
        (manifest.source.sqlite_shm.as_ref(), "synbiohub.sqlite-shm"),
    ] {
        if let Some(source) = source.filter(|path| path.is_file()) {
            std::fs::copy(source, temp.path().join(name))?;
        }
    }
    let options = SqliteConnectOptions::new()
        .filename(&private_db)
        .read_only(true)
        .create_if_missing(false);
    let source = SqlitePool::connect_with(options)
        .await
        .context("opening private SQLite snapshot")?;
    let table = match expected.user_table.as_str() {
        "user" => "user",
        "users" => "users",
        other => bail!("unsupported classic user table `{other}`"),
    };
    let query = format!(
        "SELECT id, name, username, email, affiliation, password, graphUri, \
         isAdmin, isCurator, isMember, resetPasswordLink, createdAt, updatedAt \
         FROM \"{table}\" WHERE id > ? ORDER BY id LIMIT ?"
    );
    let mut last_id = -1_i64;
    let mut source_fingerprint = Sha256::new();
    loop {
        let rows = sqlx::query(&query)
            .bind(last_id)
            .bind(i64::try_from(chunk_size)?)
            .fetch_all(&source)
            .await
            .context("streaming classic users")?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            update_user_fingerprint(&mut source_fingerprint, &row)?;
            let source_id: i64 = row.try_get("id")?;
            last_id = source_id;
            let username = row
                .try_get::<Option<String>, _>("username")?
                .unwrap_or_default();
            let graph_uri = row
                .try_get::<Option<String>, _>("graphUri")?
                .unwrap_or_else(|| format!("{}user/{username}", manifest.config.database_prefix));
            let target_id = deterministic_user_id(&manifest.source_bundle_sha256, source_id);
            let created_at =
                parse_classic_timestamp(row.try_get::<Option<String>, _>("createdAt")?.as_deref())?;
            let updated_at =
                parse_classic_timestamp(row.try_get::<Option<String>, _>("updatedAt")?.as_deref())?;
            sqlx::query(
                "INSERT INTO sbh_user \
                 (id, username, name, email, affiliation, password_hash, graph_uri, \
                  is_admin, is_curator, is_member, reset_password_link, created_at, updated_at) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,NULL,$11,$12) \
                 ON CONFLICT (id) DO UPDATE SET username=EXCLUDED.username, name=EXCLUDED.name, \
                  email=EXCLUDED.email, affiliation=EXCLUDED.affiliation, \
                  password_hash=EXCLUDED.password_hash, graph_uri=EXCLUDED.graph_uri, \
                  is_admin=EXCLUDED.is_admin, is_curator=EXCLUDED.is_curator, \
                  is_member=EXCLUDED.is_member, reset_password_link=NULL, \
                  created_at=EXCLUDED.created_at, updated_at=EXCLUDED.updated_at",
            )
            .bind(target_id)
            .bind(&username)
            .bind(
                row.try_get::<Option<String>, _>("name")?
                    .unwrap_or_default(),
            )
            .bind(
                row.try_get::<Option<String>, _>("email")?
                    .unwrap_or_default(),
            )
            .bind(row.try_get::<Option<String>, _>("affiliation")?)
            .bind(
                row.try_get::<Option<String>, _>("password")?
                    .unwrap_or_default(),
            )
            .bind(&graph_uri)
            .bind(sqlite_bool(&row, "isAdmin")?)
            .bind(sqlite_bool(&row, "isCurator")?)
            .bind(sqlite_bool(&row, "isMember")?)
            .bind(created_at)
            .bind(updated_at)
            .execute(target)
            .await
            .with_context(|| format!("loading classic user id {source_id}"))?;
            sqlx::query(
                "INSERT INTO sbh_migration_identity \
                 (run_id, source_user_id, target_user_id, source_graph_uri, status) \
                 VALUES ($1,$2,$3,$4,'verified') \
                 ON CONFLICT (run_id, source_user_id) DO UPDATE SET \
                 target_user_id=EXCLUDED.target_user_id, source_graph_uri=EXCLUDED.source_graph_uri, \
                 status='verified', error=NULL, updated_at=now()",
            )
            .bind(run_id)
            .bind(source_id)
            .bind(target_id)
            .bind(&graph_uri)
            .execute(target)
            .await?;
        }
    }
    source.close().await;
    let actual_fingerprint = hex::encode(source_fingerprint.finalize());
    if actual_fingerprint != expected.user_rows_sha256 {
        bail!(
            "classic account rows changed after preflight: expected {}, got {}",
            expected.user_rows_sha256,
            actual_fingerprint
        );
    }
    reconcile_users(target, run_id, manifest).await?;
    Ok((expected.users, expected.active_reset_links))
}

fn deterministic_user_id(bundle: &str, source_id: i64) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"sbol-db:synbiohub-user\0");
    hasher.update(bundle.as_bytes());
    hasher.update([0]);
    hasher.update(source_id.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn parse_classic_timestamp(value: Option<&str>) -> Result<DateTime<Utc>> {
    let value = value.context("classic user timestamp is missing")?;
    if let Ok(parsed) = DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f %:z") {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(parsed.and_utc());
    }
    bail!("unrecognized classic timestamp `{value}`")
}

fn sqlite_bool(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<bool> {
    Ok(row.try_get::<Option<i64>, _>(column)?.unwrap_or(0) != 0)
}

async fn stage_uploads(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    blob_root: &Path,
) -> Result<(u64, PathBuf)> {
    let source_root = manifest
        .source
        .uploads
        .as_ref()
        .expect("validated uploads path");
    let report = manifest.uploads.as_ref().expect("validated upload report");
    let final_uploads = blob_root.join("uploads");
    let staging_uploads = blob_root
        .join(".synbiohub-migration")
        .join(run_id.to_string())
        .join("uploads");
    if final_uploads.exists() && !staging_uploads.exists() {
        // A resumed run may already have promoted the byte-for-byte verified
        // tree. Reconciliation below validates its full manifest again.
        verify_upload_tree_at(&final_uploads, report)?;
        return Ok((report.blob_files, final_uploads));
    }
    std::fs::create_dir_all(&staging_uploads)?;

    for blob in &report.blobs {
        validate_relative_path(&blob.relative_path)?;
        let source = source_root.join(&blob.relative_path);
        let destination = staging_uploads.join(&blob.relative_path);
        copy_verified(
            &source,
            &destination,
            blob.compressed_bytes,
            blob.compressed_sha256
                .as_deref()
                .context("blob SHA-256 missing")?,
        )?;
        let sha1 = blob
            .expected_sha1
            .as_deref()
            .context("blob address missing")?;
        sqlx::query(
            "UPDATE sbh_migration_blob SET status='verified', error=NULL, updated_at=now() \
             WHERE run_id=$1 AND sha1=$2",
        )
        .bind(run_id)
        .bind(sha1)
        .execute(pool)
        .await?;
    }
    for asset in &report.assets {
        validate_relative_path(&asset.relative_path)?;
        copy_verified(
            &source_root.join(&asset.relative_path),
            &staging_uploads.join(&asset.relative_path),
            asset.bytes,
            &asset.sha256,
        )?;
    }
    Ok((report.blob_files, staging_uploads))
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("manifest contains unsafe upload path {}", path.display());
    }
    Ok(())
}

fn copy_verified(source: &Path, destination: &Path, bytes: u64, sha256: &str) -> Result<()> {
    if destination.is_file() {
        let actual = hash_artifact("upload", destination)?;
        if actual.bytes == bytes && actual.sha256 == sha256 {
            return Ok(());
        }
        bail!(
            "staged upload {} does not match its manifest",
            destination.display()
        );
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("migration-part");
    std::fs::copy(source, &temporary)
        .with_context(|| format!("copying upload {}", source.display()))?;
    let actual = hash_artifact("upload", &temporary)?;
    if actual.bytes != bytes || actual.sha256 != sha256 {
        let _ = std::fs::remove_file(&temporary);
        bail!(
            "copied upload {} failed checksum verification",
            source.display()
        );
    }
    std::fs::rename(&temporary, destination)?;
    Ok(())
}

fn verify_upload_tree_at(root: &Path, report: &super::preflight::UploadReport) -> Result<()> {
    for blob in &report.blobs {
        validate_relative_path(&blob.relative_path)?;
        let actual = hash_artifact("upload", &root.join(&blob.relative_path))?;
        if actual.bytes != blob.compressed_bytes
            || Some(actual.sha256.as_str()) != blob.compressed_sha256.as_deref()
        {
            bail!(
                "promoted blob {} failed resume verification",
                blob.relative_path.display()
            );
        }
    }
    for asset in &report.assets {
        validate_relative_path(&asset.relative_path)?;
        let actual = hash_artifact("upload_asset", &root.join(&asset.relative_path))?;
        if actual.bytes != asset.bytes || actual.sha256 != asset.sha256 {
            bail!(
                "promoted asset {} failed resume verification",
                asset.relative_path.display()
            );
        }
    }
    Ok(())
}

fn promote_uploads(staging_uploads: &Path, blob_root: &Path) -> Result<()> {
    let final_uploads = blob_root.join("uploads");
    if staging_uploads == final_uploads {
        return Ok(());
    }
    if final_uploads.exists() {
        if directory_has_entries(&final_uploads)? {
            bail!(
                "blob destination {} became non-empty before promotion",
                final_uploads.display()
            );
        }
        std::fs::remove_dir(&final_uploads)?;
    }
    std::fs::create_dir_all(blob_root)?;
    std::fs::rename(staging_uploads, &final_uploads)
        .with_context(|| format!("promoting uploads into {}", final_uploads.display()))?;
    Ok(())
}

async fn load_rdf(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    policy: &MigrationPolicy,
    chunk_size: usize,
) -> Result<BTreeSet<String>> {
    let path = manifest.source.rdf.as_ref().expect("validated RDF path");
    let format = rdf_format_for(path)?;
    let expected = manifest
        .rdf
        .as_ref()
        .expect("validated RDF report")
        .graphs
        .iter()
        .map(|graph| (graph.iri.as_str(), graph.class))
        .collect::<BTreeMap<_, _>>();
    for (iri, class) in &expected {
        if *class == GraphClass::Other {
            let disposition = policy
                .other_graphs
                .get(*iri)
                .or_else(|| policy.other_graphs.get("*"));
            if disposition != Some(&GraphDisposition::Import) {
                bail!("other graph `{iri}` is not approved for import");
            }
        }
    }

    let repository = TripleRepository::new(pool.clone());
    let mut connection = pool.acquire().await?;
    for iri in expected.keys() {
        repository
            .ensure_graph(&mut connection, iri, "verbatim")
            .await?;
    }
    sqlx::query(
        "UPDATE sbh_migration_graph SET status='loading', error=NULL, updated_at=now() \
         WHERE run_id=$1 AND status <> 'verified'",
    )
    .bind(run_id)
    .execute(&mut *connection)
    .await?;

    let file =
        File::open(path).with_context(|| format!("opening RDF export {}", path.display()))?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut batch = Vec::with_capacity(chunk_size);
    let mut referenced_blobs = BTreeSet::new();
    for parsed in RdfParser::from_format(format).for_reader(reader) {
        let quad = parsed.with_context(|| format!("parsing RDF export {}", path.display()))?;
        let graph = match &quad.graph_name {
            GraphName::NamedNode(graph) => graph.as_str(),
            GraphName::DefaultGraph => bail!("RDF export contains a default-graph quad"),
            GraphName::BlankNode(_) => bail!("RDF export contains a blank-node graph name"),
        };
        if !expected.contains_key(graph) {
            bail!("RDF export contains graph `{graph}` absent from the preflight manifest");
        }
        if quad.predicate.as_str() == SBOL2_HASH || quad.predicate.as_str() == SBH_ATTACHMENT_HASH {
            if let Term::Literal(value) = &quad.object {
                let hash = value.value().to_ascii_lowercase();
                if hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    referenced_blobs.insert(hash);
                }
            }
        }
        batch.push(ox_quad_to_domain(&quad)?);
        if batch.len() >= chunk_size {
            repository
                .insert_triples(&mut connection, &batch, SOURCE_TAG)
                .await?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        repository
            .insert_triples(&mut connection, &batch, SOURCE_TAG)
            .await?;
    }
    Ok(referenced_blobs)
}

fn ox_quad_to_domain(quad: &oxrdf::Quad) -> Result<Triple> {
    let graph_iri = match &quad.graph_name {
        GraphName::NamedNode(graph) => Some(IriString::unchecked(graph.as_str())),
        GraphName::DefaultGraph => None,
        GraphName::BlankNode(_) => bail!("blank-node graph names are unsupported"),
    };
    let subject = match &quad.subject {
        NamedOrBlankNode::NamedNode(node) => SubjectTerm::Iri(IriString::unchecked(node.as_str())),
        NamedOrBlankNode::BlankNode(node) => SubjectTerm::BlankNode(node.as_str().to_owned()),
    };
    let object = match &quad.object {
        Term::NamedNode(node) => ObjectTerm::Iri(IriString::unchecked(node.as_str())),
        Term::BlankNode(node) => ObjectTerm::BlankNode(node.as_str().to_owned()),
        Term::Literal(literal) => ObjectTerm::Literal {
            value: literal.value().to_owned(),
            datatype: IriString::unchecked(literal.datatype().as_str()),
            language: literal.language().map(ToOwned::to_owned),
        },
    };
    Ok(Triple {
        graph_iri,
        subject,
        predicate: IriString::unchecked(quad.predicate.as_str()),
        object,
    })
}

async fn mark_referenced_blobs(
    pool: &PgPool,
    run_id: Uuid,
    hashes: &BTreeSet<String>,
) -> Result<()> {
    for chunk in hashes.iter().collect::<Vec<_>>().chunks(10_000) {
        let chunk = chunk.iter().map(|value| value.as_str()).collect::<Vec<_>>();
        sqlx::query(
            "UPDATE sbh_migration_blob SET referenced=true, updated_at=now() \
             WHERE run_id=$1 AND sha1 = ANY($2::text[])",
        )
        .bind(run_id)
        .bind(chunk)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn reconcile_graphs(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    _policy: &MigrationPolicy,
    chunk_size: usize,
) -> Result<(u64, u64)> {
    let mut verified = 0_u64;
    let mut total = 0_u64;
    for expected in &manifest.rdf.as_ref().expect("validated RDF report").graphs {
        let (count, fingerprint) =
            target_graph_fingerprint(pool, &expected.iri, chunk_size).await?;
        total = total
            .checked_add(count)
            .context("target triple count overflow")?;
        if count != expected.quads || fingerprint != expected.fingerprint {
            sqlx::query(
                "UPDATE sbh_migration_graph SET loaded_quads=$3, status='failed', \
                 error=$4, updated_at=now() WHERE run_id=$1 AND graph_iri=$2",
            )
            .bind(run_id)
            .bind(&expected.iri)
            .bind(i64::try_from(count)?)
            .bind(format!(
                "expected {} / {}, got {} / {}",
                expected.quads, expected.fingerprint, count, fingerprint
            ))
            .execute(pool)
            .await?;
            bail!("target reconciliation failed for graph `{}`", expected.iri);
        }
        sqlx::query(
            "UPDATE sbh_migration_graph SET loaded_quads=$3, status='verified', \
             error=NULL, updated_at=now() WHERE run_id=$1 AND graph_iri=$2",
        )
        .bind(run_id)
        .bind(&expected.iri)
        .bind(i64::try_from(count)?)
        .execute(pool)
        .await?;
        verified += 1;
    }
    Ok((verified, total))
}

async fn target_graph_fingerprint(
    pool: &PgPool,
    graph: &str,
    chunk_size: usize,
) -> Result<(u64, String)> {
    let mut accumulator = GraphAccumulator::default();
    let mut last_id = 0_i64;
    loop {
        let rows = sqlx::query(
            "SELECT id, subject_iri, subject_blank, predicate_iri, object_iri, object_blank, \
             object_literal, datatype_iri, language FROM sbol_triples \
             WHERE graph_iri=$1 AND id>$2 ORDER BY id LIMIT $3",
        )
        .bind(graph)
        .bind(last_id)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            last_id = row.try_get("id")?;
            let triple = pg_row_to_ox_triple(&row)?;
            accumulator.add(&triple);
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sbol_triples WHERE graph_iri=$1")
        .bind(graph)
        .fetch_one(pool)
        .await?;
    Ok((u64::try_from(count)?, accumulator.finish()))
}

fn pg_row_to_ox_triple(row: &sqlx::postgres::PgRow) -> Result<oxrdf::Triple> {
    let subject = match (
        row.try_get::<Option<String>, _>("subject_iri")?,
        row.try_get::<Option<String>, _>("subject_blank")?,
    ) {
        (Some(iri), None) => NamedOrBlankNode::NamedNode(NamedNode::new(iri)?),
        (None, Some(blank)) => NamedOrBlankNode::BlankNode(BlankNode::new(blank)?),
        _ => bail!("target row has invalid RDF subject columns"),
    };
    let predicate = NamedNode::new(row.try_get::<String, _>("predicate_iri")?)?;
    let object = match (
        row.try_get::<Option<String>, _>("object_iri")?,
        row.try_get::<Option<String>, _>("object_blank")?,
        row.try_get::<Option<String>, _>("object_literal")?,
    ) {
        (Some(iri), None, None) => Term::NamedNode(NamedNode::new(iri)?),
        (None, Some(blank), None) => Term::BlankNode(BlankNode::new(blank)?),
        (None, None, Some(value)) => {
            let language: Option<String> = row.try_get("language")?;
            let datatype: Option<String> = row.try_get("datatype_iri")?;
            let literal = if let Some(language) = language {
                Literal::new_language_tagged_literal(value, language)?
            } else {
                Literal::new_typed_literal(
                    value,
                    NamedNode::new(datatype.context("literal row is missing datatype")?)?,
                )
            };
            Term::Literal(literal)
        }
        _ => bail!("target row has invalid RDF object columns"),
    };
    Ok(oxrdf::Triple::new(subject, predicate, object))
}

async fn reconcile_users(pool: &PgPool, run_id: Uuid, manifest: &PreflightReport) -> Result<()> {
    let expected = manifest
        .sqlite
        .as_ref()
        .expect("validated sqlite report")
        .users;
    let ledger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_identity WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let target: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_user u JOIN sbh_migration_identity i \
         ON i.target_user_id=u.id WHERE i.run_id=$1 AND i.status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if u64::try_from(ledger)? != expected || ledger != target {
        bail!(
            "identity reconciliation failed: expected {expected}, ledger={ledger}, target={target}"
        );
    }
    Ok(())
}

async fn reconcile_blobs(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    verified: u64,
) -> Result<()> {
    let expected = manifest
        .uploads
        .as_ref()
        .expect("validated uploads report")
        .blob_files;
    let ledger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_blob WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if verified != expected || u64::try_from(ledger)? != expected {
        bail!(
            "blob reconciliation failed: expected {expected}, copied={verified}, ledger={ledger}"
        );
    }
    Ok(())
}

async fn load_config(config: &dyn ConfigStore, manifest: &PreflightReport) -> Result<Vec<String>> {
    let mut effective = Value::Object(Map::new());
    if let Some(path) = manifest.source.config_defaults.as_ref() {
        effective = read_json_object_verified(path, "config_defaults", manifest)?;
    }
    if let Some(path) = manifest.source.config_local.as_ref() {
        merge_json(
            &mut effective,
            read_json_object_verified(path, "config_local", manifest)?,
        );
    }
    let transformed = transform_classic_config(&effective);
    for (key, value) in &transformed {
        config.set(key, value).await?;
    }
    Ok(transformed.keys().cloned().collect())
}

fn read_json_object_verified(path: &Path, kind: &str, manifest: &PreflightReport) -> Result<Value> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading config {}", path.display()))?;
    let expected = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == kind)
        .with_context(|| format!("manifest has no `{kind}` artifact"))?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if bytes.len() as u64 != expected.bytes || digest != expected.sha256 {
        bail!("configuration `{kind}` changed after preflight");
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if !value.is_object() {
        bail!("config {} is not a JSON object", path.display());
    }
    Ok(value)
}

fn graph_class_name(class: GraphClass) -> &'static str {
    match class {
        GraphClass::Public => "public",
        GraphClass::User => "user",
        GraphClass::Other => "other",
    }
}

fn reindex_idempotency_key(run_id: Uuid) -> String {
    format!("synbiohub-migration:{run_id}:reindex")
}

/// Rebuild the fixed-query accelerator for every verified imported graph.
///
/// Each graph commits independently and has a durable ledger row. A failure
/// therefore leaves already-verified graphs intact and a rerun resumes at the
/// first incomplete graph instead of repeating the whole production corpus.
async fn rebuild_accelerators(pool: &PgPool, run_id: Uuid) -> Result<u64> {
    sqlx::query(
        "INSERT INTO sbh_migration_accelerator (run_id, graph_iri, status) \
         SELECT run_id, graph_iri, 'pending' \
         FROM sbh_migration_graph \
         WHERE run_id = $1 AND status = 'verified' \
         ON CONFLICT (run_id, graph_iri) DO NOTHING",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("initializing query-accelerator ledger")?;

    let graphs = sqlx::query_scalar::<_, String>(
        "SELECT accelerator.graph_iri \
         FROM sbh_migration_accelerator accelerator \
         JOIN sbh_migration_graph graph \
           ON graph.run_id = accelerator.run_id \
          AND graph.graph_iri = accelerator.graph_iri \
         WHERE accelerator.run_id = $1 AND accelerator.status <> 'verified' \
         ORDER BY CASE WHEN graph.graph_class = 'public' THEN 0 ELSE 1 END, \
                  accelerator.graph_iri COLLATE \"C\"",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("listing incomplete query accelerators")?;
    let accelerator = AccelRepository::new(pool.clone(), TripleRepository::new(pool.clone()));

    for graph in graphs {
        sqlx::query(
            "UPDATE sbh_migration_accelerator \
             SET status = 'building', error = NULL, updated_at = now() \
             WHERE run_id = $1 AND graph_iri = $2",
        )
        .bind(run_id)
        .bind(&graph)
        .execute(pool)
        .await
        .with_context(|| format!("marking query accelerator building for {graph}"))?;

        let mut tx = pool
            .begin()
            .await
            .with_context(|| format!("starting query accelerator transaction for {graph}"))?;
        let refresh = accelerator.refresh_graph(&mut tx, &graph).await;
        if let Err(error) = refresh {
            let _ = tx.rollback().await;
            let message = format!("{error:#}");
            let _ = sqlx::query(
                "UPDATE sbh_migration_accelerator \
                 SET status = 'failed', error = $3, updated_at = now() \
                 WHERE run_id = $1 AND graph_iri = $2",
            )
            .bind(run_id)
            .bind(&graph)
            .bind(&message)
            .execute(pool)
            .await;
            return Err(anyhow::anyhow!(error))
                .with_context(|| format!("rebuilding query accelerator for {graph}"));
        }
        sqlx::query(
            "UPDATE sbh_migration_accelerator \
             SET status = 'verified', error = NULL, updated_at = now() \
             WHERE run_id = $1 AND graph_iri = $2",
        )
        .bind(run_id)
        .bind(&graph)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("verifying query accelerator ledger for {graph}"))?;
        tx.commit()
            .await
            .with_context(|| format!("committing query accelerator for {graph}"))?;
    }

    let expected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_graph \
         WHERE run_id = $1 AND status = 'verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_accelerator \
         WHERE run_id = $1 AND status = 'verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if verified != expected {
        bail!(
            "query accelerator reconciliation mismatch: expected {expected} verified graphs, found {verified}"
        );
    }
    sqlx::query(
        "UPDATE sbh_migration_run \
         SET accelerators_completed_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("marking query accelerators complete")?;
    u64::try_from(verified).context("query accelerator count is negative")
}

async fn summarize_ready_run(
    pool: &PgPool,
    run_id: Uuid,
    manifest: &PreflightReport,
    artifacts_verified: u64,
) -> Result<ProductionReport> {
    let users_verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_identity WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let graphs_verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_graph WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let triples_verified: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(loaded_quads),0)::bigint FROM sbh_migration_graph \
         WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let accelerators_verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_accelerator \
         WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let blobs_verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_blob WHERE run_id=$1 AND status='verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let reindex_job = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM sbol_jobs WHERE kind=$1 AND idempotency_key=$2 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(REINDEX_KIND)
    .bind(reindex_idempotency_key(run_id))
    .fetch_optional(pool)
    .await?
    .map(|id| id.to_string());
    Ok(ProductionReport {
        run_id,
        source_bundle_sha256: manifest.source_bundle_sha256.clone(),
        status: "ready".to_owned(),
        artifacts_verified,
        users_verified: u64::try_from(users_verified)?,
        reset_links_invalidated: manifest
            .sqlite
            .as_ref()
            .map(|sqlite| sqlite.active_reset_links)
            .unwrap_or_default(),
        graphs_verified: u64::try_from(graphs_verified)?,
        triples_verified: u64::try_from(triples_verified)?,
        accelerators_verified: u64::try_from(accelerators_verified)?,
        blobs_verified: u64::try_from(blobs_verified)?,
        config_keys: manifest.config.target_config_keys.clone(),
        reindex_job,
        required_runtime_secrets: vec!["SBOL_DB_PASSWORD_SALT", "SBOL_DB_SHARE_LINK_SALT"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use sbol_db_app::{AuthService, FsBlobStore};
    use sbol_db_backend::Backend;
    use sbol_db_storage::BlobStore;
    use sqlx::sqlite::SqliteConnectOptions;

    #[test]
    fn deterministic_user_ids_are_stable_and_source_specific() {
        let a = deterministic_user_id(&"a".repeat(64), 7);
        assert_eq!(a, deterministic_user_id(&"a".repeat(64), 7));
        assert_ne!(a, deterministic_user_id(&"a".repeat(64), 8));
        assert_ne!(a, deterministic_user_id(&"b".repeat(64), 7));
    }

    #[test]
    fn upload_paths_must_be_strictly_relative() {
        assert!(validate_relative_path(Path::new("ab/cdef.gz")).is_ok());
        assert!(validate_relative_path(Path::new("a/./b")).is_ok());
        assert!(validate_relative_path(Path::new("../escape")).is_err());
        assert!(validate_relative_path(Path::new("/absolute")).is_err());
    }

    #[test]
    fn classic_timestamps_preserve_subseconds_and_offset() {
        let parsed = parse_classic_timestamp(Some("2017-02-27 16:03:01.464 +00:00"))
            .expect("parse production timestamp");
        assert_eq!(parsed.to_rfc3339(), "2017-02-27T16:03:01.464+00:00");
    }

    #[tokio::test]
    async fn rehearses_manifest_gated_loader_on_postgres_when_configured() {
        let Ok(database_url) = std::env::var("SBOL_DB_MIGRATION_TEST_POSTGRES_URL") else {
            eprintln!(
                "SBOL_DB_MIGRATION_TEST_POSTGRES_URL is unset; skipping production loader rehearsal"
            );
            return;
        };
        let work = tempfile::tempdir().expect("migration workdir");
        let fixtures =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synbiohub-mini");
        let sqlite_path = work.path().join("synbiohub.sqlite");
        let sqlite = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&sqlite_path)
                .create_if_missing(true),
        )
        .await
        .expect("create classic SQLite");
        sqlx::query(
            "CREATE TABLE user (id INTEGER PRIMARY KEY, name TEXT, username TEXT, email TEXT, \
             affiliation TEXT, password TEXT, graphUri TEXT, isAdmin INTEGER, isCurator INTEGER, \
             isMember INTEGER, resetPasswordLink TEXT, createdAt TEXT, updatedAt TEXT)",
        )
        .execute(&sqlite)
        .await
        .expect("classic schema");
        sqlx::query(
            "INSERT INTO user VALUES \
             (1,'Alice Example','alice','alice@example.org','Synthetic Lab', \
              '13b2c1600e24b689e67d72da2e660bdf3c409a1c', \
              'http://synbiohub.org/user/alice',1,0,1,'invalidate-me', \
              '2017-02-27 16:03:01.464 +00:00','2017-05-17 15:35:10.367 +00:00')",
        )
        .execute(&sqlite)
        .await
        .expect("classic user");
        sqlx::query(
            "INSERT INTO user VALUES \
             (2,'Bob Example','bob','alice@example.org','Synthetic Lab', \
              '13b2c1600e24b689e67d72da2e660bdf3c409a1c', \
              'http://synbiohub.org/user/bob',0,1,0,NULL, \
              '2018-02-27 16:03:01.464 +00:00','2018-05-17 15:35:10.367 +00:00')",
        )
        .execute(&sqlite)
        .await
        .expect("classic duplicate-email user");
        sqlite.close().await;

        let manifest = super::super::preflight::inspect(super::super::preflight::PreflightInputs {
            source: None,
            virtuoso_db: None,
            rdf: Some(fixtures.join("dump.nq")),
            rdf_normalization_report: None,
            sqlite: Some(sqlite_path),
            uploads: Some(fixtures.join("uploads")),
            config: Some(fixtures.join("config.local.json")),
            config_defaults: None,
            report: None,
            allow_blockers: false,
        })
        .await
        .expect("preflight synthetic source");
        assert!(manifest.ready_for_import);
        let manifest_path = work.path().join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
        )
        .expect("write manifest");

        let backend = Backend::open(&database_url).await.expect("open Postgres");
        let blob_store = work.path().join("blobs");
        run(
            backend.require_postgres().expect("Postgres").pool.clone(),
            backend.config.clone(),
            backend.jobs.clone(),
            ProductionInputs {
                manifest: manifest_path.clone(),
                policy: None,
                blob_store: blob_store.clone(),
                chunk_size: 3,
                no_reindex: false,
            },
        )
        .await
        .expect("first production load");

        let auth = AuthService::new(backend.users.clone(), backend.tokens.clone());
        let alice = auth
            .authenticate("alice", "hunter2", "synthetic_salt")
            .await
            .expect("legacy credentials survive");
        assert!(alice.is_admin);
        assert_eq!(alice.graph_uri, "http://synbiohub.org/user/alice");
        let bob = auth
            .authenticate("bob", "hunter2", "synthetic_salt")
            .await
            .expect("second legacy credential survives");
        assert!(bob.is_curator);
        let ambiguous = auth
            .authenticate("alice@example.org", "hunter2", "synthetic_salt")
            .await
            .expect_err("duplicate email must not select an arbitrary account");
        assert!(ambiguous
            .to_string()
            .contains("multiple accounts use this email"));
        let stored = backend
            .users
            .find_by_email_or_username("alice")
            .await
            .expect("lookup")
            .expect("alice");
        assert!(stored.reset_password_link.is_none());

        let blobs = FsBlobStore::new(&blob_store);
        assert!(blobs
            .get("04b9beedcedc38daf4ff574b3d4bb291f2bbcaf0")
            .await
            .expect("blob read")
            .is_some());
        let mut graph_cursor = None;
        let mut public_triples = 0usize;
        loop {
            let page = backend
                .store
                .graph_store_read_page("http://synbiohub.org/public", graph_cursor.as_deref(), 2)
                .await
                .expect("read imported public graph page");
            public_triples += page.items.len();
            graph_cursor = page.next_cursor;
            if graph_cursor.is_none() {
                break;
            }
        }
        assert_eq!(public_triples, 7);
        let status: String = sqlx::query_scalar("SELECT status FROM sbh_migration_run")
            .fetch_one(&backend.require_postgres().expect("Postgres").pool)
            .await
            .expect("run status");
        assert_eq!(status, "ready");
        let accelerators: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sbh_migration_accelerator WHERE status = 'verified'",
        )
        .fetch_one(&backend.require_postgres().expect("Postgres").pool)
        .await
        .expect("verified accelerator count");
        let imported_graphs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sbh_migration_graph WHERE status = 'verified'",
        )
        .fetch_one(&backend.require_postgres().expect("Postgres").pool)
        .await
        .expect("verified graph count");
        assert_eq!(accelerators, imported_graphs);
        let accelerated_objects: i64 = sqlx::query_scalar("SELECT count(*) FROM accel_object")
            .fetch_one(&backend.require_postgres().expect("Postgres").pool)
            .await
            .expect("accelerated object count");
        assert!(accelerated_objects > 0);

        // A second invocation re-verifies the target and returns the same run
        // rather than duplicating users, triples, or files.
        run(
            backend.require_postgres().expect("Postgres").pool.clone(),
            backend.config.clone(),
            backend.jobs.clone(),
            ProductionInputs {
                manifest: manifest_path,
                policy: None,
                blob_store,
                chunk_size: 2,
                no_reindex: false,
            },
        )
        .await
        .expect("idempotent ready re-verification");
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM sbh_user")
            .fetch_one(&backend.require_postgres().expect("Postgres").pool)
            .await
            .expect("user count");
        assert_eq!(users, 2);
        let reindex_jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sbol_jobs WHERE kind='rebuild_search_index'")
                .fetch_one(&backend.require_postgres().expect("Postgres").pool)
                .await
                .expect("reindex job count");
        assert_eq!(reindex_jobs, 1);
        let accelerators_after_rerun: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sbh_migration_accelerator WHERE status = 'verified'",
        )
        .fetch_one(&backend.require_postgres().expect("Postgres").pool)
        .await
        .expect("accelerator count after rerun");
        assert_eq!(accelerators_after_rerun, accelerators);
    }
}
