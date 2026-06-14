//! `sbol-db doc` — operations on the stored document corpus.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sbol_db_core::{DocumentId, ImportReport, IriString, SerializationFormat};
use sbol_db_postgres::{
    DocumentRepository, ImportInput, ListDocumentsFilter, PgPool, SbolObjectService,
};
use sbol_db_rdf::hash_bytes;

use crate::cli::DocAction;
use crate::format::{parse_format, resolve_format};
use crate::output::print_json;

pub async fn run(pool: PgPool, service: Arc<SbolObjectService>, action: DocAction) -> Result<()> {
    match action {
        DocAction::Import {
            path,
            format,
            namespace,
            document_iri,
            name,
            continue_on_error,
            parallel,
            skip_existing,
        } => {
            import(
                service,
                path,
                format,
                namespace,
                document_iri,
                name,
                continue_on_error,
                parallel,
                skip_existing,
            )
            .await
        }
        DocAction::List {
            limit,
            name,
            format,
        } => {
            let repo = DocumentRepository::new(pool);
            let format = format
                .as_deref()
                .map(|f| parse_format(f).ok_or_else(|| anyhow!("unknown format: {f}")))
                .transpose()?;
            let rows = repo
                .list(&ListDocumentsFilter {
                    name,
                    format,
                    limit,
                })
                .await?;
            print_json(&rows)
        }
        DocAction::Show { id } => {
            let repo = DocumentRepository::new(pool);
            let doc = repo
                .get(DocumentId(id))
                .await?
                .ok_or_else(|| anyhow!("no document with id {id}"))?;
            print_json(&doc)
        }
        DocAction::Delete { id, yes } => delete(pool, id, yes).await,
        DocAction::Validate { document_id } => {
            println!(
                "Phase 1 placeholder: revalidation requires re-parseable raw payload; \
                 inspect the sbol_validation_runs / sbol_validation_findings tables \
                 directly for now."
            );
            let _ = document_id;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn import(
    service: Arc<SbolObjectService>,
    path: PathBuf,
    format: Option<String>,
    namespace: Option<String>,
    document_iri: Option<String>,
    name: Option<String>,
    continue_on_error: bool,
    parallel: usize,
    skip_existing: bool,
) -> Result<()> {
    let meta = std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
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
                service,
                &path,
                format.as_deref(),
                namespace.as_deref(),
                parallel.max(1),
                skip_existing,
            )
            .await
        } else {
            run_directory_import_atomic(
                service,
                &path,
                format.as_deref(),
                namespace.as_deref(),
                skip_existing,
            )
            .await
        }
    } else {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let format = resolve_format(format.as_deref(), &path)?;
        let namespace = namespace.or_else(|| default_namespace_for_path(&path, format));
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
                namespace,
                source_uri: Some(path.display().to_string()),
                document_iri,
                created_by: None,
                name,
                description: None,
            })
            .await?;
        print_json(&report)
    }
}

async fn delete(pool: PgPool, id: uuid::Uuid, yes: bool) -> Result<()> {
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(anyhow!(
                "refusing to delete without --yes when stdin is not a TTY"
            ));
        }
        eprint!("delete document {id}? [y/N] ");
        use std::io::{BufRead, Write};
        std::io::stderr().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            eprintln!("aborted");
            return Ok(());
        }
    }
    let repo = DocumentRepository::new(pool);
    let deleted = repo.delete(DocumentId(id)).await?;
    print_json(&serde_json::json!({
        "id": id,
        "deleted": deleted,
    }))
}

async fn run_directory_import_atomic(
    service: Arc<SbolObjectService>,
    root: &Path,
    explicit_format: Option<&str>,
    namespace: Option<&str>,
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
        let namespace = namespace
            .map(str::to_owned)
            .or_else(|| default_namespace_for_path(path, format));
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
            namespace,
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
        "summary: {} imported, {skipped} skipped, committed atomically",
        reports.len()
    );
    Ok(())
}

async fn run_directory_import_per_file(
    service: Arc<SbolObjectService>,
    root: &Path,
    explicit_format: Option<&str>,
    namespace: Option<&str>,
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
    let namespace = namespace.map(str::to_owned);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let mut set: tokio::task::JoinSet<(PathBuf, OutcomeOfFile)> = tokio::task::JoinSet::new();
    for (idx, path) in files.into_iter().enumerate() {
        let svc = service.clone();
        let sem = semaphore.clone();
        let fmt = explicit_format.clone();
        let namespace = namespace.clone();
        set.spawn(async move {
            let permit = sem.acquire_owned().await.expect("semaphore");
            let outcome = import_one(
                svc,
                &path,
                fmt.as_deref(),
                namespace.as_deref(),
                skip_existing,
            )
            .await;
            drop(permit);
            let label = match &outcome {
                OutcomeOfFile::Imported(rep) => format!(
                    "imported ({} objects, {} quads, {:?})",
                    rep.object_count, rep.quad_count, rep.validation_status
                ),
                OutcomeOfFile::Skipped => "skipped (already imported)".to_owned(),
                OutcomeOfFile::Failed(err) => format!("FAILED: {err}"),
            };
            println!("[{}/{}] {}: {}", idx + 1, total, path.display(), label);
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
    Imported(ImportReport),
    Skipped,
    Failed(String),
}

async fn import_one(
    service: Arc<SbolObjectService>,
    path: &Path,
    explicit_format: Option<&str>,
    namespace: Option<&str>,
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
    let namespace = namespace
        .map(str::to_owned)
        .or_else(|| default_namespace_for_path(path, format));
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
            namespace,
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

fn collect_importable_files(root: &Path) -> Result<Vec<PathBuf>> {
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

fn default_namespace_for_path(path: &Path, format: SerializationFormat) -> Option<String> {
    match format {
        SerializationFormat::Turtle
        | SerializationFormat::JsonLd
        | SerializationFormat::RdfXml
        | SerializationFormat::NTriples
        | SerializationFormat::GenBank
        | SerializationFormat::Fasta => {}
        SerializationFormat::Json | SerializationFormat::TriG | SerializationFormat::NQuads => {
            return None;
        }
    }
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    let segment = sanitize_namespace_segment(stem);
    (!segment.is_empty()).then(|| format!("https://sbol-db.local/imports/{segment}"))
}

fn sanitize_namespace_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_was_sep = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            Some(ch)
        } else if ch.is_ascii_whitespace() || matches!(ch, '.' | '/' | '\\' | ':') {
            Some('_')
        } else {
            None
        };
        if let Some(ch) = mapped {
            if ch == '_' {
                if previous_was_sep {
                    continue;
                }
                previous_was_sep = true;
            } else {
                previous_was_sep = false;
            }
            out.push(ch);
        }
    }
    out.trim_matches('_').to_owned()
}
