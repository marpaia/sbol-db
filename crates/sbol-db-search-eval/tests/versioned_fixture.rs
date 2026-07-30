use sbol_db_search_eval::{
    suite_sha256, validate_suite, EvaluationConfig, EvaluationSuite,
    EVALUATION_SUITE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

const CORPUS: &[u8] = include_bytes!("../fixtures/contributor-smoke-v1-corpus.json");
const SUITE: &str = include_str!("../fixtures/contributor-smoke-v1.json");
const SEMANTIC_CORPUS: &[u8] = include_bytes!("../fixtures/sbol-semantic-release-v1-corpus.json");
const SEMANTIC_SUITE: &str = include_str!("../fixtures/sbol-semantic-release-v1.json");

#[test]
fn checked_in_fixture_has_valid_content_and_immutable_provenance() {
    let suite: EvaluationSuite = serde_json::from_str(SUITE).unwrap();

    validate_suite(
        &suite,
        &EvaluationConfig {
            cutoffs: vec![1, 3, 5],
        },
    )
    .unwrap();
    assert_eq!(suite.schema_version, EVALUATION_SUITE_SCHEMA_VERSION);
    assert_eq!(suite.cases.len(), 6);
    assert_eq!(
        format!("{:x}", Sha256::digest(CORPUS)),
        suite.provenance.corpus_sha256
    );
    assert_eq!(
        suite_sha256(&suite).unwrap(),
        "ec28bd1ec0885b1d7c0cf4eeec2bd3648a655207edb49695802dbbb763d1b76c"
    );
}

#[test]
fn semantic_release_fixture_has_checked_corpus_and_explicit_scope() {
    let suite: EvaluationSuite = serde_json::from_str(SEMANTIC_SUITE).unwrap();

    validate_suite(
        &suite,
        &EvaluationConfig {
            cutoffs: vec![1, 3],
        },
    )
    .unwrap();
    assert_eq!(suite.id, "sbol-db-semantic-release");
    assert_eq!(suite.revision, "1");
    assert_eq!(suite.cases.len(), 8);
    assert_eq!(
        format!("{:x}", Sha256::digest(SEMANTIC_CORPUS)),
        suite.provenance.corpus_sha256
    );
    assert!(suite.provenance.judgments_method.contains("synthetic"));
    assert_eq!(
        suite_sha256(&suite).unwrap(),
        "848e271fe076a55d32b51ec09dbf6a41251cb7311cf004cb0a9b09820687ede0"
    );
}
