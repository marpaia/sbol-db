use sbol_db_core::SerializationFormat;
use sbol_db_ha_sim::corpus::{CorpusSource, ImportGroup};
use sbol_db_ha_sim::{run_corpus_chaos, Corpus, CorpusDocument, CorpusManifest, ScenarioConfig};
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deterministic_fault_schedule_preserves_every_acknowledged_value() {
    let documents = (0..60)
        .map(|ordinal| {
            let relative_path = format!("SBOL3/synthetic-{ordinal:03}.ttl");
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
            id: "synthetic-ha-smoke".to_owned(),
            revision: "1".to_owned(),
            source: CorpusSource {
                repository: "fixture://sbol-db-ha-sim".to_owned(),
                commit: "fixture-1".to_owned(),
                selection: "deterministic synthetic smoke documents".to_owned(),
            },
            import_groups: vec![ImportGroup {
                path: "SBOL3".to_owned(),
                expected_imported_documents: 60,
                expected_parse_failures: 0,
            }],
            expected_imported_documents: 60,
        },
        root: std::path::PathBuf::from("fixture://sbol-db-ha-sim"),
        fingerprint: hex::encode(Sha256::digest(b"synthetic-ha-smoke-v1")),
        documents,
        groups: Vec::new(),
    };

    let report = run_corpus_chaos(
        &corpus,
        ScenarioConfig {
            seed: 0xfeed_beef,
            retry_every: 11,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.document_count, 60);
    assert_eq!(report.acknowledged_document_writes, 60);
    assert_eq!(report.final_state_sha256.len(), 64);
}
