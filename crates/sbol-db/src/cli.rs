//! Clap definitions for the `sbol-db` binary.
//!
//! The CLI is noun-first: top-level commands are nouns (`doc`, `object`,
//! `query`, ...) and each noun owns a small set of verbs as a subcommand
//! enum. Single-process daemons (`server`, `worker`) stay top-level
//! because they are not "operations on a noun" — they are the noun.
//!
//! Handler bodies live in `crate::cmd::*`; this file is the parser
//! surface only. Doc-comment convention: the first line is the short
//! summary shown in `--help` listings; anything after the blank `///`
//! line is the long form shown under `<cmd> --help`.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(version, about = "sbol-db CLI", long_about = None)]
pub struct Cli {
    /// Storage backend connection string. The scheme selects the backend:
    /// `postgres://`, `sqlite://`, or `rocksdb://`. With `--backend` set, a
    /// bare path (no scheme) is also accepted.
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://sbol:sbol@localhost:5432/sbol"
    )]
    pub database_url: String,

    /// Storage backend. When omitted it is inferred from `--database-url`'s
    /// scheme. When set, it must agree with that scheme (or the URL may be a
    /// bare path, which this scheme then completes).
    #[arg(long, value_enum, env = "SBOL_DB_BACKEND")]
    pub backend: Option<BackendKind>,

    #[command(subcommand)]
    pub command: Command,
}

/// The storage backend selectors accepted by `--backend` / `SBOL_DB_BACKEND`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    Postgres,
    Sqlite,
    Rocksdb,
}

