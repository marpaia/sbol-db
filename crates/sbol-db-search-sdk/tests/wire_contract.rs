use sbol_db_search_sdk::{IndexGenerationSpec, SearchInput, SearchRequest, Total};

#[test]
fn structured_request_defaults_are_stable() {
    let request: SearchRequest = serde_json::from_value(serde_json::json!({
        "query": { "kind": "text", "text": "inducible promoter" }
    }))
    .expect("request");

    assert_eq!(
        request.query,
        SearchInput::Text {
            text: "inducible promoter".to_owned()
        }
    );
    assert_eq!(request.page.limit, 50);
    assert!(!request.options.explain);
    assert!(request.filters.graphs.is_empty());
}

#[test]
fn totals_do_not_imply_exactness() {
    assert_eq!(
        serde_json::to_value(Total::Exact(42)).expect("exact"),
        serde_json::json!({ "kind": "exact", "value": 42 })
    );
    assert_eq!(
        serde_json::to_value(Total::LowerBound(25)).expect("lower bound"),
        serde_json::json!({ "kind": "lower_bound", "value": 25 })
    );
    assert_eq!(
        serde_json::to_value(Total::Unknown).expect("unknown"),
        serde_json::json!({ "kind": "unknown" })
    );
}

#[test]
fn older_generation_specs_default_missing_embedding_provenance() {
    let spec: IndexGenerationSpec = serde_json::from_value(serde_json::json!({
        "artifact_id": "components",
        "generation": "legacy",
        "vector_name": "content",
        "dimension": 384,
        "distance": "cosine"
    }))
    .expect("legacy generation spec");

    assert!(spec.embedding.is_none());
}
