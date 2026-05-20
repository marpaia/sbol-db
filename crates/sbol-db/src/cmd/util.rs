//! `sbol-db util` — local utilities that don't touch Postgres: file
//! content hashing and k-mer debug helpers.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use sbol::Document;
use sbol_db_core::kmer::{
    canonical_kmers, encode_kmer, reverse_complement_string, KmerStrand, KMER_K,
};
use sbol_db_rdf::{content_hash, hash_bytes};

use crate::cli::UtilAction;
use crate::format::{resolve_format, serialization_to_rdf_format};
use crate::output::{print_json, write_jsonl};

pub async fn run(action: UtilAction) -> Result<()> {
    match action {
        UtilAction::Hash {
            path,
            bytes,
            format,
        } => hash(path, bytes, format).await,
        UtilAction::KmerEncode { sequence } => {
            if sequence.len() != KMER_K {
                return Err(anyhow!(
                    "expected {KMER_K}-base sequence, got {} bases",
                    sequence.len()
                ));
            }
            let encoded = encode_kmer(&sequence)
                .ok_or_else(|| anyhow!("sequence contains a non-IUPAC base"))?;
            print_json(&serde_json::json!({
                "sequence": sequence,
                "encoded": encoded,
                "encoded_hex": format!("{encoded:#010x}"),
            }))
        }
        UtilAction::KmerCanonical { sequence } => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for hit in canonical_kmers(&sequence) {
                let record = serde_json::json!({
                    "position": hit.position,
                    "canonical": hit.canonical,
                    "canonical_hex": format!("{:#010x}", hit.canonical),
                    "strand": strand_char(hit.strand),
                });
                write_jsonl(&mut out, &record)?;
            }
            out.flush()?;
            Ok(())
        }
        UtilAction::KmerRevcomp { sequence } => {
            println!("{}", reverse_complement_string(&sequence));
            Ok(())
        }
    }
}

async fn hash(path: PathBuf, bytes: bool, format: Option<String>) -> Result<()> {
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if bytes {
        let h = hash_bytes(body.as_bytes());
        print_json(&serde_json::json!({
            "path": path.display().to_string(),
            "mode": "bytes",
            "hash_hex": hex_encode(&h),
            "bytes_hashed": body.len(),
        }))
    } else {
        let format = resolve_format(format.as_deref(), &path)?;
        let rdf_format = serialization_to_rdf_format(format)?;
        let doc = Document::read(&body, rdf_format)
            .with_context(|| format!("parsing {}", path.display()))?;
        let triples = doc.rdf_graph().triples().to_vec();
        let triple_count = triples.len();
        let h = content_hash(&triples);
        print_json(&serde_json::json!({
            "path": path.display().to_string(),
            "mode": "content",
            "hash_hex": hex_encode(&h),
            "triple_count": triple_count,
        }))
    }
}

fn strand_char(s: KmerStrand) -> char {
    s.as_char()
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