impl BackendKind {
    /// The canonical connection-string scheme for this backend.
    pub fn scheme(self) -> &'static str {
        match self {
            BackendKind::Postgres => "postgres",
            BackendKind::Sqlite => "sqlite",
            BackendKind::Rocksdb => "rocksdb",
        }
    }

    /// Whether a connection-string scheme belongs to this backend (Postgres
    /// answers to both `postgres` and `postgresql`).
    pub fn accepts_scheme(self, scheme: &str) -> bool {
        match self {
            BackendKind::Postgres => scheme == "postgres" || scheme == "postgresql",
            BackendKind::Sqlite => scheme == "sqlite",
            BackendKind::Rocksdb => scheme == "rocksdb",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the HTTP server (and embedded worker).
    ///
    /// By default an async-job worker runs in the same process,
    /// subscribed to every registered queue. Use `--no-worker` on
    /// API-only nodes when a dedicated worker fleet runs elsewhere
    /// (see `sbol-db worker`).
    Server {
        #[arg(long, env = "SBOL_DB_BIND", default_value = "127.0.0.1:8888")]
        bind: SocketAddr,
        /// Disable the embedded worker.
        #[arg(long, env = "SBOL_DB_WORKER_DISABLED")]
        no_worker: bool,
        /// Maximum concurrent in-flight handler tasks. Defaults to the
        /// machine's available parallelism.
        #[arg(long, env = "SBOL_DB_WORKER_CONCURRENCY")]
        worker_concurrency: Option<usize>,
        /// Comma-separated queue allowlist. Defaults to all registered
        /// queues (currently just `default`).
        #[arg(long, env = "SBOL_DB_WORKER_QUEUES")]
        worker_queues: Option<String>,
        /// Stable worker identity for log attribution. Defaults to
        /// `<hostname>-<pid>-<random>`.
        #[arg(long, env = "SBOL_DB_WORKER_ID")]
        worker_id: Option<String>,
    },

    /// Run a standalone async-job worker (no HTTP listener).
    ///
    /// Stops on SIGTERM / Ctrl-C; in-flight handlers get a grace window
    /// before their leases are abandoned.
    Worker {
        #[arg(long, env = "SBOL_DB_WORKER_CONCURRENCY")]
        concurrency: Option<usize>,
        #[arg(long, env = "SBOL_DB_WORKER_QUEUES")]
        queues: Option<String>,
        #[arg(long, env = "SBOL_DB_WORKER_ID")]
        worker_id: Option<String>,
    },

    /// Named graphs (the import corpus and any RDF graphs).
    Graph {
        #[command(subcommand)]
        action: GraphAction,
    },

    /// Stored objects (the typed SBOL view derived from graphs).
    Object {
        #[command(subcommand)]
        action: ObjectAction,
    },

    /// Query the graph.
    Query {
        #[command(subcommand)]
        action: QueryAction,
    },

    /// Loaded OBO ontologies.
    Ontology {
        #[command(subcommand)]
        action: OntologyAction,
    },

    /// Async job queue operations.
    Jobs {
        #[command(subcommand)]
        action: JobsAction,
    },

    /// Database lifecycle: migrations, health check.
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// Read-only Postgres inspection.
    ///
    /// Wraps the same `pg_stat_*` / `pg_locks` queries the lab UI reads.
    Inspect {
        #[command(subcommand)]
        action: InspectAction,
    },

    /// Local utilities (file hashing, k-mer debug, ...).
    ///
    /// None of these touch Postgres.
    Util {
        #[command(subcommand)]
        action: UtilAction,
    },
}

// -----------------------------------------------------------------------
// graph
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GraphAction {
    /// Import SBOL documents from a file or directory.
    ///
    /// `path` may be a single file or a directory; directories are walked
    /// recursively for files whose extension is a recognised import format:
    /// SBOL RDF (`.ttl`, `.nt`, `.jsonld`, `.rdf`, `.xml`, `.trig`, `.nq`),
    /// GenBank (`.gb`, `.gbk`), or FASTA (`.fa`, `.fasta`, `.fna`, `.faa`).
    ///
    /// Directory imports default to one atomic Postgres transaction:
    /// either every file commits or none do. Use `--continue-on-error`
    /// for corpus-scale onboarding where per-file resilience matters
    /// more than batch atomicity; that mode also enables `--parallel`.
    Import {
        path: PathBuf,
        /// Override the format inferred from file extensions. For directory
        /// imports, applies to every collected file.
        #[arg(long)]
        format: Option<String>,
        /// Namespace IRI for formats that need one. SBOL 2 uses this only
        /// as the upgrade fallback; GenBank and FASTA default to a stable
        /// `https://sbol-db.local/imports/<file-stem>` namespace when omitted.
        #[arg(long)]
        namespace: Option<String>,
        /// Optional document IRI to record alongside the import. Only allowed
        /// for single-file imports.
        #[arg(long)]
        document_iri: Option<String>,
        /// Optional name. Only allowed for single-file imports.
        #[arg(long)]
        name: Option<String>,
        /// Run each file in its own transaction in parallel, continuing past
        /// per-file failures.
        #[arg(long)]
        continue_on_error: bool,
        /// Number of files to import in parallel. Only valid with
        /// `--continue-on-error`.
        #[arg(long, default_value_t = 1)]
        parallel: usize,
        /// Skip files whose SHA3-256 content hash is already present in
        /// the document corpus.
        #[arg(long)]
        skip_existing: bool,
    },
    /// List stored documents, newest first.
    List {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Filter by case-insensitive substring against the document name.
        #[arg(long)]
        name: Option<String>,
        /// Filter by serialization format (`turtle`, `ntriples`, ...).
        #[arg(long)]
        format: Option<String>,
    },
    /// Show one document by id.
    Show { id: uuid::Uuid },
    /// Delete a document by id.
    ///
    /// Cascades to its triples via FK; objects whose sole source was this
    /// document are left with a NULL `graph_id`.
    Delete {
        id: uuid::Uuid,
        /// Skip the confirmation prompt. Required when stdin isn't a TTY.
        #[arg(long)]
        yes: bool,
    },
    /// Re-print validation findings for a document.
    ///
    /// Not yet implemented: revalidation requires re-parseable raw payload
    /// retention, which is not yet wired through.
    Validate { graph_id: uuid::Uuid },
}

// -----------------------------------------------------------------------
// object
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ObjectAction {
    /// Fetch a stored object by its IRI.
    Get {
        iri: String,
        #[arg(long)]
        json: bool,
    },
    /// Export an object subgraph as RDF.
    Export {
        iri: String,
        #[arg(long, default_value = "turtle")]
        format: String,
    },
    /// Stream every stored object as newline-delimited JSON.
    ///
    /// Pages through `sbol_objects` with a keyset cursor; safe for
    /// corpus-scale dumps. One `SbolObjectRecord` per line.
    ExportAll {
        /// Restrict to objects whose `sbol_class` equals this IRI.
        #[arg(long)]
        sbol_class: Option<String>,
        /// Restrict to objects carrying this role IRI in their `roles` array.
        #[arg(long)]
        role: Option<String>,
        /// Restrict to objects belonging to a specific document.
        #[arg(long)]
        graph_id: Option<uuid::Uuid>,
        /// Page size used internally; max 5000.
        #[arg(long, default_value_t = 1000)]
        page_size: u32,
    },
}

// -----------------------------------------------------------------------
// query
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum QueryAction {
    /// Run a read-only SPARQL query.
    ///
    /// SELECT/ASK/CONSTRUCT/DESCRIBE only. UPDATE strings are rejected at
    /// parse time.
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
    /// Parse a SPARQL query without executing it.
    ///
    /// Pre-flight for the syntax errors that `query sparql` only surfaces
    /// after a DB round-trip. Prints the detected form and AST.
    Explain {
        /// Path to a `.rq` query file, or `-` to read from stdin.
        source: String,
    },
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
        /// of JSON.
        #[arg(long)]
        rdf: Option<String>,
    },
    /// Substring search across indexed sequences.
    ///
    /// Includes the pattern's reverse complement unless `--forward-only`.
    SequenceSearch {
        pattern: String,
        #[arg(long, default_value_t = 1024)]
        max_hits: u32,
        #[arg(long)]
        forward_only: bool,
    },
    /// Run many patterns against the indexed sequences in one shot.
    ///
    /// Reads newline-delimited patterns from a file or stdin (`-`) and
    /// emits one JSON object per line keyed by query.
    SequenceBatch {
        /// Path to a file with one pattern per line, or `-` for stdin.
        source: String,
        #[arg(long, default_value_t = 1024)]
        max_hits: u32,
        #[arg(long)]
        forward_only: bool,
    },
}

