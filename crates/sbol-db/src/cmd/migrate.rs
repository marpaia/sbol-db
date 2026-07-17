//! `sbol-db migrate-synbiohub` — load a classic SynBioHub instance into sbol-db.
//!
//! The migration reads an unpacked classic instance (or its individual parts)
//! and reconstructs the equivalent sbol-db state:
//!
//! - the Virtuoso RDF dump is loaded verbatim through the graph-store write
//!   path, one named graph at a time, so the public graph and every per-user
//!   graph land byte-for-byte where they were;
//! - `synbiohub.sqlite`'s `users` rows become [`UserStore`] accounts with their
//!   legacy `sha1(salt + sha1(password))` hash kept intact, so the first login
//!   transparently rehashes to argon2;
//! - the `uploads/` blob tree is copied into the [`FsBlobStore`] root under the
//!   same `<sha1[0:2]>/<sha1[2:]>.gz` layout, so every attachment stays
//!   retrievable by its content hash;
//! - each top-level `config.local.json` key is written into the [`ConfigStore`],
//!   the durable replacement for the mutable config file;
//! - a `rebuild_search_index` job is enqueued so the native ranked index,
//!   PageRank scores, and sequence clusters are rebuilt from the loaded corpus.
//!
//! [`FsBlobStore`]: sbol_db_app::FsBlobStore

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use oxrdf::{GraphName, Triple};
use oxrdfio::{RdfFormat, RdfParser, RdfSerializer};
use sbol_db_app::AuthService;
use sbol_db_core::{NewUser, SerializationFormat};
use sbol_db_storage::{
    ConfigStore, GraphWriteMode, JobQueue, Migrator, NewJob, SbolStore, UserStore,
};
use serde::Serialize;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::output::print_json;

/// The default filenames a migration looks for under `--source` when a part is
/// not given its own explicit path.
const DEFAULT_RDF_DUMP: &str = "dump.nq";
const DEFAULT_SQLITE: &str = "synbiohub.sqlite";
const DEFAULT_UPLOADS: &str = "uploads";
const DEFAULT_CONFIG: &str = "config.local.json";

/// The job kind the search-index rebuild handler is registered under.
const REINDEX_KIND: &str = "rebuild_search_index";

/// The paths and toggles that drive a migration run. Each part is optional: an
/// explicit path overrides the one derived from `source`, and a part with no
/// resolvable path (neither explicit nor under `source`) is skipped.
pub struct MigrateInputs {
    /// Root of an unpacked classic instance; supplies defaults for any part
    /// left unset.
    pub source: Option<PathBuf>,
    /// The Virtuoso RDF dump (N-Quads or TriG).
    pub rdf: Option<PathBuf>,
    /// The classic `synbiohub.sqlite` account database.
    pub sqlite: Option<PathBuf>,
    /// The classic `uploads/` blob tree.
    pub uploads: Option<PathBuf>,
    /// The classic `config.local.json`.
    pub config: Option<PathBuf>,
    /// The [`FsBlobStore`](sbol_db_app::FsBlobStore) root the uploads tree is
    /// copied under (as `<root>/uploads`).
    pub blob_store: PathBuf,
    /// Named graph to load default-graph (unnamed) triples into. When unset,
    /// such triples are counted and skipped, since a SynBioHub dump keeps
    /// everything in named graphs.
    pub default_graph: Option<String>,
    /// Skip applying pending schema migrations before loading.
    pub skip_migrations: bool,
    /// Skip enqueuing the search-index rebuild.
    pub no_reindex: bool,
}

/// One migrated named graph and the triple count written into it.
#[derive(Debug, Serialize)]
pub struct GraphReport {
    pub graph: String,
    pub triples: usize,
}

/// A structured summary of what a migration run loaded.
#[derive(Debug, Default, Serialize)]
pub struct MigrateReport {
    pub graphs: Vec<GraphReport>,
    pub triples_total: usize,
    pub default_graph_triples_skipped: usize,
    pub users_imported: usize,
    pub blobs_copied: usize,
    pub config_keys: Vec<String>,
    pub reindex_job: Option<String>,
}

