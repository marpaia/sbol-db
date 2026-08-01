use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use sbol_db_ha_sim::load_corpus;
use sbol_db_ha_test::{run_process_chaos, ProcessChaosConfig};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(about = "Run the external-process RocksDB HA systems test")]
struct Cli {
    /// Pinned, clean SBOLTestSuite checkout.
    #[arg(long, env = "SBOL_TEST_SUITE_ROOT")]
    corpus_root: PathBuf,

    #[arg(
        long,
        default_value = "crates/sbol-db-search-eval/fixtures/sbol-test-suite-integration-v1.json"
    )]
    manifest: PathBuf,

    /// Pre-built standalone node executable. Defaults to the runner's sibling.
    #[arg(long)]
    node_binary: Option<PathBuf>,

    /// New directory in which all histories, logs, and RocksDB data are kept.
    #[arg(long)]
    artifact_root: Option<PathBuf>,

    #[arg(long, default_value = "0x5b01db0000000001", value_parser = parse_seed)]
    seed: u64,

    #[arg(long, default_value_t = 37)]
    retry_every: usize,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 6)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let node_binary = match cli.node_binary {
        Some(path) => path,
        None => std::env::current_exe()?
            .parent()
            .context("runner executable has no parent directory")?
            .join("sbol-db-ha-node"),
    };
    let artifact_root = cli.artifact_root.unwrap_or_else(|| {
        PathBuf::from("target/ha-runs").join(format!(
            "process-{:016x}-{}",
            cli.seed,
            Uuid::new_v4().simple()
        ))
    });
    if artifact_root.exists()
        && std::fs::read_dir(&artifact_root)
            .with_context(|| format!("reading {}", artifact_root.display()))?
            .next()
            .is_some()
    {
        bail!(
            "artifact directory must be absent or empty: {}",
            artifact_root.display()
        );
    }
    let corpus = load_corpus(&cli.manifest, &cli.corpus_root)?;
    eprintln!(
        "loaded {} documents at {} for real-process HA testing",
        corpus.documents.len(),
        corpus.manifest.source.commit
    );
    let report = run_process_chaos(
        &corpus,
        ProcessChaosConfig {
            seed: cli.seed,
            retry_every: cli.retry_every,
            node_binary,
            artifact_root: artifact_root.clone(),
        },
    )
    .await
    .with_context(|| {
        format!(
            "single-host process chaos failed; artifacts retained at {}",
            artifact_root.display()
        )
    })?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_seed(raw: &str) -> Result<u64, String> {
    if let Some(hex) = raw.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|error| error.to_string())
    } else {
        raw.parse::<u64>().map_err(|error| error.to_string())
    }
}
