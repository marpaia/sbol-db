use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sbol_db_core::{
    Direction, DocumentId, IriString, NeighborhoodQuery, ObjectId, SerializationFormat,
};
use sbol_db_postgres::{
    connect_with_retry, run_migrations, ImportInput, ListObjectsFilter, SbolObjectService,
    SequenceSearchOptions,
};
use sbol_db_rdf::hash_bytes;
use sbol_db_server::{router, AppState, Metrics};
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions};

#[derive(Parser, Debug)]
#[command(version, about = "sbol-db CLI", long_about = None)]
struct Cli {
    /// Postgres connection string. Defaults to the docker-compose dev DB.
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://sbol:sbol@localhost:5432/sbol"
    )]
    database_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Apply pending migrations.
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
    /// Import one or more SBOL documents. `path` may be a single file or a
    /// directory; directories are walked recursively for files whose
    /// extension is a recognised RDF serialization (`.ttl`, `.nt`, `.jsonld`,
    /// `.rdf`, `.xml`, `.trig`, `.nq`).
    ///
    /// **Directory imports default to one atomic Postgres transaction** —
    /// either every file commits or none do. Use `--continue-on-error` for
    /// corpus-scale onboarding where per-file resilience matters more than
    /// batch atomicity; that mode also enables `--parallel`.
    Import {
        path: PathBuf,
        /// Override the format inferred from the file extension. Only meaningful
        /// when `path` is a single file.
        #[arg(long)]
        format: Option<String>,
        /// Optional document IRI to record alongside the import. Only allowed
        /// for single-file imports.
        #[arg(long)]
        document_iri: Option<String>,
        /// Optional name. Only allowed for single-file imports.
        #[arg(long)]
        name: Option<String>,
        /// Run each file in its own transaction in parallel, continuing past
        /// per-file failures. Disables the default atomic-batch behavior; use
        /// for corpus onboarding where one bad file shouldn't roll back the
        /// rest.
        #[arg(long)]
        continue_on_error: bool,
        /// Number of files to import in parallel. Only valid with
        /// `--continue-on-error`; ignored otherwise (transactional mode is
        /// single-threaded by definition).
        #[arg(long, default_value_t = 1)]
        parallel: usize,
        /// Skip files whose SHA3-256 content hash is already present in
        /// `sbol_documents`. Cheap re-import idempotency.
        #[arg(long)]
        skip_existing: bool,
    },
    /// Stream every stored object out as newline-delimited JSON (`SbolObjectRecord`
    /// per line). Pages through `sbol_objects` with a keyset cursor; safe for
    /// corpus-scale dumps.
    ExportAll {
        /// Restrict to objects whose `sbol_class` equals this IRI.
        #[arg(long)]
        sbol_class: Option<String>,
        /// Restrict to objects carrying this role IRI in their `roles` array.
        #[arg(long)]
        role: Option<String>,
        /// Restrict to objects belonging to a specific document.
        #[arg(long)]
        document_id: Option<uuid::Uuid>,
        /// Page size used internally; max 5000.
        #[arg(long, default_value_t = 1000)]
        page_size: u32,
    },
    /// Fetch a stored object by its IRI.
    Get {
        iri: String,
        #[arg(long)]
        json: bool,
    },
    /// Export an object subgraph in the requested format.
    Export {
        iri: String,
        #[arg(long, default_value = "turtle")]
        format: String,
    },
    /// Re-print the validation findings for a document (Phase 1 placeholder).
    Validate { document_id: uuid::Uuid },
    /// Walk the graph neighborhood around an IRI.
    Neighborhood {
        iri: String,
        /// Maximum traversal depth from the root.
        #[arg(long, default_value_t = 2)]
        depth: u32,
        /// Edge direction: `forward`, `backward`, or `both`.
        #[arg(long, default_value = "forward")]
        direction: String,
        /// Restrict traversal to these predicate IRIs (repeatable).
        #[arg(long = "predicate")]
        predicates: Vec<String>,
        /// Hard cap on visited nodes.
        #[arg(long, default_value_t = 2048)]
        max_nodes: u32,
        /// Include literal-position edges (off by default; they're skipped
        /// during traversal and added in a second pass for the visited set).
        #[arg(long)]
        literals: bool,
        /// If set, emit the reached subgraph as RDF in this format instead
        /// of JSON. Accepts the same names as `export --format`.
        #[arg(long)]
        rdf: Option<String>,
    },
    /// Run a SPARQL 1.1 read-only query (SELECT/ASK/CONSTRUCT/DESCRIBE).
    Sparql {
        /// Path to a `.rq` query file, or `-` to read from stdin.
        source: String,
        /// Output format. Defaults to JSON for SELECT/ASK and Turtle for
        /// CONSTRUCT/DESCRIBE.
        #[arg(long)]
        format: Option<String>,
        /// Wall-clock timeout in seconds (best-effort soft cap).
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
        /// Maximum solution/triple rows in the serialized output.
        #[arg(long, default_value_t = 100_000)]
        max_rows: usize,
        /// Reject query strings exceeding this many bytes.
        #[arg(long, default_value_t = 64 * 1024)]
        max_query_size: usize,
    },
    /// Sequence substring search with reverse-complement awareness.
    Sequences {
        #[command(subcommand)]
        action: SequencesAction,
    },
    /// Manage loaded ontologies (SO, SBO, ...) used for role/type expansion.
    Ontology {
        #[command(subcommand)]
        action: OntologyAction,
    },
    /// Start the HTTP server.
    Serve {
        #[arg(long, env = "SBOL_DB_BIND", default_value = "127.0.0.1:8080")]
        bind: SocketAddr,
    },
}