/// CLI entry point: run the migration and print the report as JSON.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    store: Arc<dyn SbolStore>,
    users: Arc<dyn UserStore>,
    config: Arc<dyn ConfigStore>,
    jobs: Arc<dyn JobQueue>,
    migrator: Option<Arc<dyn Migrator>>,
    inputs: MigrateInputs,
) -> Result<()> {
    let report = migrate(store, users, config, jobs, migrator, inputs).await?;
    print_json(&report)
}

/// Run the migration and return the report. Split from [`run`] so tests can
/// assert against the loaded state directly.
pub(crate) async fn migrate(
    store: Arc<dyn SbolStore>,
    users: Arc<dyn UserStore>,
    config: Arc<dyn ConfigStore>,
    jobs: Arc<dyn JobQueue>,
    migrator: Option<Arc<dyn Migrator>>,
    inputs: MigrateInputs,
) -> Result<MigrateReport> {
    let mut report = MigrateReport::default();

    if !inputs.skip_migrations {
        if let Some(migrator) = &migrator {
            migrator
                .run_migrations()
                .await
                .context("applying schema migrations before load")?;
            tracing::info!("schema migrations applied");
        }
    }

    if let Some(path) = resolve(&inputs.source, &inputs.rdf, DEFAULT_RDF_DUMP, "RDF dump")? {
        load_graphs(&store, &path, inputs.default_graph.as_deref(), &mut report).await?;
    }

    if let Some(path) = resolve(&inputs.source, &inputs.sqlite, DEFAULT_SQLITE, "sqlite")? {
        load_users(&users, &path, &mut report).await?;
    }

    if let Some(path) = resolve(&inputs.source, &inputs.uploads, DEFAULT_UPLOADS, "uploads")? {
        copy_uploads(&path, &inputs.blob_store, &mut report)?;
    }

    if let Some(path) = resolve(&inputs.source, &inputs.config, DEFAULT_CONFIG, "config")? {
        load_config(&config, &path, &mut report).await?;
    }

    if !inputs.no_reindex {
        let outcome = jobs
            .enqueue(NewJob::new(REINDEX_KIND, serde_json::json!({})))
            .await
            .context("enqueuing the search-index rebuild")?;
        let id = outcome.into_job().id.to_string();
        tracing::info!(job_id = %id, "enqueued {REINDEX_KIND}");
        report.reindex_job = Some(id);
    }

    Ok(report)
}

/// Resolve a part's path: an explicit override wins; otherwise it is `source`
/// joined with `default_name`. Returns `None` (with a warning) when the
/// resolved path does not exist, so a partial instance still migrates the parts
/// it has. Errors only when neither an override nor a source is given.
fn resolve(
    source: &Option<PathBuf>,
    explicit: &Option<PathBuf>,
    default_name: &str,
    label: &str,
) -> Result<Option<PathBuf>> {
    let candidate = match (explicit, source) {
        (Some(path), _) => path.clone(),
        (None, Some(root)) => root.join(default_name),
        (None, None) => {
            return Err(anyhow!(
                "no path for the {label}: pass --source or the part's explicit flag"
            ))
        }
    };
    if candidate.exists() {
        Ok(Some(candidate))
    } else {
        tracing::warn!(path = %candidate.display(), "skipping absent {label}");
        Ok(None)
    }
}

