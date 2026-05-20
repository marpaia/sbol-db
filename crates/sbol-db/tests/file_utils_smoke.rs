//! Smoke tests for `sbol-db util` subcommands. These run unconditionally;
//! no `DATABASE_URL` is required.

use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;

fn fixture_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("sbol-db-postgres/tests/fixtures/simple_component.ttl")
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::cargo_bin("sbol-db")
        .unwrap()
        .args(args)
        .output()
        .expect("run sbol-db")
}

#[test]
fn hash_content_matches_across_invocations() {
    let fixture = fixture_path();
    let out_a = cli(&["util", "hash", fixture.to_str().unwrap()]);
    assert!(out_a.status.success(), "first hash failed: {out_a:?}");
    let json_a: serde_json::Value = serde_json::from_slice(&out_a.stdout).unwrap();
    let hash_a = json_a["hash_hex"].as_str().unwrap().to_owned();
    assert_eq!(json_a["mode"], "content");
    assert!(json_a["triple_count"].as_u64().unwrap() > 0);

    let out_b = cli(&["util", "hash", fixture.to_str().unwrap()]);
    assert!(out_b.status.success(), "second hash failed: {out_b:?}");
    let json_b: serde_json::Value = serde_json::from_slice(&out_b.stdout).unwrap();
    assert_eq!(json_b["hash_hex"].as_str().unwrap(), hash_a);
}

#[test]
fn hash_bytes_mode_differs_from_content_mode() {
    let fixture = fixture_path();
    let content = cli(&["util", "hash", fixture.to_str().unwrap()]);
    let bytes = cli(&["util", "hash", "--bytes", fixture.to_str().unwrap()]);
    let c: serde_json::Value = serde_json::from_slice(&content.stdout).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&bytes.stdout).unwrap();
    assert_eq!(c["mode"], "content");
    assert_eq!(b["mode"], "bytes");
    assert_ne!(c["hash_hex"], b["hash_hex"]);
}

#[test]
fn kmer_encode_round_trips_a_known_sequence() {
    let out = cli(&["util", "kmer-encode", "ACGTACGT"]);
    assert!(out.status.success(), "kmer-encode failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["sequence"], "ACGTACGT");
    assert!(json["encoded"].is_number());
}

#[test]
fn kmer_revcomp_produces_complement() {
    // `reverse_complement_string` lowercases by contract.
    let out = cli(&["util", "kmer-revcomp", "ACGT"]);
    assert!(out.status.success(), "kmer-revcomp failed: {out:?}");
    let body = String::from_utf8(out.stdout).unwrap();
    assert_eq!(body.trim(), "acgt"); // ACGT is its own reverse complement
}

#[test]
fn kmer_canonical_emits_one_record_per_window() {
    let out = cli(&["util", "kmer-canonical", "ACGTACGTAC"]);
    assert!(out.status.success(), "kmer-canonical failed: {out:?}");
    let body = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    // 10 - 8 + 1 = 3 windows
    assert_eq!(lines.len(), 3, "expected 3 windows, got: {lines:?}");
    for line in &lines {
        let _: serde_json::Value = serde_json::from_str(line).expect("jsonl");
    }
}
