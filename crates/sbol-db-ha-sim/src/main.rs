use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use sbol_db_ha_sim::{load_corpus, run_corpus_chaos, ScenarioConfig};

#[derive(Debug, Parser)]
#[command(about = "Run replayable multi-node RocksDB/Raft chaos simulations")]
struct Cli {
    /// Pinned, clean SBOLTestSuite checkout.
    #[arg(long, env = "SBOL_TEST_SUITE_ROOT")]
    corpus_root: PathBuf,

    /// Corpus provenance and expected-count manifest.
    #[arg(
        long,
        default_value = "crates/sbol-db-search-eval/fixtures/sbol-test-suite-integration-v1.json"
    )]
    manifest: PathBuf,

    /// Reproduction seed, accepted as decimal or 0x-prefixed hexadecimal.
    #[arg(long, default_value = "0x5b01db0000000001", value_parser = parse_seed)]
    seed: u64,

    /// Submit an exact duplicate request every N documents; zero disables retries.
    #[arg(long, default_value_t = 37)]
    retry_every: usize,

    /// Write the complete JSON event trace here as well as the summary to stdout.
    #[arg(long)]
    trace: Option<PathBuf>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let corpus = load_corpus(&cli.manifest, &cli.corpus_root)?;
    eprintln!(
        "loaded {} importable documents at {} (fingerprint {})",
        corpus.documents.len(),
        corpus.manifest.source.commit,
        corpus.fingerprint
    );
    for group in &corpus.groups {
        eprintln!(
            "  {}: {} imported, {} expected parse failures",
            group.path,
            group.imported_documents,
            group.parse_failures.len()
        );
    }

    let report = run_corpus_chaos(
        &corpus,
        ScenarioConfig {
            seed: cli.seed,
            retry_every: cli.retry_every,
        },
    )
    .await
    .with_context(|| {
        format!(
            "chaos simulation failed; replay with --seed {:#x}",
            cli.seed
        )
    })?;
    let encoded = serde_json::to_string_pretty(&report).context("encoding simulation report")?;
    if let Some(trace) = cli.trace {
        fs::write(&trace, format!("{encoded}\n"))
            .with_context(|| format!("writing trace {}", trace.display()))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "seed": report.seed,
                "corpus_commit": report.corpus_commit,
                "corpus_fingerprint": report.corpus_fingerprint,
                "document_count": report.document_count,
                "acknowledged_document_writes": report.acknowledged_document_writes,
                "final_state_sha256": report.final_state_sha256,
                "duration_ms": report.duration_ms,
                "trace": trace,
            }))?
        );
    } else {
        println!("{encoded}");
    }
    Ok(())
}

fn parse_seed(raw: &str) -> Result<u64, String> {
    if let Some(hex) = raw.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|error| error.to_string())
    } else {
        raw.parse::<u64>().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_accept_decimal_and_hex() {
        assert_eq!(parse_seed("42").unwrap(), 42);
        assert_eq!(parse_seed("0x2a").unwrap(), 42);
    }
}