/// Load an RDF dump verbatim, one named graph at a time. The dump is parsed as
/// a quad stream, its triples are grouped by graph name, and each group is
/// written to its own named graph with the graph-store write path so per-graph
/// boundaries are preserved exactly.
async fn load_graphs(
    store: &Arc<dyn SbolStore>,
    path: &Path,
    default_graph: Option<&str>,
    report: &mut MigrateReport,
) -> Result<()> {
    let format = rdf_format_for(path)?;
    let bytes =
        std::fs::read(path).with_context(|| format!("reading RDF dump {}", path.display()))?;

    let mut by_graph: BTreeMap<String, Vec<Triple>> = BTreeMap::new();
    for quad in RdfParser::from_format(format).for_reader(bytes.as_slice()) {
        let quad = quad.with_context(|| format!("parsing RDF dump {}", path.display()))?;
        let graph_iri = match quad.graph_name {
            GraphName::NamedNode(node) => node.into_string(),
            GraphName::BlankNode(_) => {
                return Err(anyhow!(
                    "blank-node graph names are not supported by the graph store"
                ))
            }
            GraphName::DefaultGraph => match default_graph {
                Some(graph) => graph.to_owned(),
                None => {
                    report.default_graph_triples_skipped += 1;
                    continue;
                }
            },
        };
        by_graph.entry(graph_iri).or_default().push(Triple {
            subject: quad.subject,
            predicate: quad.predicate,
            object: quad.object,
        });
    }

    for (graph_iri, triples) in by_graph {
        let ntriples = serialize_ntriples(&triples)?;
        let inserted = store
            .graph_store_write(
                &graph_iri,
                &ntriples,
                SerializationFormat::NTriples,
                GraphWriteMode::Replace,
            )
            .await
            .with_context(|| format!("loading graph {graph_iri}"))?;
        tracing::info!(graph = %graph_iri, triples = inserted, "loaded graph");
        report.triples_total += inserted;
        report.graphs.push(GraphReport {
            graph: graph_iri,
            triples: inserted,
        });
    }
    Ok(())
}

/// Serialize a set of triples to N-Triples, the graph-store write path's
/// canonical single-graph input.
fn serialize_ntriples(triples: &[Triple]) -> Result<String> {
    let mut buf = Vec::new();
    let mut serializer = RdfSerializer::from_format(RdfFormat::NTriples).for_writer(&mut buf);
    for triple in triples {
        serializer
            .serialize_triple(triple)
            .context("serializing triple to N-Triples")?;
    }
    serializer.finish().context("finishing N-Triples output")?;
    String::from_utf8(buf).context("N-Triples output is not valid UTF-8")
}

/// Map an RDF dump's extension to its parser format. Virtuoso dumps are
/// quad-bearing (N-Quads or TriG); triple-only formats are accepted too and
/// land in the default graph.
fn rdf_format_for(path: &Path) -> Result<RdfFormat> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    Ok(match ext.as_str() {
        "nq" | "nquads" => RdfFormat::NQuads,
        "trig" => RdfFormat::TriG,
        "ttl" | "turtle" => RdfFormat::Turtle,
        "nt" | "ntriples" => RdfFormat::NTriples,
        "rdf" | "xml" => RdfFormat::RdfXml,
        other => {
            return Err(anyhow!(
                "unrecognized RDF dump extension `{other}` \
                 (expected nq, trig, ttl, nt, or rdf)"
            ))
        }
    })
}

/// Import every `users` row from a classic `synbiohub.sqlite`, preserving each
/// account's legacy password hash and owned graph URI. The database is opened
/// read-only so the source instance is never mutated.
async fn load_users(
    users: &Arc<dyn UserStore>,
    path: &Path,
    report: &mut MigrateReport,
) -> Result<()> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("opening classic sqlite {}", path.display()))?;

    let result = read_and_insert_users(users, &pool, report).await;
    pool.close().await;
    result
}

async fn read_and_insert_users(
    users: &Arc<dyn UserStore>,
    pool: &SqlitePool,
    report: &mut MigrateReport,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT name, username, email, affiliation, password, graphUri, \
         isAdmin, isCurator, isMember, resetPasswordLink FROM users",
    )
    .fetch_all(pool)
    .await
    .context("reading users from classic sqlite")?;

    for row in rows {
        let username: String = row
            .try_get("username")
            .context("user row missing username")?;
        let graph_uri: Option<String> = row.try_get("graphUri")?;
        let reset_link: Option<String> = row.try_get("resetPasswordLink")?;
        let new_user = NewUser {
            name: row
                .try_get::<Option<String>, _>("name")?
                .unwrap_or_default(),
            email: row
                .try_get::<Option<String>, _>("email")?
                .unwrap_or_default(),
            affiliation: row.try_get("affiliation")?,
            password_hash: row
                .try_get::<Option<String>, _>("password")?
                .unwrap_or_default(),
            graph_uri: graph_uri.unwrap_or_else(|| AuthService::graph_uri(&username)),
            is_admin: get_bool(&row, "isAdmin")?,
            is_curator: get_bool(&row, "isCurator")?,
            is_member: get_bool(&row, "isMember")?,
            username: username.clone(),
        };
        let created = users
            .create_user(new_user)
            .await
            .with_context(|| format!("creating migrated user {username}"))?;
        if let Some(link) = reset_link.filter(|link| !link.is_empty()) {
            users.set_reset_link(created.id, Some(&link)).await?;
        }
        tracing::info!(username = %username, "migrated user");
        report.users_imported += 1;
    }
    Ok(())
}

