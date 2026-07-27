use sbol_db_search_eval::{
    suite_sha256, validate_suite, EvaluationConfig, EvaluationSuite,
    EVALUATION_SUITE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

const CORPUS: &[u8] = include_bytes!("../fixtures/contributor-smoke-v1-corpus.json");
const SUITE: &str = include_str!("../fixtures/contributor-smoke-v1.json");

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
