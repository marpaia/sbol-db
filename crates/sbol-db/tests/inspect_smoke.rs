//! Smoke tests for the DB-touching read-only surfaces: `sbol-db inspect`,
//! `sbol-db db doctor`, `sbol-db graph {list,show,delete}`, and the
//! `sbol-db query explain` parse-only path. Each skips when
//! `DATABASE_URL` is unset.

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

fn cli_with_stdin(url: &str, args: &[&str], stdin: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::cargo_bin("sbol-db")
        .unwrap()
        .args(args)
        .env("DATABASE_URL", url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sbol-db");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().expect("wait sbol-db")
}

#[test]
fn inspect_size_reports_bytes() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["inspect", "size"]);
    assert!(out.status.success(), "size failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["total_bytes"].as_i64().unwrap() > 0);
    assert!(json["database"].is_string());
}

#[test]
fn inspect_tables_returns_a_list() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["inspect", "tables", "--limit", "10"]);
    assert!(out.status.success(), "tables failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.is_array(), "expected an array, got: {json}");
}

#[test]
fn inspect_table_reports_schema_for_known_table() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["inspect", "table", "sbol_objects"]);
    assert!(out.status.success(), "table inspect failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "sbol_objects");
    assert!(!json["columns"].as_array().unwrap().is_empty());
}

#[test]
fn inspect_slow_queries_is_resilient_to_missing_extension() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["inspect", "slow-queries"]);
    assert!(out.status.success(), "slow-queries failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.is_array() || json["ok"].is_boolean());
}

#[test]
fn inspect_config_prints_effective_server_config() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let out = cli(&url, &["inspect", "config"]);
    assert!(out.status.success(), "inspect config failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["max_body_bytes"].is_number());
    assert!(json["request_timeout_secs"].is_number());
}

#[test]
fn graph_list_returns_array() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(&url, &["graph", "list", "--limit", "5"]);
    assert!(out.status.success(), "graph list failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.is_array(), "expected array, got {json}");
}

#[test]
fn graph_show_round_trips_with_list() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("sbol-db-postgres/tests/fixtures/simple_component.ttl");
    let _ = cli(
        &url,
        &[
            "graph",
            "import",
            fixture.to_str().unwrap(),
            "--skip-existing",
        ],
    );
    let listed = cli(&url, &["graph", "list", "--limit", "1"]);
    assert!(listed.status.success(), "list failed: {listed:?}");
    let arr: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let id = arr[0]["id"].as_str().expect("id");
    let shown = cli(&url, &["graph", "show", id]);
    assert!(shown.status.success(), "show failed: {shown:?}");
    let one: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(one["id"], id);
}

#[test]
fn graph_delete_without_tty_requires_yes_flag() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let bogus = "00000000-0000-0000-0000-000000000000";
    let out = cli(&url, &["graph", "delete", bogus]);
    assert!(!out.status.success(), "expected failure: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--yes") || stderr.contains("not a TTY"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn db_doctor_reports_each_check() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let _ = cli(&url, &["db", "migrate"]);
    let out = cli(
        &url,
        &["db", "doctor", "--require-ontologies", "", "--json"],
    );
    assert!(
        out.status.success(),
        "doctor exited non-zero: {} stderr=[{}]",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], true);
    let names: Vec<String> = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.iter().any(|n| n == "database"));
    assert!(names.iter().any(|n| n == "migrations"));
    assert!(names.iter().any(|n| n == "worker_registry"));
    assert!(names.iter().any(|n| n == "queue_health"));
}

#[test]
fn query_explain_parses_a_select_query() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let out = cli_with_stdin(
        &url,
        &["query", "explain", "-"],
        "SELECT ?s WHERE { ?s ?p ?o }",
    );
    assert!(out.status.success(), "explain failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["form"], "Select");
    assert!(json["query_size_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn query_explain_rejects_update_query() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping");
        return;
    };
    let out = cli_with_stdin(
        &url,
        &["query", "explain", "-"],
        "INSERT DATA { <a:b> <a:b> <a:b> }",
    );
    assert!(!out.status.success(), "expected rejection, got {out:?}");
}