/// Read a SQLite boolean column, which classic SynBioHub stores as an integer
/// `0`/`1` (a NULL reads as `false`).
fn get_bool(row: &SqliteRow, column: &str) -> Result<bool> {
    let value: Option<i64> = row.try_get(column)?;
    Ok(value.unwrap_or(0) != 0)
}

/// Copy the classic `uploads/` tree into the blob-store root, preserving the
/// `<sha1[0:2]>/<sha1[2:]>.gz` shard layout so every blob stays retrievable by
/// its content hash. Copies are byte-for-byte; the tree is already gzip-encoded
/// and content-addressed.
fn copy_uploads(src_uploads: &Path, blob_root: &Path, report: &mut MigrateReport) -> Result<()> {
    let dest_uploads = blob_root.join("uploads");
    let mut stack = vec![src_uploads.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("reading uploads directory {}", dir.display()))?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(src_uploads)
                .expect("walked path is under the uploads root");
            let dest = dest_uploads.join(relative);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &dest)
                .with_context(|| format!("copying blob {}", path.display()))?;
            report.blobs_copied += 1;
        }
    }
    tracing::info!(blobs = report.blobs_copied, "copied uploads tree");
    Ok(())
}

/// Write each top-level `config.local.json` key into the durable config store.
async fn load_config(
    config: &Arc<dyn ConfigStore>,
    path: &Path,
    report: &mut MigrateReport,
) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing config {}", path.display()))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("config {} is not a JSON object", path.display()))?;
    for (key, value) in object {
        config.set(key, value).await?;
        report.config_keys.push(key.clone());
    }
    report.config_keys.sort();
    tracing::info!(keys = report.config_keys.len(), "loaded config keys");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use sbol_db_app::FsBlobStore;
    use sbol_db_backend::Backend;
    use sbol_db_core::{ObjectTerm, SubjectTerm};
    use sbol_db_storage::BlobStore;

    const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";
    const ALICE_GRAPH: &str = "http://synbiohub.org/user/alice";
    const SALT: &str = "synthetic_salt";
    /// SHA-1 of the fixture uploads blob's uncompressed content.
    const BLOB_SHA1: &str = "04b9beedcedc38daf4ff574b3d4bb291f2bbcaf0";

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synbiohub-mini")
    }

    /// Write a classic-shaped `synbiohub.sqlite` with two accounts, both
    /// carrying the legacy `sha1(salt + sha1(password))` password hash, into
    /// `path`.
    async fn write_classic_sqlite(path: &Path) {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options)
            .await
            .expect("open sqlite");
        sqlx::query(
            "CREATE TABLE users ( \
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT, username TEXT, email TEXT, affiliation TEXT, \
             password TEXT, graphUri TEXT, isAdmin INTEGER, \
             resetPasswordLink TEXT, isCurator INTEGER, isMember INTEGER )",
        )
        .execute(&pool)
        .await
        .expect("create users table");

        // alice: administrator, legacy hash of "hunter2".
        sqlx::query(
            "INSERT INTO users \
             (name, username, email, affiliation, password, graphUri, isAdmin, isCurator, isMember) \
             VALUES (?, ?, ?, ?, ?, ?, 1, 0, 1)",
        )
        .bind("Alice Example")
        .bind("alice")
        .bind("alice@example.org")
        .bind("Synthetic Lab")
        .bind("13b2c1600e24b689e67d72da2e660bdf3c409a1c")
        .bind(ALICE_GRAPH)
        .execute(&pool)
        .await
        .expect("insert alice");

        // bob: plain member, legacy hash of "correcthorse".
        sqlx::query(
            "INSERT INTO users \
             (name, username, email, affiliation, password, graphUri, isAdmin, isCurator, isMember) \
             VALUES (?, ?, ?, ?, ?, ?, 0, 0, 1)",
        )
        .bind("Bob Example")
        .bind("bob")
        .bind("bob@example.org")
        .bind(Option::<String>::None)
        .bind("63a6d9fdecc15cc6696680a5ae822747ff9b1883")
        .bind("http://synbiohub.org/user/bob")
        .execute(&pool)
        .await
        .expect("insert bob");

        pool.close().await;
    }

    #[tokio::test]
    async fn migrates_a_synthetic_mini_dump() {
        let workdir = tempfile::tempdir().expect("tempdir");
        let fixtures = fixtures_dir();

        // A classic sqlite is synthesized next to the shipped static fixtures.
        let sqlite_path = workdir.path().join("synbiohub.sqlite");
        write_classic_sqlite(&sqlite_path).await;

        let db_path = workdir.path().join("sbol-db.sqlite");
        let backend = Backend::open(&format!("sqlite://{}", db_path.display()))
            .await
            .expect("open sbol-db sqlite backend");

        let blob_root = workdir.path().join("blobs");
        let inputs = MigrateInputs {
            source: None,
            rdf: Some(fixtures.join("dump.nq")),
            sqlite: Some(sqlite_path),
            uploads: Some(fixtures.join("uploads")),
            config: Some(fixtures.join("config.local.json")),
            blob_store: blob_root.clone(),
            default_graph: None,
            skip_migrations: false,
            no_reindex: false,
        };

        let report = migrate(
            backend.store.clone(),
            backend.users.clone(),
            backend.config.clone(),
            backend.jobs.clone(),
            backend.migrator.clone(),
            inputs,
        )
        .await
        .expect("migration succeeds");

        // (a) The named graphs are queryable verbatim.
        let public = backend
            .store
            .graph_store_read(PUBLIC_GRAPH)
            .await
            .expect("read public graph");
        assert_eq!(public.len(), 7, "public graph triple count is verbatim");
        assert!(
            public
                .iter()
                .any(|t| matches!(&t.subject, SubjectTerm::Iri(s)
                if s.as_str() == "http://synbiohub.org/public/mini/BBa_J23100/1")
                    && t.predicate.as_str() == "http://purl.org/dc/terms/title"
                    && matches!(&t.object, ObjectTerm::Literal { value, .. }
                    if value == "Anderson promoter J23100")),
            "a known public triple round-trips"
        );

        let alice = backend
            .store
            .graph_store_read(ALICE_GRAPH)
            .await
            .expect("read alice graph");
        assert_eq!(alice.len(), 3, "the per-user graph is preserved separately");
        assert_eq!(report.triples_total, 10);
        assert_eq!(report.default_graph_triples_skipped, 0);

        // (b) A migrated legacy user authenticates, and the hash upgrades.
        assert_eq!(report.users_imported, 2);
        let auth = AuthService::new(backend.users.clone(), backend.tokens.clone());
        let logged_in = auth
            .authenticate("alice", "hunter2", SALT)
            .await
            .expect("legacy login succeeds");
        assert!(logged_in.is_admin, "admin flag migrated");
        assert_eq!(logged_in.graph_uri, ALICE_GRAPH, "owned graph migrated");

        let stored = backend
            .users
            .find_by_email_or_username("alice")
            .await
            .expect("lookup")
            .expect("alice present");
        assert!(
            stored.password_hash.starts_with("$argon2"),
            "the legacy hash upgraded to argon2 on login"
        );

        // (c) The migrated blob is retrievable by its content hash.
        let blobs = FsBlobStore::new(&blob_root);
        assert_eq!(report.blobs_copied, 1);
        let payload = blobs
            .get(BLOB_SHA1)
            .await
            .expect("blob read")
            .expect("blob present by hash");
        assert_eq!(payload, b"synthetic attachment payload\n");

        // (d) A config key is set.
        assert!(report.config_keys.contains(&"instanceName".to_owned()));
        let name = backend
            .config
            .get("instanceName")
            .await
            .expect("config read")
            .expect("instanceName set");
        assert_eq!(name, serde_json::json!("Synthetic SynBioHub"));

        // (e) The search-index rebuild was enqueued.
        assert!(report.reindex_job.is_some(), "reindex job enqueued");
    }
}
