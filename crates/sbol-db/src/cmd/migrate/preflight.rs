//! Read-only, fail-closed inspection of a classic SynBioHub source snapshot.
//!
//! This module deliberately has no SBOL DB backend dependency. It inventories
//! and hashes the copied source, reads SQLite from a private temporary copy so
//! WAL handling cannot touch the supplied files, validates every content-
//! addressed upload, and streams an optional RDF export into per-graph counts
//! and order-independent fingerprints.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use flate2::read::GzDecoder;
use oxrdf::{GraphName, NamedOrBlankNode, Term, Triple};
use oxrdfio::{RdfFormat, RdfParser};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};

use crate::cmd::migrate::rdf_format_for;
use crate::output::print_json;

pub(crate) const MANIFEST_SCHEMA: &str = "sbol-db.synbiohub-preflight.v1";
const REDACTED_CONFIG_VALUE: &str = "[REDACTED: read from verified source at import time]";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SBOL2_COLLECTION: &str = "http://sbols.org/v2#Collection";
const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
const SBH_ATTACHMENT_HASH: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash";
const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";
const SBH_CAN_VIEW: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#canView";

/// Source paths and operator controls for a preflight run.
#[derive(Debug)]
pub struct PreflightInputs {
    pub source: Option<PathBuf>,
    pub virtuoso_db: Option<PathBuf>,
    pub rdf: Option<PathBuf>,
    pub sqlite: Option<PathBuf>,
    pub uploads: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub config_defaults: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub allow_blockers: bool,
}

