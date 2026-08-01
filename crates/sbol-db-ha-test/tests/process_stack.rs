use sbol_db_core::SerializationFormat;
use sbol_db_ha_sim::corpus::{CorpusSource, ImportGroup};
use sbol_db_ha_sim::{Corpus, CorpusDocument, CorpusManifest};
use sbol_db_ha_test::{run_process_chaos, ProcessChaosConfig};
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_processes_survive_failover_partition_snapshot_and_restart() {
    let documents = (0..60)
        .map(|ordinal| {
            let relative_path = format!("SBOL3/process-{ordinal:03}.ttl");
            let body = format!(
                "<https://example.test/{ordinal}> <https://example.test/value> \"{ordinal}\" ."
            );
            CorpusDocument {
                ordinal,
                relative_path,
                format: SerializationFormat::Turtle,
                sha256: hex::encode(Sha256::digest(body.as_bytes())),
                body,
                object_count: 1,
                triple_count: 1,
            }
        })
        .collect::<Vec<_>>();
    let corpus = Corpus {
        manifest: CorpusManifest {
            id: "synthetic-process-ha".to_owned(),
            revision: "1".to_owned(),
            source: CorpusSource {
                repository: "fixture://sbol-db-ha-test".to_owned(),
                commit: "fixture-process-1".to_owned(),
                selection: "deterministic process systems test".to_owned(),
            },
            import_groups: vec![ImportGroup {
                path: "SBOL3".to_owned(),
                expected_imported_documents: 60,
                expected_parse_failures: 0,
            }],
            expected_imported_documents: 60,
        },
        root: std::path::PathBuf::from("fixture://sbol-db-ha-test"),
        fingerprint: hex::encode(Sha256::digest(b"synthetic-process-ha-v1")),
        documents,
        groups: Vec::new(),
    };
    let artifacts = tempfile::tempdir().unwrap();
    let report = run_process_chaos(
        &corpus,
        ProcessChaosConfig {
            seed: 0xfeed_f00d,
            retry_every: 13,
            node_binary: env!("CARGO_BIN_EXE_sbol-db-ha-node").into(),
            artifact_root: artifacts.path().to_path_buf(),
        },
    )
    .await
    .unwrap();

    assert_eq!(report.document_count, 60);
    assert_eq!(report.acknowledged_document_writes, 60);
    assert_eq!(report.ambiguous_retries, 1);
    assert!(report.linearizability.valid);
    assert_eq!(report.linearizability.operations_checked, 48);
    assert_eq!(report.node_ids, vec![1, 2, 3]);
    assert_eq!(report.final_state_sha256.len(), 64);
    assert!(artifacts.path().join("history.jsonl").is_file());
    assert!(artifacts.path().join("checker.json").is_file());
}