#[derive(Subcommand, Debug)]
enum SequencesAction {
    /// Search every indexed nucleotide Sequence for `pattern` (and its
    /// reverse complement unless `--forward-only`).
    Search {
        pattern: String,
        #[arg(long, default_value_t = 1024)]
        max_hits: u32,
        #[arg(long)]
        forward_only: bool,
    },
}

#[derive(Subcommand, Debug)]
enum OntologyAction {
    /// Fetch an OBO ontology from a canonical URL and load it. Recognised
    /// shorthand prefixes (`so`, `sbo`) use sensible defaults; for any other
    /// prefix supply `--url` and `--name`.
    Fetch {
        prefix: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// List ontologies currently loaded into the database.
    List,
    /// Show every descendant of a term (resolved canonically by IRI or
    /// CURIE such as `SO:0000167`).
    Descendants { iri_or_curie: String },
}

#[derive(Subcommand, Debug)]
enum MigrateAction {
    /// Run all pending migrations.
    Up,
    /// Show migration status.
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();
    let pool = open_pool(&cli.database_url, &cli.command)
        .await
        .with_context(|| format!("connecting to {}", cli.database_url))?;
    let service = Arc::new(SbolObjectService::new(pool.clone()));

    match cli.command {
        Command::Migrate { action } => match action {
            MigrateAction::Up => {
                run_migrations(&pool).await?;
                println!("migrations applied");
            }
            MigrateAction::Status => {
                let entries = sbol_db_postgres::pool::migration_status(&pool).await?;
                for entry in entries {
                    let marker = if entry.applied { "[x]" } else { "[ ]" };
                    println!("{marker} {} {}", entry.version, entry.description);
                }
            }
        },
        Command::Import {
            path,
            format,
            document_iri,
            name,
            parallel,
            continue_on_error,
            skip_existing,
        } => {
            let meta =
                std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
            if meta.is_dir() {
                if document_iri.is_some() || name.is_some() {
                    return Err(anyhow!(
                        "--document-iri and --name are only valid for single-file imports"
                    ));
                }
                if !continue_on_error && parallel > 1 {
                    return Err(anyhow!(
                        "--parallel requires --continue-on-error (atomic transactional \
                         imports are single-threaded by definition; set both flags to \
                         opt into per-file resilient mode)"
                    ));
                }
                if continue_on_error {
                    run_directory_import_per_file(
                        service.clone(),
                        &path,
                        format.as_deref(),
                        parallel.max(1),
                        skip_existing,
                    )
                    .await?;
                } else {
                    run_directory_import_atomic(
                        service.clone(),
                        &path,
                        format.as_deref(),
                        skip_existing,
                    )
                    .await?;
                }
            } else {
                let body = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let format = resolve_format(format.as_deref(), &path)?;
                if skip_existing {
                    let hash = hash_bytes(body.as_bytes());
                    if service.documents().exists_by_hash(&hash).await? {
                        println!("[skipped] {} (already imported)", path.display());
                        return Ok(());
                    }
                }
                let document_iri = document_iri.map(IriString::new).transpose()?;
                let report = service
                    .import_document(ImportInput {
                        body,
                        format,
                        source_uri: Some(path.display().to_string()),
                        document_iri,
                        created_by: None,
                        name,
                        description: None,
                    })
                    .await?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }
        Command::ExportAll {
            sbol_class,
            role,
            document_id,
            page_size,
        } => {
            run_export_all(service.clone(), sbol_class, role, document_id, page_size).await?;
        }
        Command::Get { iri, json } => {
            let obj = service
                .objects()
                .get_by_iri(&iri)
                .await?
                .ok_or_else(|| anyhow!("not found: {iri}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&obj.data)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
        }
        Command::Export { iri, format } => {
            let format =
                parse_format(&format).ok_or_else(|| anyhow!("unknown format: {format}"))?;
            // Look up the object id to validate the IRI exists.
            let obj = service
                .objects()
                .get_by_iri(&iri)
                .await?
                .ok_or_else(|| anyhow!("not found: {iri}"))?;
            let body = sbol_db_server_export(service.clone(), obj.id, format).await?;
            print!("{body}");
        }
        Command::Validate { document_id } => {
            println!(
                "Phase 1 placeholder: revalidation requires re-parseable raw payload; \
                 inspect the sbol_validation_runs / sbol_validation_findings tables directly for now."
            );
            let _ = document_id;
        }
        Command::Neighborhood {
            iri,
            depth,
            direction,
            predicates,
            max_nodes,
            literals,
            rdf,
        } => {
            let direction = parse_direction(&direction)?;
            let query = NeighborhoodQuery {
                root_iri: IriString::new(&iri)?,
                depth,
                direction,
                predicate_allowlist: predicates.into_iter().map(IriString::unchecked).collect(),
                max_nodes: Some(max_nodes),
                include_literals: literals,
            };
            let result = service.neighborhood().walk(&query).await?;
            if let Some(fmt) = rdf {
                let fmt = parse_format(&fmt).ok_or_else(|| anyhow!("unknown format: {fmt}"))?;
                let body = sbol_db_rdf::neighborhood_to_rdf(&result, fmt)?;
                print!("{body}");
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        Command::Sparql {
            source,
            format,
            timeout_secs,
            max_rows,
            max_query_size,
        } => {
            let query = read_query_source(&source)?;
            let format = format
                .map(|f| f.parse::<ResultFormat>())
                .transpose()
                .map_err(|e| anyhow!("{e}"))?;
            let engine = SparqlEngine::new(Arc::new(service.quads().clone()));
            let options = SparqlOptions {
                timeout: Duration::from_secs(timeout_secs),
                max_rows,
                max_query_size,
            };
            let outcome = engine
                .execute(&query, format, &options)
                .await
                .map_err(|e| anyhow!("{e}"))?;
            use std::io::Write;
            std::io::stdout().write_all(&outcome.payload.body)?;
            if outcome.payload.truncated {
                eprintln!("(result truncated at --max-rows={max_rows})");
            }
        }
        Command::Sequences { action } => match action {
            SequencesAction::Search {
                pattern,
                max_hits,
                forward_only,
            } => {
                let matches = service
                    .sequence_search()
                    .search(
                        &pattern,
                        SequenceSearchOptions {
                            max_hits: Some(max_hits),
                            forward_only: if forward_only { Some(true) } else { None },
                        },
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&matches)?);
            }
        },
        Command::Ontology { action } => match action {
            OntologyAction::Fetch { prefix, url, name } => {
                let (resolved_url, resolved_name) = resolve_ontology_source(&prefix, url, name)?;
                let report = service
                    .ontology()
                    .load_from_url(&prefix.to_ascii_uppercase(), &resolved_name, &resolved_url)
                    .await?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            OntologyAction::List => {
                let rows = service.ontology().list_ontologies().await?;
                println!("{}", serde_json::to_string_pretty(&rows)?);
            }
            OntologyAction::Descendants { iri_or_curie } => {
                let canonical = canonical_term_iri(&iri_or_curie);
                let descendants = service.ontology().descendants(&canonical).await?;
                println!("{}", serde_json::to_string_pretty(&descendants)?);
            }
        },
        Command::Serve { bind } => {
            let engine = Arc::new(SparqlEngine::new(Arc::new(service.quads().clone())));
            let metrics = Metrics::install(pool.clone(), env!("CARGO_PKG_VERSION"));
            let state = AppState {
                service,
                sparql: engine,
                metrics,
            };
            let app = router(state, sbol_db_server::ServerConfig::from_env());
            let listener = tokio::net::TcpListener::bind(bind).await?;
            tracing::info!(%bind, "sbol-db serving");
            println!("sbol-db listening on http://{bind}");
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            tracing::info!("sbol-db serve loop exited cleanly");
        }
    }
    Ok(())
}

/// Atomic directory import: read every file in the tree, then submit them
/// as one `import_documents` call so the whole batch commits or rolls back.
/// This is the default for `sbol-db import <dir>`.
async fn run_directory_import_atomic(
    service: Arc<SbolObjectService>,
    root: &std::path::Path,
    explicit_format: Option<&str>,
    skip_existing: bool,
) -> Result<()> {
    let files = collect_importable_files(root)?;
    if files.is_empty() {
        println!("no importable files under {}", root.display());
        return Ok(());
    }
    let total = files.len();
    println!(
        "preparing {total} file(s) from {} (transactional)",
        root.display()
    );

    let mut inputs: Vec<ImportInput> = Vec::with_capacity(total);
    let mut skipped = 0usize;
    for path in &files {
        let body =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let format = resolve_format(explicit_format, path)?;
        if skip_existing {
            let hash = hash_bytes(body.as_bytes());
            if service.documents().exists_by_hash(&hash).await? {
                skipped += 1;
                println!("[skip] {}", path.display());
                continue;
            }
        }
        inputs.push(ImportInput {
            body,
            format,
            source_uri: Some(path.display().to_string()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        });
    }
    if inputs.is_empty() {
        println!("nothing new to import ({skipped} skipped)");
        return Ok(());
    }
    println!(
        "committing {} document(s) in one transaction ({skipped} skipped)",
        inputs.len()
    );
    let reports = service
        .import_documents(inputs)
        .await
        .map_err(|e| anyhow!("rolled back: {e}"))?;
    println!(
        "summary: {} imported, {skipped} skipped — committed atomically",
        reports.len()
    );
    Ok(())
}

/// Per-file directory import: each file runs in its own transaction; failures
/// are reported but don't abort the batch. Enabled with `--continue-on-error`;
/// the right shape for corpus-scale onboarding where one bad file shouldn't
/// roll back the rest.
async fn run_directory_import_per_file(
    service: Arc<SbolObjectService>,
    root: &std::path::Path,
    explicit_format: Option<&str>,
    parallel: usize,
    skip_existing: bool,
) -> Result<()> {
    let files = collect_importable_files(root)?;
    if files.is_empty() {
        println!("no importable files under {}", root.display());
        return Ok(());
    }
    let total = files.len();
    println!(
        "importing {total} file(s) from {} (per-file, parallel={parallel})",
        root.display()
    );

    let explicit_format = explicit_format.map(str::to_owned);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let mut set: tokio::task::JoinSet<(PathBuf, OutcomeOfFile)> = tokio::task::JoinSet::new();
    for (idx, path) in files.into_iter().enumerate() {
        let svc = service.clone();
        let sem = semaphore.clone();
        let fmt = explicit_format.clone();
        set.spawn(async move {
            let permit = sem.acquire_owned().await.expect("semaphore");
            let outcome = import_one(svc, &path, fmt.as_deref(), skip_existing).await;
            drop(permit);
            let label = match &outcome {
                OutcomeOfFile::Imported(rep) => format!(
                    "imported ({} objects, {} quads, {:?})",
                    rep.object_count, rep.quad_count, rep.validation_status
                ),
                OutcomeOfFile::Skipped => "skipped (already imported)".to_owned(),
                OutcomeOfFile::Failed(err) => format!("FAILED: {err}"),
            };
            println!("[{}/{}] {} — {}", idx + 1, total, path.display(), label);
            (path, outcome)
        });
    }

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        let (path, outcome) = joined.context("join import task")?;
        match outcome {
            OutcomeOfFile::Imported(_) => imported += 1,
            OutcomeOfFile::Skipped => skipped += 1,
            OutcomeOfFile::Failed(err) => failed.push((path, err)),
        }
    }
    println!(
        "summary: {imported} imported, {skipped} skipped, {failed} failed",
        failed = failed.len()
    );
    Ok(())
}

enum OutcomeOfFile {
    Imported(sbol_db_core::ImportReport),
    Skipped,
    Failed(String),
}

async fn import_one(
    service: Arc<SbolObjectService>,
    path: &std::path::Path,
    explicit_format: Option<&str>,
    skip_existing: bool,
) -> OutcomeOfFile {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return OutcomeOfFile::Failed(format!("read: {e}")),
    };
    let format = match resolve_format(explicit_format, path) {
        Ok(f) => f,
        Err(e) => return OutcomeOfFile::Failed(e.to_string()),
    };
    if skip_existing {
        let hash = hash_bytes(body.as_bytes());
        match service.documents().exists_by_hash(&hash).await {
            Ok(true) => return OutcomeOfFile::Skipped,
            Ok(false) => {}
            Err(e) => return OutcomeOfFile::Failed(e.to_string()),
        }
    }
    match service
        .import_document(ImportInput {
            body,
            format,
            source_uri: Some(path.display().to_string()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        })
        .await
    {
        Ok(report) => OutcomeOfFile::Imported(report),
        Err(e) => OutcomeOfFile::Failed(e.to_string()),
    }
}

fn collect_importable_files(root: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_dir() {
                stack.push(path);
            } else if ty.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(SerializationFormat::from_extension)
                    .is_some()
            {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

async fn run_export_all(
    service: Arc<SbolObjectService>,
    sbol_class: Option<String>,
    role: Option<String>,
    document_id: Option<uuid::Uuid>,
    page_size: u32,
) -> Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut cursor: Option<String> = None;
    let limit = page_size.clamp(1, 5000);
    loop {
        let filter = ListObjectsFilter {
            sbol_class: sbol_class.clone(),
            role: role.clone(),
            document_id: document_id.map(DocumentId),
            after_iri: cursor.clone(),
            limit,
        };
        let page = service.objects().list(&filter).await?;
        let page_len = page.len();
        for record in &page {
            serde_json::to_writer(&mut out, record)?;
            out.write_all(b"\n")?;
        }
        if (page_len as u32) < limit {
            break;
        }
        cursor = page.last().map(|r| r.iri.as_str().to_owned());
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}

fn read_query_source(source: &str) -> Result<String> {
    if source == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    let path = std::path::Path::new(source);
    std::fs::read_to_string(path).with_context(|| format!("reading SPARQL source {source}"))
}

async fn sbol_db_server_export(
    service: Arc<SbolObjectService>,
    object_id: ObjectId,
    format: SerializationFormat,
) -> Result<String> {
    let iri = service
        .objects()
        .get_iri_by_id(object_id)
        .await?
        .ok_or_else(|| anyhow!("object id not found"))?;
    let body = sbol_db_server::export_subject_rdf(service.quads(), &iri, format).await?;
    Ok(body)
}

fn resolve_format(explicit: Option<&str>, path: &std::path::Path) -> Result<SerializationFormat> {
    if let Some(f) = explicit {
        return parse_format(f).ok_or_else(|| anyhow!("unknown format: {f}"));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("could not infer format from path {}", path.display()))?;
    SerializationFormat::from_extension(ext).ok_or_else(|| anyhow!("unknown extension: {ext}"))
}

fn parse_direction(s: &str) -> Result<Direction> {
    match s.to_ascii_lowercase().as_str() {
        "forward" | "out" => Ok(Direction::Forward),
        "backward" | "back" | "in" => Ok(Direction::Backward),
        "both" | "either" => Ok(Direction::Both),
        other => Err(anyhow!("unknown direction: {other}")),
    }
}

fn resolve_ontology_source(
    prefix: &str,
    url: Option<String>,
    name: Option<String>,
) -> Result<(String, String)> {
    let prefix_lower = prefix.to_ascii_lowercase();
    match prefix_lower.as_str() {
        "so" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/so.obo".to_owned()),
            name.unwrap_or_else(|| "Sequence Ontology".to_owned()),
        )),
        "sbo" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/sbo.obo".to_owned()),
            name.unwrap_or_else(|| "Systems Biology Ontology".to_owned()),
        )),
        _ => {
            let url = url.ok_or_else(|| {
                anyhow!("--url is required for ontology prefix {prefix} (no default known)")
            })?;
            let name = name.unwrap_or_else(|| prefix.to_owned());
            Ok((url, name))
        }
    }
}

fn canonical_term_iri(input: &str) -> String {
    if input.contains("://") {
        return input.to_owned();
    }
    if let Some((prefix, suffix)) = input.split_once(':') {
        let p = prefix.to_ascii_uppercase();
        return format!("http://purl.obolibrary.org/obo/{p}_{suffix}");
    }
    input.to_owned()
}

fn parse_format(s: &str) -> Option<SerializationFormat> {
    match s.to_ascii_lowercase().as_str() {
        "turtle" | "ttl" => Some(SerializationFormat::Turtle),
        "jsonld" => Some(SerializationFormat::JsonLd),
        "rdfxml" | "rdf" | "xml" => Some(SerializationFormat::RdfXml),
        "ntriples" | "nt" => Some(SerializationFormat::NTriples),
        "nquads" | "nq" => Some(SerializationFormat::NQuads),
        "trig" => Some(SerializationFormat::TriG),
        "json" => Some(SerializationFormat::Json),
        _ => None,
    }
}

fn init_logging() {
    use std::io::IsTerminal;
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // LOG_FORMAT={json,text} forces a format; otherwise default to JSON
    // when stdout isn't a TTY (containers, pipes) and human-readable
    // when it is.
    let want_json = match std::env::var("LOG_FORMAT")
        .ok()
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => true,
        Some("text") | Some("plain") | Some("human") => false,
        _ => !std::io::stdout().is_terminal(),
    };
    if want_json {
        let _ = fmt()
            .with_env_filter(filter)
            .with_target(false)
            .json()
            .try_init();
    } else {
        let _ = fmt().with_env_filter(filter).with_target(false).try_init();
    }
}

/// Commands that need a long startup retry loop (`serve`, `migrate up`)
/// honor `DATABASE_STARTUP_TIMEOUT_SECS`; everything else fails fast on
/// the first connection error.
async fn open_pool(database_url: &str, command: &Command) -> Result<sbol_db_postgres::PgPool> {
    let needs_retry = matches!(command, Command::Serve { .. } | Command::Migrate { .. });
    let deadline = if needs_retry {
        Duration::from_secs(
            std::env::var("DATABASE_STARTUP_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
        )
    } else {
        Duration::ZERO
    };
    connect_with_retry(database_url, deadline)
        .await
        .map_err(Into::into)
}

/// Listens for SIGTERM (k8s pod termination) and Ctrl-C so axum can
/// drain in-flight requests during a Helm rollout instead of dropping
/// them mid-flight.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!(signal = "SIGINT", "shutdown signal received"),
        _ = terminate => tracing::info!(signal = "SIGTERM", "shutdown signal received"),
    }
}