/// Fully resolved inputs. Paths are recorded in the report for operators but
/// are excluded from the content-derived bundle identifier.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResolvedSource {
    pub root: Option<PathBuf>,
    pub virtuoso_db: Option<PathBuf>,
    pub rdf: Option<PathBuf>,
    pub sqlite: Option<PathBuf>,
    pub sqlite_wal: Option<PathBuf>,
    pub sqlite_shm: Option<PathBuf>,
    /// Other SQLite-looking siblings are never silently treated as canonical.
    /// They are hashed into the bundle and require an operator waiver that
    /// records why the selected account database is authoritative.
    #[serde(default)]
    pub additional_sqlite: Vec<PathBuf>,
    pub uploads: Option<PathBuf>,
    pub config_local: Option<PathBuf>,
    pub config_defaults: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArtifactReport {
    pub kind: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub modified_unix_seconds: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Blocker,
    Warning,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueScope {
    Source,
    Target,
    Policy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightIssue {
    pub severity: IssueSeverity,
    pub scope: IssueScope,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RoleCounts {
    pub administrators: u64,
    pub curators: u64,
    pub members: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IdentityCollision {
    /// `username`, `email`, `username_case_folded`, or `email_case_folded`.
    pub field: String,
    /// A digest permits stable reconciliation without writing the identifier
    /// (especially an email address) into an ordinary report.
    pub value_sha256: String,
    pub source_user_ids: Vec<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SqliteReport {
    pub quick_check: String,
    pub journal_mode: String,
    pub user_table: String,
    pub users: u64,
    /// SHA-256 over every classic account field in source-id order. This
    /// commits to password hashes and reset-link values without disclosing
    /// either in the reviewable manifest.
    pub user_rows_sha256: String,
    pub sessions: u64,
    pub jobs: u64,
    pub tasks: u64,
    pub external_profiles: u64,
    pub roles: RoleCounts,
    pub active_reset_links: u64,
    pub missing_username: u64,
    pub missing_email: u64,
    pub missing_password_hash: u64,
    pub missing_graph_uri: u64,
    pub exact_collisions: Vec<IdentityCollision>,
    pub case_folded_collisions: Vec<IdentityCollision>,
    pub created_at_min: Option<String>,
    pub created_at_max: Option<String>,
    pub updated_at_min: Option<String>,
    pub updated_at_max: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BlobEntry {
    pub relative_path: PathBuf,
    pub expected_sha1: Option<String>,
    pub content_sha1: Option<String>,
    pub compressed_sha256: Option<String>,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: Option<u64>,
    pub valid_gzip: bool,
    pub address_matches_content: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UploadAsset {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct UploadReport {
    pub tree_sha256: String,
    pub blob_files: u64,
    pub valid_gzip_files: u64,
    pub address_matches_content: u64,
    pub invalid_files: u64,
    pub total_compressed_bytes: u64,
    pub total_uncompressed_bytes: u64,
    pub blobs: Vec<BlobEntry>,
    pub assets: Vec<UploadAsset>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphClass {
    Public,
    User,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RdfGraphReport {
    pub iri: String,
    pub class: GraphClass,
    pub quads: u64,
    /// SHA-256 over `(count, modular sum of triple SHA-256 values, XOR of
    /// triple SHA-256 values)`. It is order independent and duplicate aware.
    pub fingerprint: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ClassifiedCounts {
    pub public: u64,
    pub user: u64,
    pub other: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RdfReport {
    pub total_quads: u64,
    pub default_graph_quads: u64,
    pub blank_named_graph_quads: u64,
    pub graph_counts: ClassifiedCounts,
    pub quad_counts: ClassifiedCounts,
    pub graphs: Vec<RdfGraphReport>,
    pub owned_top_levels: ClassifiedCounts,
    pub collections: ClassifiedCounts,
    pub distinct_owners: u64,
    pub shared_objects: u64,
    pub share_grants: u64,
    pub share_viewers: u64,
    pub referenced_blob_hashes: u64,
    pub referenced_blob_hashes_by_class: ClassifiedCounts,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ReconciliationReport {
    pub account_graphs_present: u64,
    pub accounts_without_rdf_graph: u64,
    pub user_graphs_without_account: Vec<String>,
    pub owner_principals_without_account: Vec<String>,
    pub share_viewers_without_account: Vec<String>,
    pub referenced_blobs_present: u64,
    pub referenced_blobs_missing: Vec<String>,
    pub referenced_public_blobs_present: u64,
    pub referenced_public_blobs_missing: Vec<String>,
    pub unreferenced_blob_files: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct EffectiveConfigReport {
    pub source_keys: Vec<String>,
    pub target_config_keys: Vec<String>,
    pub database_prefix: String,
    pub public_graph: String,
    pub user_graph_prefix: String,
    pub instance_url: Option<String>,
    pub frontend_url: Option<String>,
    pub require_login: bool,
    pub allow_public_signup: bool,
    pub revision: Option<String>,
    pub legacy_password_salt_available: bool,
    pub share_link_salt_available: bool,
    pub target_config_preview: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightReport {
    pub schema: String,
    pub generated_at: String,
    pub source_bundle_sha256: String,
    pub ready_for_import: bool,
    pub source: ResolvedSource,
    pub artifacts: Vec<ArtifactReport>,
    pub config: EffectiveConfigReport,
    pub sqlite: Option<SqliteReport>,
    pub uploads: Option<UploadReport>,
    pub rdf: Option<RdfReport>,
    pub reconciliation: Option<ReconciliationReport>,
    pub issues: Vec<PreflightIssue>,
}

#[derive(Clone, Debug, Serialize)]
struct PreflightSummary<'a> {
    schema: &'a str,
    source_bundle_sha256: &'a str,
    ready_for_import: bool,
    report: &'a Path,
    blockers: usize,
    warnings: usize,
}

#[derive(Default)]
struct AccountFacts {
    graph_to_ids: BTreeMap<String, Vec<i64>>,
}

#[derive(Default)]
struct RdfFacts {
    graphs: BTreeSet<String>,
    owners: BTreeSet<String>,
    viewers: BTreeSet<String>,
    referenced_blobs: BTreeMap<String, BTreeSet<GraphClass>>,
}

/// Execute a preflight, write the complete report before returning a blocking
/// status, and avoid opening any target database.
pub async fn run(inputs: PreflightInputs) -> Result<()> {
    let report_path = inputs.report.clone();
    let allow_blockers = inputs.allow_blockers;
    let report = inspect(inputs).await?;

    if let Some(path) = &report_path {
        write_report_atomic(path, &report)?;
        let blockers = report
            .issues
            .iter()
            .filter(|issue| issue.severity == IssueSeverity::Blocker)
            .count();
        let warnings = report.issues.len() - blockers;
        print_json(&PreflightSummary {
            schema: &report.schema,
            source_bundle_sha256: &report.source_bundle_sha256,
            ready_for_import: report.ready_for_import,
            report: path,
            blockers,
            warnings,
        })?;
    } else {
        print_json(&report)?;
    }

    if !report.ready_for_import && !allow_blockers {
        bail!(
            "SynBioHub preflight found blocking issues; inspect the emitted report or pass \
             --allow-blockers only for inventory work"
        );
    }
    Ok(())
}

/// Inspect and reconcile one source bundle. Exposed within the crate for
/// production-shaped tests and for the eventual resumable importer gate.
pub(crate) async fn inspect(inputs: PreflightInputs) -> Result<PreflightReport> {
    let source = resolve_source(&inputs)?;
    let mut issues = Vec::new();

    require_path(
        &source.sqlite,
        "sqlite_missing",
        "the classic account SQLite database is required",
        &mut issues,
    );
    require_path(
        &source.uploads,
        "uploads_missing",
        "the classic uploads directory is required",
        &mut issues,
    );
    require_path(
        &source.config_local,
        "config_local_missing",
        "config.local.json is required",
        &mut issues,
    );
    if source.rdf.is_none() && source.virtuoso_db.is_none() {
        issue(
            &mut issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "rdf_source_missing",
            "neither a raw Virtuoso database nor an RDF export was found",
            None,
        );
    } else if source.rdf.is_none() {
        issue(
            &mut issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "rdf_export_required",
            "the raw Virtuoso database is inventoried, but a graph-bearing N-Quads or TriG export is required for reconciliation and import",
            None,
        );
    }
    if !source.additional_sqlite.is_empty() {
        issue(
            &mut issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "additional_sqlite_snapshot_requires_disposition",
            "additional SQLite snapshots were found beside the selected account database; confirm that they are historical/non-canonical before import",
            Some(source.additional_sqlite.len() as u64),
        );
    }

    let (config_report, effective_config) = inspect_config(
        source.config_defaults.as_deref(),
        source.config_local.as_deref(),
        &mut issues,
    )?;

    let (sqlite_report, account_facts) = match &source.sqlite {
        Some(path) if path.is_file() => {
            let (report, facts) = inspect_sqlite(path, &mut issues).await?;
            (Some(report), facts)
        }
        _ => (None, AccountFacts::default()),
    };

    let uploads_report = match &source.uploads {
        Some(path) if path.is_dir() => Some(inspect_uploads(path, &mut issues)?),
        _ => None,
    };

    let (rdf_report, rdf_facts) = match &source.rdf {
        Some(path) if path.is_file() => {
            let (report, facts) = inspect_rdf(
                path,
                &config_report.public_graph,
                &config_report.user_graph_prefix,
                &mut issues,
            )?;
            (Some(report), facts)
        }
        _ => (None, RdfFacts::default()),
    };

    let reconciliation =
        if rdf_report.is_some() && sqlite_report.is_some() && uploads_report.is_some() {
            Some(reconcile(
                &account_facts,
                &rdf_facts,
                uploads_report.as_ref().expect("checked above"),
                &mut issues,
            ))
        } else {
            None
        };

    // Artifact hashes are intentionally computed after semantic inspection so
    // a concurrently changing source is likely to be noticed in the manifest.
    // Production acquisition should still freeze writes before this command.
    let mut artifacts = Vec::new();
    add_artifact(&mut artifacts, "virtuoso_db", source.virtuoso_db.as_deref())?;
    add_artifact(&mut artifacts, "rdf_export", source.rdf.as_deref())?;
    add_artifact(&mut artifacts, "sqlite", source.sqlite.as_deref())?;
    add_artifact(&mut artifacts, "sqlite_wal", source.sqlite_wal.as_deref())?;
    add_artifact(&mut artifacts, "sqlite_shm", source.sqlite_shm.as_deref())?;
    for (index, path) in source.additional_sqlite.iter().enumerate() {
        add_artifact(
            &mut artifacts,
            &format!("sqlite_additional_{index:04}"),
            Some(path),
        )?;
    }
    add_artifact(
        &mut artifacts,
        "config_local",
        source.config_local.as_deref(),
    )?;
    add_artifact(
        &mut artifacts,
        "config_defaults",
        source.config_defaults.as_deref(),
    )?;
    artifacts.sort_by(|a, b| a.kind.cmp(&b.kind));

    let source_bundle_sha256 = bundle_fingerprint(
        &artifacts,
        uploads_report.as_ref().map(|u| u.tree_sha256.as_str()),
        &effective_config,
    )?;
    let ready_for_import = !issues
        .iter()
        .any(|issue| issue.severity == IssueSeverity::Blocker);

    let report = PreflightReport {
        schema: MANIFEST_SCHEMA.to_owned(),
        generated_at: Utc::now().to_rfc3339(),
        source_bundle_sha256,
        ready_for_import,
        source,
        artifacts,
        config: config_report,
        sqlite: sqlite_report,
        uploads: uploads_report,
        rdf: rdf_report,
        reconciliation,
        issues,
    };
    ensure_report_contains_no_config_secrets(&report, &effective_config)?;
    Ok(report)
}

fn resolve_source(inputs: &PreflightInputs) -> Result<ResolvedSource> {
    if inputs.source.is_none()
        && inputs.virtuoso_db.is_none()
        && inputs.rdf.is_none()
        && inputs.sqlite.is_none()
        && inputs.uploads.is_none()
        && inputs.config.is_none()
    {
        bail!("pass --source or explicit source-component paths");
    }
    let root = inputs.source.as_deref();
    let virtuoso_db = explicit_or_discovered(
        &inputs.virtuoso_db,
        root,
        &["virtuoso.db", "virtuoso/virtuoso.db"],
    );
    let rdf = explicit_or_discovered(
        &inputs.rdf,
        root,
        &["dump.nq", "dump.nquads", "dump.trig", "dumps/dump.nq"],
    );
    let sqlite = explicit_or_discovered(
        &inputs.sqlite,
        root,
        &["synbiohub.sqlite", "sbhData/data/synbiohub.sqlite"],
    );
    let uploads =
        explicit_or_discovered(&inputs.uploads, root, &["uploads", "sbhData/data/uploads"]);
    let config_local = explicit_or_discovered(
        &inputs.config,
        root,
        &["config.local.json", "sbhData/config/config.local.json"],
    );
    let config_defaults = explicit_or_discovered(
        &inputs.config_defaults,
        root,
        &["config.json", "sbhData/config/config.json"],
    );
    let sqlite_wal = sqlite
        .as_ref()
        .map(|path| sidecar(path, "-wal"))
        .filter(|p| p.is_file());
    let sqlite_shm = sqlite
        .as_ref()
        .map(|path| sidecar(path, "-shm"))
        .filter(|p| p.is_file());
    let additional_sqlite = discover_additional_sqlite(sqlite.as_deref())?;
    Ok(ResolvedSource {
        root: inputs.source.clone(),
        virtuoso_db,
        rdf,
        sqlite,
        sqlite_wal,
        sqlite_shm,
        additional_sqlite,
        uploads,
        config_local,
        config_defaults,
    })
}

fn discover_additional_sqlite(selected: Option<&Path>) -> Result<Vec<PathBuf>> {
    let Some(selected) = selected else {
        return Ok(Vec::new());
    };
    let Some(parent) = selected.parent() else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(parent)
        .with_context(|| format!("scanning SQLite directory {}", parent.display()))?
    {
        let path = entry?.path();
        if path != selected
            && path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "sqlite")
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    Ok(candidates)
}

fn explicit_or_discovered(
    explicit: &Option<PathBuf>,
    root: Option<&Path>,
    candidates: &[&str],
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.clone());
    }
    root.and_then(|root| {
        candidates
            .iter()
            .map(|candidate| root.join(candidate))
            .find(|path| path.exists())
    })
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn require_path(
    path: &Option<PathBuf>,
    code: &str,
    message: &str,
    issues: &mut Vec<PreflightIssue>,
) {
    if path.as_ref().is_none_or(|path| !path.exists()) {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            code,
            message,
            None,
        );
    }
}

fn issue(
    issues: &mut Vec<PreflightIssue>,
    severity: IssueSeverity,
    scope: IssueScope,
    code: impl Into<String>,
    message: impl Into<String>,
    count: Option<u64>,
) {
    issues.push(PreflightIssue {
        severity,
        scope,
        code: code.into(),
        message: message.into(),
        count,
    });
}

fn add_artifact(
    artifacts: &mut Vec<ArtifactReport>,
    kind: &str,
    path: Option<&Path>,
) -> Result<()> {
    if let Some(path) = path.filter(|path| path.is_file()) {
        artifacts.push(hash_artifact(kind, path)?);
    }
    Ok(())
}

pub(crate) fn hash_artifact(kind: &str, path: &Path) -> Result<ArtifactReport> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;
    let mut reader = BufReader::with_capacity(
        1024 * 1024,
        File::open(path).with_context(|| format!("opening {}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 1024 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("hashing {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let modified_unix_seconds = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_secs()).ok());
    Ok(ArtifactReport {
        kind: kind.to_owned(),
        path: path.to_path_buf(),
        bytes: metadata.len(),
        sha256: hex::encode(hasher.finalize()),
        modified_unix_seconds,
    })
}

fn bundle_fingerprint(
    artifacts: &[ArtifactReport],
    upload_tree_sha256: Option<&str>,
    effective_config: &Value,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST_SCHEMA.as_bytes());
    for artifact in artifacts {
        hasher.update(artifact.kind.as_bytes());
        hasher.update([0]);
        hasher.update(artifact.bytes.to_be_bytes());
        hasher.update(artifact.sha256.as_bytes());
    }
    if let Some(tree) = upload_tree_sha256 {
        hasher.update(b"uploads\0");
        hasher.update(tree.as_bytes());
    }
    // The effective configuration is included only through a digest; secret
    // values never appear in the report.
    hasher.update(b"effective_config\0");
    hasher.update(serde_json::to_vec(effective_config)?);
    Ok(hex::encode(hasher.finalize()))
}

fn write_report_atomic(path: &Path, report: &PreflightReport) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating report directory {}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary report in {}", parent.display()))?;
    serde_json::to_writer_pretty(&mut temp, report)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| anyhow!("persisting report {}: {}", path.display(), error.error))?;
    Ok(())
}

fn inspect_config(
    defaults: Option<&Path>,
    local: Option<&Path>,
    issues: &mut Vec<PreflightIssue>,
) -> Result<(EffectiveConfigReport, Value)> {
    let mut effective = Value::Object(Map::new());
    if let Some(path) = defaults.filter(|path| path.is_file()) {
        effective = read_json_object(path)?;
    } else {
        issue(
            issues,
            IssueSeverity::Warning,
            IssueScope::Source,
            "config_defaults_missing",
            "classic config.json defaults were not supplied; the effective configuration may be incomplete",
            None,
        );
    }
    if let Some(path) = local.filter(|path| path.is_file()) {
        let overlay = read_json_object(path)?;
        merge_json(&mut effective, overlay);
    }

    let source = effective.as_object().cloned().unwrap_or_default();
    let database_prefix = value_text(&effective, &["databasePrefix"])
        .unwrap_or_else(|| "http://synbiohub.org/".to_owned());
    let database_prefix = ensure_trailing_slash(&database_prefix);
    let public_graph = value_text(&effective, &["triplestore", "defaultGraph"])
        .unwrap_or_else(|| format!("{database_prefix}public"));
    let user_graph_prefix = format!("{database_prefix}user/");
    let password_salt = value_text(&effective, &["passwordSalt"]);
    let share_link_salt = value_text(&effective, &["shareLinkSalt"]);
    if password_salt.as_deref().is_none_or(str::is_empty) {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "legacy_password_salt_unavailable",
            "the effective classic password salt is unavailable; migrated credentials cannot be verified without it",
            None,
        );
    }

    let target_config = transform_classic_config(&effective);
    let target_config_preview = target_config
        .iter()
        .map(|(key, value)| (key.clone(), redact_config_for_report(value)))
        .collect::<BTreeMap<_, _>>();
    let mut source_keys = source.keys().cloned().collect::<Vec<_>>();
    source_keys.sort();
    let target_config_keys = target_config_preview.keys().cloned().collect();
    Ok((
        EffectiveConfigReport {
            source_keys,
            target_config_keys,
            database_prefix,
            public_graph,
            user_graph_prefix,
            instance_url: value_text(&effective, &["instanceUrl"]),
            frontend_url: value_text(&effective, &["frontendURL"]),
            require_login: value_bool(&effective, &["requireLogin"]).unwrap_or(false),
            allow_public_signup: value_bool(&effective, &["allowPublicSignup"]).unwrap_or(false),
            revision: value_text(&effective, &["revision"]),
            legacy_password_salt_available: password_salt.is_some_and(|v| !v.is_empty()),
            share_link_salt_available: share_link_salt.is_some_and(|v| !v.is_empty()),
            target_config_preview,
        },
        effective,
    ))
}

fn read_json_object(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading configuration {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing configuration {}", path.display()))?;
    if !value.is_object() {
        bail!("configuration {} is not a JSON object", path.display());
    }
    Ok(value)
}

pub(crate) fn merge_json(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Convert classic top-level configuration into the canonical target
/// documents consumed by the native server. The importer applies this to the
/// hash-verified source so integration credentials survive; the separately
/// archived preview is recursively redacted before serialization.
pub(crate) fn transform_classic_config(config: &Value) -> BTreeMap<String, Value> {
    const THEME_KEYS: &[&str] = &[
        "instanceName",
        "frontendURL",
        "instanceUrl",
        "frontPageText",
        "altHome",
        "currentTheme",
        "themeParameters",
        "showModuleInteractions",
        "removePublicEnabled",
        "allowPublicSignup",
        "requireLogin",
        "pluginsUseLocalCompose",
        "pluginLocalComposePrefix",
        "suppressInfoLogs",
        "suppressDebugLogs",
        "suppressWarningLogs",
        "suppressErrorLogs",
    ];
    let mut result = BTreeMap::new();
    let mut theme = Map::new();
    for key in THEME_KEYS {
        if let Some(value) = config.get(*key) {
            theme.insert((*key).to_owned(), value.clone());
        }
    }
    if let Some(prefix) = config.get("databasePrefix") {
        theme.insert("uriPrefix".to_owned(), prefix.clone());
    }
    result.insert("theme".to_owned(), Value::Object(theme));
    if let Some(prefix) = config.get("databasePrefix").and_then(Value::as_str) {
        let prefix = ensure_trailing_slash(prefix);
        let public_graph = value_text(config, &["triplestore", "defaultGraph"])
            .unwrap_or_else(|| format!("{prefix}public"));
        result.insert(
            "registryNamespace".to_owned(),
            serde_json::json!({
                "databasePrefix": prefix,
                "publicGraph": public_graph,
            }),
        );
    }

    for key in [
        "mail",
        "remotes",
        "webOfRegistries",
        "usersConfig",
        "plugins",
        "collectionIcons",
    ] {
        if let Some(value) = config.get(key) {
            result.insert(key.to_owned(), value.clone());
        }
    }
    // Keep the classic names available to compatibility/federation readers,
    // while the canonical theme document drives native instance behavior.
    for key in ["databasePrefix", "instanceUrl", "frontendURL", "revision"] {
        if let Some(value) = config.get(key) {
            result.insert(key.to_owned(), value.clone());
        }
    }
    result
}

/// Recursively replace configuration values whose keys conventionally carry
/// credentials. The manifest is designed to be archived with run evidence, so
/// it must never become a second secret store. The importer reconstructs the
/// unredacted target documents from the hash-verified source config instead.
fn redact_config_for_report(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if config_key_is_secret(key) {
                        Value::String(REDACTED_CONFIG_VALUE.to_owned())
                    } else {
                        redact_config_for_report(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_config_for_report).collect()),
        _ => value.clone(),
    }
}

fn config_key_is_secret(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized == "key"
        || normalized == "authorization"
        || [
            "password",
            "salt",
            "secret",
            "token",
            "apikey",
            "accesskey",
            "signingkey",
            "privatekey",
            "credential",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn ensure_report_contains_no_config_secrets(
    report: &PreflightReport,
    effective_config: &Value,
) -> Result<()> {
    let report_value = serde_json::to_value(report)?;
    let mut secrets = Vec::new();
    collect_config_secrets(effective_config, false, &mut secrets);
    let mut leaked_at = BTreeSet::new();
    for secret in secrets {
        if secret.len() >= 8 {
            find_secret_occurrences(&report_value, &secret, "$", &mut leaked_at);
        }
    }
    if !leaked_at.is_empty() {
        bail!(
            "refusing to emit preflight report because a source configuration secret survived redaction at report field(s): {}",
            leaked_at.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

/// Locate a leaked value without ever returning or formatting that value. The
/// structural report paths make redaction failures actionable while keeping
/// diagnostics safe to preserve in CI and migration run logs.
fn find_secret_occurrences(value: &Value, secret: &str, path: &str, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == secret {
                    output.insert(format!("{path}/<object-key>"));
                }
                let key = key.replace('~', "~0").replace('/', "~1");
                find_secret_occurrences(value, secret, &format!("{path}/{key}"), output);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                find_secret_occurrences(value, secret, &format!("{path}/{index}"), output);
            }
        }
        Value::String(value) if value == secret => {
            output.insert(path.to_owned());
        }
        _ => {}
    }
}

fn collect_config_secrets(value: &Value, secret_context: bool, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                collect_config_secrets(value, secret_context || config_key_is_secret(key), output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_config_secrets(value, secret_context, output);
            }
        }
        Value::String(value) if secret_context && !value.is_empty() => output.push(value.clone()),
        _ => {}
    }
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| value.get(*key))
}

fn value_text(value: &Value, path: &[&str]) -> Option<String> {
    value_at(value, path)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn value_bool(value: &Value, path: &[&str]) -> Option<bool> {
    value_at(value, path).and_then(Value::as_bool)
}

fn ensure_trailing_slash(value: &str) -> String {
    if value.ends_with('/') {
        value.to_owned()
    } else {
        format!("{value}/")
    }
}

#[derive(Clone)]
struct SourceUser {
    id: i64,
    username: String,
    email: String,
    graph_uri: String,
}

async fn inspect_sqlite(
    path: &Path,
    issues: &mut Vec<PreflightIssue>,
) -> Result<(SqliteReport, AccountFacts)> {
    // SQLite may update a `-shm` file even for a read-only WAL connection. Work
    // exclusively on a private copy so the supplied snapshot remains immutable.
    let temp = tempfile::Builder::new()
        .prefix("sbol-db-synbiohub-sqlite-")
        .tempdir()
        .context("creating private SQLite inspection directory")?;
    let copy = temp.path().join("synbiohub.sqlite");
    std::fs::copy(path, &copy)
        .with_context(|| format!("copying SQLite snapshot {}", path.display()))?;
    for suffix in ["-wal", "-shm"] {
        let source = sidecar(path, suffix);
        if source.is_file() {
            std::fs::copy(&source, sidecar(&copy, suffix))
                .with_context(|| format!("copying SQLite sidecar {}", source.display()))?;
        }
    }

    let options = SqliteConnectOptions::new()
        .filename(&copy)
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("opening private SQLite copy of {}", path.display()))?;

    let result = inspect_sqlite_pool(&pool, issues).await;
    pool.close().await;
    result
}

async fn inspect_sqlite_pool(
    pool: &SqlitePool,
    issues: &mut Vec<PreflightIssue>,
) -> Result<(SqliteReport, AccountFacts)> {
    let quick_check: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(pool)
        .await
        .context("running SQLite quick_check")?;
    if quick_check != "ok" {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "sqlite_integrity_failure",
            "SQLite quick_check did not return ok",
            None,
        );
    }
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(pool)
        .await
        .context("reading SQLite journal mode")?;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(pool)
            .await
            .context("listing SQLite tables")?;
    let table_set = tables.iter().cloned().collect::<BTreeSet<_>>();
    let user_table = if table_set.contains("user") {
        "user"
    } else if table_set.contains("users") {
        issue(
            issues,
            IssueSeverity::Warning,
            IssueScope::Source,
            "legacy_plural_user_table",
            "the source uses the non-production plural users table",
            None,
        );
        "users"
    } else {
        bail!("classic SQLite contains neither `user` nor `users`");
    };

    let rows = sqlx::query(&format!(
        "SELECT id, name, username, email, affiliation, password, graphUri, \
         isAdmin, isCurator, isMember, resetPasswordLink, createdAt, updatedAt \
         FROM \"{user_table}\" ORDER BY id"
    ))
    .fetch_all(pool)
    .await
    .context("reading classic users")?;

    let mut report = SqliteReport {
        quick_check,
        journal_mode,
        user_table: user_table.to_owned(),
        users: rows.len() as u64,
        sessions: count_table(pool, &table_set, "Sessions").await?,
        jobs: count_table(pool, &table_set, "job").await?,
        tasks: count_table(pool, &table_set, "task").await?,
        external_profiles: count_table(pool, &table_set, "user_external_profile").await?,
        ..SqliteReport::default()
    };

    let mut users = Vec::with_capacity(rows.len());
    let mut usernames: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut emails: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut usernames_folded: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut emails_folded: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut created = Vec::new();
    let mut updated = Vec::new();
    let mut user_fingerprint = Sha256::new();

    for row in rows {
        update_user_fingerprint(&mut user_fingerprint, &row)?;
        let id: i64 = row.try_get("id")?;
        let username = row
            .try_get::<Option<String>, _>("username")?
            .unwrap_or_default();
        let email = row
            .try_get::<Option<String>, _>("email")?
            .unwrap_or_default();
        let password = row
            .try_get::<Option<String>, _>("password")?
            .unwrap_or_default();
        let graph_uri = row
            .try_get::<Option<String>, _>("graphUri")?
            .unwrap_or_default();
        if username.is_empty() {
            report.missing_username += 1;
        }
        if email.is_empty() {
            report.missing_email += 1;
        }
        if password.is_empty() {
            report.missing_password_hash += 1;
        }
        if graph_uri.is_empty() {
            report.missing_graph_uri += 1;
        }
        if sqlite_bool(&row, "isAdmin")? {
            report.roles.administrators += 1;
        }
        if sqlite_bool(&row, "isCurator")? {
            report.roles.curators += 1;
        }
        if sqlite_bool(&row, "isMember")? {
            report.roles.members += 1;
        }
        if row
            .try_get::<Option<String>, _>("resetPasswordLink")?
            .is_some_and(|value| !value.is_empty())
        {
            report.active_reset_links += 1;
        }
        if let Some(value) = row.try_get::<Option<String>, _>("createdAt")? {
            created.push(value);
        }
        if let Some(value) = row.try_get::<Option<String>, _>("updatedAt")? {
            updated.push(value);
        }
        usernames.entry(username.clone()).or_default().push(id);
        emails.entry(email.clone()).or_default().push(id);
        usernames_folded
            .entry(username.to_lowercase())
            .or_default()
            .push(id);
        emails_folded
            .entry(email.to_lowercase())
            .or_default()
            .push(id);
        users.push(SourceUser {
            id,
            username,
            email,
            graph_uri,
        });
    }
    created.sort();
    updated.sort();
    report.created_at_min = created.first().cloned();
    report.created_at_max = created.last().cloned();
    report.updated_at_min = updated.first().cloned();
    report.updated_at_max = updated.last().cloned();
    report.user_rows_sha256 = hex::encode(user_fingerprint.finalize());
    report
        .exact_collisions
        .extend(collisions("username", usernames));
    report.exact_collisions.extend(collisions("email", emails));
    report
        .case_folded_collisions
        .extend(collisions("username_case_folded", usernames_folded));
    report
        .case_folded_collisions
        .extend(collisions("email_case_folded", emails_folded));

    let exact_email = report
        .exact_collisions
        .iter()
        .filter(|collision| collision.field == "email")
        .count() as u64;
    if exact_email > 0 {
        issue(
            issues,
            IssueSeverity::Warning,
            IssueScope::Policy,
            "duplicate_email_requires_username_login",
            "source accounts contain duplicate exact emails; all rows will be preserved and affected users must log in by username",
            Some(exact_email),
        );
    }
    if !report.case_folded_collisions.is_empty() {
        issue(
            issues,
            IssueSeverity::Warning,
            IssueScope::Policy,
            "case_folded_identity_collisions",
            "case-folding usernames or emails would merge distinct classic accounts",
            Some(report.case_folded_collisions.len() as u64),
        );
    }
    let required_missing = report.missing_username
        + report.missing_email
        + report.missing_password_hash
        + report.missing_graph_uri;
    if required_missing > 0 {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "account_required_fields_missing",
            "one or more classic accounts is missing a required identity field",
            Some(required_missing),
        );
    }

    let mut facts = AccountFacts::default();
    for user in users {
        if !user.graph_uri.is_empty() {
            facts
                .graph_to_ids
                .entry(user.graph_uri)
                .or_default()
                .push(user.id);
        }
        // Force the fields through parsing even though the report intentionally
        // excludes their values; this prevents a future schema drift from going
        // unnoticed behind a count-only query.
        let _ = (&user.username, &user.email);
    }
    Ok((report, facts))
}

/// Add one classic user row to the stable logical account fingerprint. JSON
/// array encoding preserves NULL/string/integer distinctions and length-prefix
/// framing prevents concatenation ambiguity.
pub(crate) fn update_user_fingerprint(
    hasher: &mut Sha256,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<()> {
    let encoded = serde_json::to_vec(&serde_json::json!([
        row.try_get::<i64, _>("id")?,
        row.try_get::<Option<String>, _>("name")?,
        row.try_get::<Option<String>, _>("username")?,
        row.try_get::<Option<String>, _>("email")?,
        row.try_get::<Option<String>, _>("affiliation")?,
        row.try_get::<Option<String>, _>("password")?,
        row.try_get::<Option<String>, _>("graphUri")?,
        row.try_get::<Option<i64>, _>("isAdmin")?,
        row.try_get::<Option<i64>, _>("isCurator")?,
        row.try_get::<Option<i64>, _>("isMember")?,
        row.try_get::<Option<String>, _>("resetPasswordLink")?,
        row.try_get::<Option<String>, _>("createdAt")?,
        row.try_get::<Option<String>, _>("updatedAt")?,
    ]))?;
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(())
}

async fn count_table(pool: &SqlitePool, tables: &BTreeSet<String>, table: &str) -> Result<u64> {
    if !tables.contains(table) {
        return Ok(0);
    }
    let value: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
        .fetch_one(pool)
        .await
        .with_context(|| format!("counting classic table {table}"))?;
    u64::try_from(value).context("negative SQLite row count")
}

fn sqlite_bool(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<bool> {
    Ok(row.try_get::<Option<i64>, _>(column)?.unwrap_or(0) != 0)
}

fn collisions(field: &str, groups: BTreeMap<String, Vec<i64>>) -> Vec<IdentityCollision> {
    groups
        .into_iter()
        .filter(|(value, ids)| !value.is_empty() && ids.len() > 1)
        .map(|(value, mut source_user_ids)| {
            source_user_ids.sort_unstable();
            IdentityCollision {
                field: field.to_owned(),
                value_sha256: sha256_text(&format!("{field}\0{value}")),
                source_user_ids,
            }
        })
        .collect()
}

fn sha256_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

fn inspect_uploads(root: &Path, issues: &mut Vec<PreflightIssue>) -> Result<UploadReport> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading uploads directory {}", dir.display()))?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();

    let mut report = UploadReport::default();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .expect("walked upload path remains below root")
            .to_path_buf();
        if path.extension().and_then(|ext| ext.to_str()) != Some("gz") {
            let artifact = hash_artifact("upload_asset", &path)?;
            report.assets.push(UploadAsset {
                relative_path: relative,
                bytes: artifact.bytes,
                sha256: artifact.sha256,
            });
            continue;
        }
        report.blob_files += 1;
        let metadata = std::fs::metadata(&path)?;
        report.total_compressed_bytes += metadata.len();
        let expected_sha1 = expected_blob_hash(&relative);
        let mut entry = BlobEntry {
            relative_path: relative,
            expected_sha1,
            content_sha1: None,
            compressed_sha256: None,
            compressed_bytes: metadata.len(),
            uncompressed_bytes: None,
            valid_gzip: false,
            address_matches_content: false,
            error: None,
        };
        match inspect_gzip_blob(&path) {
            Ok((content_sha1, compressed_sha256, uncompressed_bytes)) => {
                entry.valid_gzip = true;
                entry.address_matches_content =
                    entry.expected_sha1.as_deref() == Some(&content_sha1);
                entry.content_sha1 = Some(content_sha1);
                entry.compressed_sha256 = Some(compressed_sha256);
                entry.uncompressed_bytes = Some(uncompressed_bytes);
                report.valid_gzip_files += 1;
                report.total_uncompressed_bytes += uncompressed_bytes;
                if entry.address_matches_content {
                    report.address_matches_content += 1;
                }
            }
            Err(error) => {
                entry.error = Some(error.to_string());
            }
        }
        if !entry.valid_gzip || !entry.address_matches_content {
            report.invalid_files += 1;
        }
        report.blobs.push(entry);
    }

    report.tree_sha256 = upload_tree_fingerprint(&report);
    if report.invalid_files > 0 {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "invalid_upload_blobs",
            "one or more upload blobs is not valid gzip content at its SHA-1 address",
            Some(report.invalid_files),
        );
    }
    Ok(report)
}

fn expected_blob_hash(relative: &Path) -> Option<String> {
    let parent = relative.parent()?.file_name()?.to_str()?;
    let file = relative.file_name()?.to_str()?;
    let tail = file.strip_suffix(".gz")?;
    let candidate = format!("{parent}{tail}");
    (candidate.len() == 40 && candidate.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| candidate.to_ascii_lowercase())
}

fn inspect_gzip_blob(path: &Path) -> Result<(String, String, u64)> {
    let file = File::open(path).with_context(|| format!("opening blob {}", path.display()))?;
    let hashing = HashingReader::new(BufReader::new(file));
    let mut decoder = GzDecoder::new(hashing);
    let mut content_hasher = Sha1::new();
    let mut uncompressed_bytes = 0_u64;
    let mut buf = vec![0_u8; 1024 * 1024];
    loop {
        let n = decoder
            .read(&mut buf)
            .with_context(|| format!("decompressing blob {}", path.display()))?;
        if n == 0 {
            break;
        }
        content_hasher.update(&buf[..n]);
        uncompressed_bytes += n as u64;
    }
    let hashing = decoder.into_inner();
    Ok((
        hex::encode(content_hasher.finalize()),
        hex::encode(hashing.hasher.finalize()),
        uncompressed_bytes,
    ))
}

fn upload_tree_fingerprint(report: &UploadReport) -> String {
    let mut hasher = Sha256::new();
    for blob in &report.blobs {
        hasher.update(blob.relative_path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(blob.compressed_bytes.to_be_bytes());
        hasher.update(
            blob.compressed_sha256
                .as_deref()
                .unwrap_or("invalid")
                .as_bytes(),
        );
        hasher.update(blob.content_sha1.as_deref().unwrap_or("invalid").as_bytes());
    }
    for asset in &report.assets {
        hasher.update(asset.relative_path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(asset.bytes.to_be_bytes());
        hasher.update(asset.sha256.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[derive(Default)]
pub(crate) struct GraphAccumulator {
    count: u64,
    sum: [u8; 32],
    xor: [u8; 32],
}

impl GraphAccumulator {
    pub(crate) fn add(&mut self, triple: &Triple) {
        let mut hasher = Sha256::new();
        hasher.update(triple.to_string().as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        self.count += 1;
        for (slot, value) in self.xor.iter_mut().zip(digest) {
            *slot ^= value;
        }
        let mut carry = 0_u16;
        for index in (0..32).rev() {
            let total = u16::from(self.sum[index]) + u16::from(digest[index]) + carry;
            self.sum[index] = total as u8;
            carry = total >> 8;
        }
    }

    pub(crate) fn finish(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.count.to_be_bytes());
        hasher.update(self.sum);
        hasher.update(self.xor);
        hex::encode(hasher.finalize())
    }
}

fn inspect_rdf(
    path: &Path,
    public_graph: &str,
    user_graph_prefix: &str,
    issues: &mut Vec<PreflightIssue>,
) -> Result<(RdfReport, RdfFacts)> {
    let format: RdfFormat = rdf_format_for(path)?;
    let file =
        File::open(path).with_context(|| format!("opening RDF export {}", path.display()))?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut accumulators: BTreeMap<String, GraphAccumulator> = BTreeMap::new();
    let mut default_graph_quads = 0_u64;
    let mut blank_named_graph_quads = 0_u64;
    let mut owned_subjects = BTreeSet::new();
    let mut collection_subjects = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut viewers = BTreeSet::new();
    let mut shared_objects = BTreeSet::new();
    let mut share_grants = 0_u64;
    let mut referenced_blobs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for quad in RdfParser::from_format(format).for_reader(reader) {
        let quad = quad.with_context(|| format!("parsing RDF export {}", path.display()))?;
        let graph = match &quad.graph_name {
            GraphName::NamedNode(node) => node.as_str().to_owned(),
            GraphName::BlankNode(node) => {
                blank_named_graph_quads += 1;
                format!("_:{}", node.as_str())
            }
            GraphName::DefaultGraph => {
                default_graph_quads += 1;
                continue;
            }
        };
        let triple = Triple::new(
            quad.subject.clone(),
            quad.predicate.clone(),
            quad.object.clone(),
        );
        accumulators.entry(graph.clone()).or_default().add(&triple);

        let subject = match &quad.subject {
            NamedOrBlankNode::NamedNode(node) => node.as_str().to_owned(),
            NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()),
        };
        let predicate = quad.predicate.as_str();
        if predicate == SBH_OWNED_BY {
            owned_subjects.insert((graph.clone(), subject));
            if let Term::NamedNode(owner) = &quad.object {
                owners.insert(owner.as_str().to_owned());
            }
        } else if predicate == RDF_TYPE {
            if matches!(&quad.object, Term::NamedNode(node) if node.as_str() == SBOL2_COLLECTION) {
                collection_subjects.insert((graph.clone(), subject));
            }
        } else if predicate == SBH_CAN_VIEW {
            share_grants += 1;
            if let NamedOrBlankNode::NamedNode(viewer) = &quad.subject {
                viewers.insert(viewer.as_str().to_owned());
            }
            if let Term::NamedNode(object) = &quad.object {
                shared_objects.insert(object.as_str().to_owned());
            }
        } else if predicate == SBOL2_HASH || predicate == SBH_ATTACHMENT_HASH {
            if let Term::Literal(literal) = &quad.object {
                let value = literal.value();
                if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    referenced_blobs
                        .entry(value.to_ascii_lowercase())
                        .or_default()
                        .insert(graph.clone());
                }
            }
        }
    }

    if default_graph_quads > 0 {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "rdf_default_graph_present",
            "the classic export contains unnamed-graph quads and requires an explicit mapping policy",
            Some(default_graph_quads),
        );
    }
    if blank_named_graph_quads > 0 {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Target,
            "blank_named_graphs_unsupported",
            "the export contains blank-node graph names unsupported by the target graph store",
            Some(blank_named_graph_quads),
        );
    }

    let mut report = RdfReport {
        default_graph_quads,
        blank_named_graph_quads,
        distinct_owners: owners.len() as u64,
        shared_objects: shared_objects.len() as u64,
        share_grants,
        share_viewers: viewers.len() as u64,
        referenced_blob_hashes: referenced_blobs.len() as u64,
        ..RdfReport::default()
    };
    let mut facts = RdfFacts {
        owners,
        viewers,
        ..RdfFacts::default()
    };

    for (iri, accumulator) in accumulators {
        let class = classify_graph(&iri, public_graph, user_graph_prefix);
        report.total_quads += accumulator.count;
        add_classified(&mut report.graph_counts, class, 1);
        add_classified(&mut report.quad_counts, class, accumulator.count);
        facts.graphs.insert(iri.clone());
        report.graphs.push(RdfGraphReport {
            iri,
            class,
            quads: accumulator.count,
            fingerprint: accumulator.finish(),
        });
    }
    for (graph, _) in owned_subjects {
        add_classified(
            &mut report.owned_top_levels,
            classify_graph(&graph, public_graph, user_graph_prefix),
            1,
        );
    }
    for (graph, _) in collection_subjects {
        add_classified(
            &mut report.collections,
            classify_graph(&graph, public_graph, user_graph_prefix),
            1,
        );
    }
    for (hash, graphs) in referenced_blobs {
        let mut classes = BTreeSet::new();
        for graph in graphs {
            classes.insert(classify_graph(&graph, public_graph, user_graph_prefix));
        }
        for class in &classes {
            add_classified(&mut report.referenced_blob_hashes_by_class, *class, 1);
        }
        facts.referenced_blobs.insert(hash, classes);
    }

    if report.graph_counts.other > 0 {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Policy,
            "unclassified_rdf_graphs",
            "graphs outside the configured public and user namespaces require an explicit import, transform, or exclusion decision",
            Some(report.graph_counts.other),
        );
    }
    Ok((report, facts))
}

fn classify_graph(iri: &str, public_graph: &str, user_graph_prefix: &str) -> GraphClass {
    if iri == public_graph {
        GraphClass::Public
    } else if iri.starts_with(user_graph_prefix) {
        GraphClass::User
    } else {
        GraphClass::Other
    }
}

fn add_classified(counts: &mut ClassifiedCounts, class: GraphClass, amount: u64) {
    match class {
        GraphClass::Public => counts.public += amount,
        GraphClass::User => counts.user += amount,
        GraphClass::Other => counts.other += amount,
    }
}

fn reconcile(
    accounts: &AccountFacts,
    rdf: &RdfFacts,
    uploads: &UploadReport,
    issues: &mut Vec<PreflightIssue>,
) -> ReconciliationReport {
    let account_graphs = accounts
        .graph_to_ids
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let present_blobs = uploads
        .blobs
        .iter()
        .filter(|blob| blob.valid_gzip && blob.address_matches_content)
        .filter_map(|blob| blob.content_sha1.clone())
        .collect::<BTreeSet<_>>();
    let referenced = rdf
        .referenced_blobs
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let public_referenced = rdf
        .referenced_blobs
        .iter()
        .filter(|(_, classes)| classes.contains(&GraphClass::Public))
        .map(|(hash, _)| hash.clone())
        .collect::<BTreeSet<_>>();

    let mut report = ReconciliationReport {
        account_graphs_present: account_graphs.intersection(&rdf.graphs).count() as u64,
        accounts_without_rdf_graph: account_graphs.difference(&rdf.graphs).count() as u64,
        user_graphs_without_account: rdf
            .graphs
            .difference(&account_graphs)
            .filter(|graph| graph.contains("/user/"))
            .cloned()
            .collect(),
        owner_principals_without_account: rdf.owners.difference(&account_graphs).cloned().collect(),
        share_viewers_without_account: rdf.viewers.difference(&account_graphs).cloned().collect(),
        referenced_blobs_present: referenced.intersection(&present_blobs).count() as u64,
        referenced_blobs_missing: referenced.difference(&present_blobs).cloned().collect(),
        referenced_public_blobs_present: public_referenced.intersection(&present_blobs).count()
            as u64,
        referenced_public_blobs_missing: public_referenced
            .difference(&present_blobs)
            .cloned()
            .collect(),
        unreferenced_blob_files: present_blobs.difference(&referenced).count() as u64,
    };
    report.user_graphs_without_account.sort();
    report.owner_principals_without_account.sort();
    report.share_viewers_without_account.sort();
    report.referenced_blobs_missing.sort();
    report.referenced_public_blobs_missing.sort();

    if !report.referenced_blobs_missing.is_empty() {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Source,
            "referenced_blobs_missing",
            "RDF references attachment hashes that are absent or invalid in the uploads tree",
            Some(report.referenced_blobs_missing.len() as u64),
        );
    }
    if !report.user_graphs_without_account.is_empty()
        || !report.owner_principals_without_account.is_empty()
        || !report.share_viewers_without_account.is_empty()
    {
        issue(
            issues,
            IssueSeverity::Blocker,
            IssueScope::Policy,
            "orphan_identity_principals",
            "one or more RDF graph, owner, or sharing principal has no matching account and requires an explicit recovery policy",
            Some(
                (report.user_graphs_without_account.len()
                    + report.owner_principals_without_account.len()
                    + report.share_viewers_without_account.len()) as u64,
            ),
        );
    }
    if report.unreferenced_blob_files > 0 {
        issue(
            issues,
            IssueSeverity::Warning,
            IssueScope::Policy,
            "unreferenced_blob_files",
            "upload blobs not referenced by the current RDF should be retained in a cold/orphan catalog",
            Some(report.unreferenced_blob_files),
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_layout_is_discovered() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("sbhData/data/uploads")).expect("dirs");
        std::fs::write(root.path().join("virtuoso.db"), b"db").expect("virtuoso");
        std::fs::write(root.path().join("sbhData/data/synbiohub.sqlite"), b"sqlite")
            .expect("sqlite");
        std::fs::write(
            root.path().join("sbhData/data/synibohub.sqlite"),
            b"historical",
        )
        .expect("historical sqlite");
        std::fs::write(root.path().join("config.local.json"), b"{}").expect("config");
        let resolved = resolve_source(&PreflightInputs {
            source: Some(root.path().to_path_buf()),
            virtuoso_db: None,
            rdf: None,
            sqlite: None,
            uploads: None,
            config: None,
            config_defaults: None,
            report: None,
            allow_blockers: true,
        })
        .expect("resolve");
        assert_eq!(resolved.virtuoso_db, Some(root.path().join("virtuoso.db")));
        assert_eq!(
            resolved.sqlite,
            Some(root.path().join("sbhData/data/synbiohub.sqlite"))
        );
        assert_eq!(
            resolved.uploads,
            Some(root.path().join("sbhData/data/uploads"))
        );
        assert_eq!(
            resolved.additional_sqlite,
            vec![root.path().join("sbhData/data/synibohub.sqlite")]
        );
    }

    #[test]
    fn config_transform_maps_database_prefix_into_theme_uri_prefix() {
        let source = serde_json::json!({
            "instanceName": "Production",
            "databasePrefix": "https://example.org/",
            "requireLogin": true,
            "passwordSalt": "must-not-leak",
            "remotes": [{"url": "https://remote.example/"}]
        });
        let target = transform_classic_config(&source);
        assert_eq!(
            target["theme"]["uriPrefix"],
            serde_json::json!("https://example.org/")
        );
        assert_eq!(target["theme"]["requireLogin"], serde_json::json!(true));
        assert!(!serde_json::to_string(&target)
            .expect("serialize")
            .contains("must-not-leak"));
    }

    #[test]
    fn config_preview_redacts_nested_credentials_without_changing_shape() {
        let source = serde_json::json!({
            "mail": {"sendgridApiKey": "must-not-leak", "fromAddress": "ops@example.test"},
            "remotes": {
                "ice": {
                    "X-ICE-API-Token": "must-not-leak-either",
                    "key": "generic-key-must-not-leak",
                    "url": "https://ice.example.test"
                }
            },
            "plugins": [{"credentials": {"clientSecret": "also-secret"}}]
        });
        let transformed = transform_classic_config(&source);
        let preview = transformed
            .iter()
            .map(|(key, value)| (key.clone(), redact_config_for_report(value)))
            .collect::<BTreeMap<_, _>>();
        let serialized = serde_json::to_string(&preview).expect("serialize preview");

        assert!(!serialized.contains("must-not-leak"));
        assert!(!serialized.contains("must-not-leak-either"));
        assert!(!serialized.contains("generic-key-must-not-leak"));
        assert!(!serialized.contains("also-secret"));
        assert_eq!(preview["mail"]["fromAddress"], "ops@example.test");
        assert_eq!(preview["remotes"]["ice"]["url"], "https://ice.example.test");
        assert_eq!(preview["mail"]["sendgridApiKey"], REDACTED_CONFIG_VALUE);
    }

    #[test]
    fn secret_fail_safe_matches_values_not_public_substrings() {
        let report = serde_json::json!({
            "public_url": "https://synbiohub.org/public",
            "nested": {"credential": "actual-secret"},
        });
        let mut occurrences = BTreeSet::new();
        find_secret_occurrences(&report, "synbiohub.org", "$", &mut occurrences);
        assert!(occurrences.is_empty());

        find_secret_occurrences(&report, "actual-secret", "$", &mut occurrences);
        assert_eq!(
            occurrences,
            BTreeSet::from(["$/nested/credential".to_owned()])
        );
    }

    #[test]
    fn upload_fixture_is_fully_content_addressed() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synbiohub-mini/uploads");
        let mut issues = Vec::new();
        let report = inspect_uploads(&root, &mut issues).expect("inspect uploads");
        assert_eq!(report.blob_files, 1);
        assert_eq!(report.valid_gzip_files, 1);
        assert_eq!(report.address_matches_content, 1);
        assert_eq!(report.invalid_files, 0);
        assert!(issues.is_empty());
    }

    #[test]
    fn rdf_fixture_has_stable_graph_counts() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synbiohub-mini/dump.nq");
        let mut issues = Vec::new();
        let (report, _) = inspect_rdf(
            &path,
            "http://synbiohub.org/public",
            "http://synbiohub.org/user/",
            &mut issues,
        )
        .expect("inspect RDF");
        assert_eq!(report.total_quads, 10);
        assert_eq!(report.graph_counts.public, 1);
        assert_eq!(report.graph_counts.user, 1);
        assert_eq!(report.graph_counts.other, 0);
        assert_eq!(report.quad_counts.public, 7);
        assert_eq!(report.quad_counts.user, 3);
    }

    #[test]
    fn graph_fingerprint_is_order_independent_and_duplicate_aware() {
        use oxrdf::{Literal, NamedNode};

        let a = Triple::new(
            NamedNode::new_unchecked("https://example.org/a"),
            NamedNode::new_unchecked("https://example.org/p"),
            Literal::new_simple_literal("one"),
        );
        let b = Triple::new(
            NamedNode::new_unchecked("https://example.org/b"),
            NamedNode::new_unchecked("https://example.org/p"),
            Literal::new_simple_literal("two"),
        );
        let mut first = GraphAccumulator::default();
        first.add(&a);
        first.add(&b);
        let mut second = GraphAccumulator::default();
        second.add(&b);
        second.add(&a);
        assert_eq!(first.finish(), second.finish());
        second.add(&a);
        assert_ne!(first.finish(), second.finish());
    }

    #[tokio::test]
    async fn complete_synthetic_snapshot_reconciles_without_blockers() {
        let root = tempfile::tempdir().expect("tempdir");
        let sqlite = root.path().join("synbiohub.sqlite");
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&sqlite)
                .create_if_missing(true),
        )
        .await
        .expect("open synthetic SQLite");
        sqlx::query(
            "CREATE TABLE user ( \
             id INTEGER PRIMARY KEY, name TEXT, username TEXT, email TEXT, affiliation TEXT, \
             password TEXT, graphUri TEXT, \
             isAdmin INTEGER, isCurator INTEGER, isMember INTEGER, resetPasswordLink TEXT, \
             createdAt TEXT, updatedAt TEXT )",
        )
        .execute(&pool)
        .await
        .expect("create users");
        for (id, username) in [(1_i64, "alice"), (2_i64, "bob")] {
            sqlx::query("INSERT INTO user VALUES (?, NULL, ?, ?, NULL, ?, ?, 0, 0, 1, NULL, ?, ?)")
                .bind(id)
                .bind(username)
                .bind(format!("{username}@example.org"))
                .bind("0123456789012345678901234567890123456789")
                .bind(format!("http://synbiohub.org/user/{username}"))
                .bind("2025-01-01 00:00:00 +00:00")
                .bind("2025-01-01 00:00:00 +00:00")
                .execute(&pool)
                .await
                .expect("insert user");
        }
        pool.close().await;

        let fixtures =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synbiohub-mini");
        let uploads = root.path().join("uploads");
        std::fs::create_dir_all(uploads.join("04")).expect("upload shard");
        std::fs::copy(
            fixtures.join("uploads/04/b9beedcedc38daf4ff574b3d4bb291f2bbcaf0.gz"),
            uploads.join("04/b9beedcedc38daf4ff574b3d4bb291f2bbcaf0.gz"),
        )
        .expect("copy blob");

        let report = inspect(PreflightInputs {
            source: None,
            virtuoso_db: None,
            rdf: Some(fixtures.join("dump.nq")),
            sqlite: Some(sqlite),
            uploads: Some(uploads),
            config: Some(fixtures.join("config.local.json")),
            config_defaults: None,
            report: None,
            allow_blockers: false,
        })
        .await
        .expect("preflight");

        assert!(report.ready_for_import);
        assert_eq!(report.sqlite.as_ref().expect("sqlite").users, 2);
        assert_eq!(report.rdf.as_ref().expect("rdf").total_quads, 10);
        let reconciliation = report.reconciliation.expect("reconciliation");
        assert_eq!(reconciliation.account_graphs_present, 1);
        assert_eq!(reconciliation.accounts_without_rdf_graph, 1);
        assert!(reconciliation.user_graphs_without_account.is_empty());
        assert_eq!(reconciliation.unreferenced_blob_files, 1);
        assert!(report
            .issues
            .iter()
            .all(|issue| issue.severity != IssueSeverity::Blocker));
    }
}
