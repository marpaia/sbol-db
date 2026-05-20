//! Smoke tests for the operational `jobs` subcommands plus a couple of
//! cross-noun checks (`ontology term`, `query sequence-batch`). Each
//! exercises the compiled binary against the live `DATABASE_URL`.

use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;

fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

fn cli(url: &str, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("sbol-db")
        .unwrap()
        .args(args)
        .env("DATABASE_URL", url)
        .output()
        .expect("run sbol-db")
}

#[test]
fn handlers_lists_at_least_one_kind() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let out = cli(&url, &["jobs", "handlers"]);
    assert!(out.status.success(), "handlers failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let kinds = json["kinds"].as_array().unwrap();
    assert!(!kinds.is_empty());
}

#[test]
fn queue_depth_and_queue_age_are_json_arrays() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let depth = cli(&url, &["jobs", "queue-depth"]);
    assert!(depth.status.success(), "queue-depth failed: {depth:?}");
    let json: serde_json::Value = serde_json::from_slice(&depth.stdout).unwrap();
    assert!(json.is_array());

    let age = cli(&url, &["jobs", "queue-age"]);
    assert!(age.status.success(), "queue-age failed: {age:?}");
    let json: serde_json::Value = serde_json::from_slice(&age.stdout).unwrap();
    assert!(json.is_array());
}

#[test]
fn attempts_for_unknown_id_returns_empty() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let bogus = "00000000-0000-0000-0000-000000000000";
    let out = cli(&url, &["jobs", "attempts", bogus]);
    assert!(out.status.success(), "attempts failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[test]
fn sequences_search_batch_emits_one_record_per_pattern() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let path = std::env::temp_dir().join(format!(
        "sbol-db-batch-{}-{}.txt",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, "ACGT\nGCAT\n").unwrap();
    let out = cli(
        &url,
        &[
            "query",
            "sequence-batch",
            path.to_str().unwrap(),
            "--max-hits",
            "5",
        ],
    );
    let _ = std::fs::remove_file(&path);
    assert!(out.status.success(), "search-batch failed: {out:?}");
    let body = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 records, got {lines:?}");
    for line in lines {
        let json: serde_json::Value = serde_json::from_str(line).expect("jsonl");
        assert!(json["pattern"].is_string());
        assert!(json["matches"].is_array());
    }
}

#[test]
fn ontology_term_unknown_returns_error() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["ontology", "term", "SO:0000000000"]);
    assert!(
        !out.status.success(),
        "expected unknown-term failure: {out:?}"
    );
}

#[test]
fn replay_fails_for_unknown_id() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let bogus = "00000000-0000-0000-0000-000000000000";
    let out = cli(&url, &["jobs", "replay", bogus]);
    assert!(!out.status.success(), "expected failure: {out:?}");
}
