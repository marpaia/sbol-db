//! End-to-end smoke test that runs the compiled CLI against the live
//! Postgres at `DATABASE_URL`. Skips itself if the env var is unset.

use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;

fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

fn fixture_path() -> std::path::PathBuf {
    // Tests run from the workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("sbol-db-postgres/tests/fixtures/simple_component.ttl")
}

#[test]
fn import_get_export_round_trip() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set, skipping CLI smoke test");
        return;
    };

    // Migrate first.
    let migrate = Command::cargo_bin("sbol-db")
        .unwrap()
        .args(["migrate", "up"])
        .env("DATABASE_URL", &url)
        .output()
        .expect("run migrate");
    assert!(migrate.status.success(), "migrate failed: {migrate:?}");

    // Import.
    let import = Command::cargo_bin("sbol-db")
        .unwrap()
        .args(["import", fixture_path().to_str().unwrap()])
        .env("DATABASE_URL", &url)
        .output()
        .expect("run import");
    assert!(import.status.success(), "import failed: {import:?}");
    let import_json: serde_json::Value =
        serde_json::from_slice(&import.stdout).expect("import json");
    let obj_count = import_json["object_count"].as_u64().unwrap();
    assert!(obj_count >= 2, "expected >= 2 objects, got {obj_count}");

    // Get.
    let iri = "https://example.org/sbol-db/test/promoter_j23119";
    let get = Command::cargo_bin("sbol-db")
        .unwrap()
        .args(["get", iri])
        .env("DATABASE_URL", &url)
        .output()
        .expect("run get");
    assert!(get.status.success(), "get failed: {get:?}");
    let get_json: serde_json::Value = serde_json::from_slice(&get.stdout).expect("get json");
    assert_eq!(get_json["iri"], iri);

    // Export.
    let export = Command::cargo_bin("sbol-db")
        .unwrap()
        .args(["export", iri, "--format", "turtle"])
        .env("DATABASE_URL", &url)
        .output()
        .expect("run export");
    assert!(export.status.success(), "export failed: {export:?}");
    let turtle = String::from_utf8(export.stdout).unwrap();
    assert!(
        turtle.contains("Component"),
        "turtle should contain a Component: {turtle}"
    );
}