// -----------------------------------------------------------------------
// ontology
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum OntologyAction {
    /// Fetch and load an OBO ontology from a URL.
    ///
    /// Recognised shorthand prefixes (`so`, `sbo`) use sensible defaults;
    /// for any other prefix supply `--url` and `--name`.
    Fetch {
        prefix: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// List ontologies currently loaded into the database.
    List,
    /// Show one term's full record.
    ///
    /// Accepts an IRI or CURIE; resolved canonically.
    Term { iri_or_curie: String },
    /// Show every descendant of a term.
    Descendants { iri_or_curie: String },
    /// Load an OBO ontology from a local file.
    LoadFile {
        path: PathBuf,
        /// Short prefix the terms are scoped under (`SO`, `SBO`, ...).
        #[arg(long)]
        prefix: String,
        /// Human-friendly ontology name; falls back to the prefix.
        #[arg(long)]
        name: Option<String>,
    },
}

// -----------------------------------------------------------------------
// jobs
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum JobsAction {
    /// Enqueue a job.
    Enqueue {
        kind: String,
        /// JSON payload, inline (`'{"...":...}'`) or `@path/to/file.json`
        /// to read from disk.
        payload: String,
        #[arg(long)]
        queue: Option<String>,
        #[arg(long)]
        priority: Option<i16>,
        #[arg(long)]
        max_attempts: Option<i32>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Show one job by id.
    Status { id: uuid::Uuid },
    /// List recent jobs, newest first.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        queue: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Cancel a queued or running job.
    Cancel { id: uuid::Uuid },
    /// Show the per-attempt audit log for a job.
    Attempts { id: uuid::Uuid },
    /// Re-enqueue a failed/dead/cancelled job.
    Replay {
        id: uuid::Uuid,
        /// Inherit the original `idempotency_key`.
        #[arg(long)]
        keep_idempotency_key: bool,
    },
    /// Snapshot of jobs-per-(status, queue).
    QueueDepth,
    /// Per-queue age of the oldest still-queued job.
    QueueAge,
    /// List job kinds in the in-process registry.
    Handlers,
}

// -----------------------------------------------------------------------
// db
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum DbAction {
    /// Apply all pending migrations.
    Migrate,
    /// Show migration status.
    MigrateStatus,
    /// Composite health check.
    ///
    /// Checks DB reachability, migration status, worker registry, queue
    /// depth, and ontology load. Exits non-zero on any failure.
    Doctor {
        /// Emit a structured JSON report instead of human-readable lines.
        #[arg(long)]
        json: bool,
        /// Comma-separated list of ontology prefixes the doctor should
        /// require to be loaded.
        #[arg(long, default_value = "SO")]
        require_ontologies: String,
        /// Maximum allowed age in seconds for the oldest queued job before
        /// the queue-depth check fails.
        #[arg(long, default_value_t = 3600)]
        max_queued_age_secs: i64,
    },
}

// -----------------------------------------------------------------------
// inspect
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum InspectAction {
    /// Total size of the connected database.
    Size,
    /// List every user table with row-estimate and size.
    Tables {
        #[arg(long, default_value_t = 100)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    /// Schema and stats for one table.
    Table { name: String },
    /// Current Postgres backend activity.
    Activity {
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Include idle connections; off by default since they're noise.
        #[arg(long)]
        include_idle: bool,
    },
    /// Blocking-lock pairs from `pg_locks`.
    Locks,
    /// Busiest indexes.
    Indexes {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Top-N slow queries from `pg_stat_statements`.
    SlowQueries {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Print the effective ServerConfig as JSON.
    Config,
}

// -----------------------------------------------------------------------
// util
// -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum UtilAction {
    /// Content-hash an RDF file.
    ///
    /// Matches the import dedup key, so the output equals the
    /// `content_hash` of a successfully imported file.
    Hash {
        path: PathBuf,
        /// Hash raw bytes instead of the parsed-triple content.
        #[arg(long)]
        bytes: bool,
        /// Override the format inferred from the file extension.
        #[arg(long)]
        format: Option<String>,
    },
    /// Encode an 8-character DNA sequence as a 32-bit integer.
    KmerEncode { sequence: String },
    /// Emit canonical k-mers for a sequence as JSONL.
    KmerCanonical { sequence: String },
    /// Reverse-complement a DNA/RNA sequence.
    KmerRevcomp { sequence: String },
}
