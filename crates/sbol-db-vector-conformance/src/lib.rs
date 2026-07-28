//! Backend-neutral conformance scenarios for vector index implementations.
//!
//! Backend crates construct a fresh implementation and call [`run_all`]. The
//! scenarios use only [`sbol_db_search_sdk::VectorBackend`], so passing them
//! demonstrates observable contract behavior rather than implementation
//! similarity.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sbol_db_search_sdk::{
    DistanceMetric, DocumentId, FilterCapability, GenerationHandle, IndexGenerationSpec,
    SparseVector, VectorBackend, VectorChange, VectorError, VectorFilter, VectorQuery, VectorValue,
};
use serde_json::json;

const ARTIFACT: &str = "vector-conformance";
const VECTOR_NAME: &str = "content";

/// Run the portable lifecycle, validation, filtering, and paging contract.
///
/// The backend must be freshly constructed and must not already contain the
/// [`ARTIFACT`] artifact. The final state intentionally retains generation
/// `one` as active so the scenario can prove that active generations cannot be
/// deleted.
pub async fn run_all(backend: Arc<dyn VectorBackend>) {
    descriptor_contract(backend.as_ref());
    rejected_batch_is_atomic(backend.as_ref()).await;

    let first = build_ready_generation(
        backend.as_ref(),
        "one",
        vec![
            upsert("a", [1.0, 0.0], "public", 2024, "alpha"),
            upsert("b", [0.8, 0.2], "public", 2026, "beta"),
            upsert("c", [-1.0, 0.0], "private", 2026, "gamma"),
            upsert("d", [0.0, 1.0], "archive", 2020, "alpha"),
        ],
    )
    .await;

    snapshot_contract(backend.as_ref(), &first).await;
    backend
        .activate(&first)
        .await
        .expect("activate first generation");
    query_and_filter_contract(backend.as_ref()).await;
    validation_contract(backend.as_ref()).await;

    let second = build_ready_generation(
        backend.as_ref(),
        "two",
        vec![upsert("replacement", [1.0, 0.0], "public", 2030, "new")],
    )
    .await;
    backend
        .activate(&second)
        .await
        .expect("activate replacement generation");
    assert_eq!(
        ids(&query(backend.as_ref(), None, 10, None).await),
        ["replacement"]
    );
    assert!(
        backend.delete_generation(&second).await.is_err(),
        "the active generation must not be deletable"
    );

    backend
        .activate(&first)
        .await
        .expect("roll back to first generation");
    assert_eq!(
        ids(&query(backend.as_ref(), None, 10, None).await),
        ["a", "b", "d", "c"],
        "rollback must atomically restore the prior generation"
    );
    backend
        .delete_generation(&second)
        .await
        .expect("delete inactive replacement generation");
    assert!(
        backend.delete_generation(&first).await.is_err(),
        "the rolled-back active generation must remain protected"
    );

    let generations = backend
        .generations(ARTIFACT)
        .await
        .expect("list final generations");
    assert_eq!(generations.len(), 1);
    assert!(generations[0].active);
    assert_eq!(generations[0].handle.generation, "one");
    assert_eq!(generations[0].vector_count, 4);
}

fn descriptor_contract(backend: &dyn VectorBackend) {
    let descriptor = backend.descriptor();
    assert!(
        !descriptor.id.trim().is_empty(),
        "backend id must be non-empty"
    );
    assert!(
        !descriptor.kind.trim().is_empty(),
        "backend kind must be non-empty"
    );
    assert!(
        descriptor.capabilities.dense,
        "conforming backends must support dense vectors"
    );
    assert!(
        descriptor
            .capabilities
            .distances
            .contains(&DistanceMetric::Cosine),
        "conforming backends must support cosine distance"
    );
    assert_eq!(
        descriptor.capabilities.filter_execution,
        FilterCapability::Native,
        "authorization filters must execute inside the backend"
    );
    assert!(descriptor.capabilities.atomic_activation);
    assert!(descriptor.capabilities.deletes);
}

async fn rejected_batch_is_atomic(backend: &dyn VectorBackend) {
    let handle = backend
        .create_generation(spec("rejected"))
        .await
        .expect("create rejection-test generation");
    let result = backend
        .apply(
            &handle,
            vec![
                upsert("valid", [1.0, 0.0], "public", 2026, "valid"),
                upsert("invalid", [f32::NAN, 0.0], "public", 2026, "invalid"),
            ],
        )
        .await;
    assert!(result.is_err(), "a non-finite vector must reject the batch");
    let status = backend
        .generations(ARTIFACT)
        .await
        .expect("inspect rejected generation")
        .into_iter()
        .find(|status| status.handle.generation == "rejected")
        .expect("rejected generation exists");
    assert_eq!(
        status.vector_count, 0,
        "a rejected batch must not partially mutate state"
    );
    backend
        .delete_generation(&handle)
        .await
        .expect("delete rejected inactive generation");
}

async fn build_ready_generation(
    backend: &dyn VectorBackend,
    generation: &str,
    changes: Vec<VectorChange>,
) -> GenerationHandle {
    let handle = backend
        .create_generation(spec(generation))
        .await
        .expect("create generation");
    let expected = changes.len();
    let receipt = backend
        .apply(&handle, changes)
        .await
        .expect("apply generation records");
    assert_eq!(receipt.applied, expected);
    backend.flush(&handle).await.expect("flush generation");
    backend
        .optimize(&handle)
        .await
        .expect("optimize generation");
    handle
}

async fn snapshot_contract(backend: &dyn VectorBackend, generation: &GenerationHandle) {
    let result = backend.snapshot(generation).await;
    if backend.descriptor().capabilities.snapshots {
        let snapshot = result.expect("backend advertises snapshots");
        assert!(!snapshot.locator.trim().is_empty());
    } else {
        assert!(
            matches!(result, Err(VectorError::Unsupported(_))),
            "a backend that does not advertise snapshots must reject the operation"
        );
    }
}

async fn query_and_filter_contract(backend: &dyn VectorBackend) {
    let unfiltered = query(backend, None, 10, None).await;
    assert_eq!(ids(&unfiltered), ["a", "b", "d", "c"]);
    assert!(unfiltered.windows(2).all(|pair| pair[0].1 >= pair[1].1));

    assert_ids(
        backend,
        VectorFilter::Match {
            field: "graph".to_owned(),
            value: json!("public"),
        },
        ["a", "b"],
    )
    .await;
    assert_ids(
        backend,
        VectorFilter::Any {
            field: "graph".to_owned(),
            values: vec![json!("public"), json!("archive")],
        },
        ["a", "b", "d"],
    )
    .await;
    assert_ids(
        backend,
        VectorFilter::Range {
            field: "metadata.year".to_owned(),
            gte: Some(2025.0),
            lte: None,
        },
        ["b", "c"],
    )
    .await;
    assert_ids(
        backend,
        VectorFilter::And {
            clauses: vec![
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("public"),
                },
                VectorFilter::Range {
                    field: "metadata.year".to_owned(),
                    gte: Some(2025.0),
                    lte: None,
                },
            ],
        },
        ["b"],
    )
    .await;
    assert_ids(
        backend,
        VectorFilter::Or {
            clauses: vec![
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("private"),
                },
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("archive"),
                },
            ],
        },
        ["c", "d"],
    )
    .await;
    assert_ids(
        backend,
        VectorFilter::Not {
            clause: Box::new(VectorFilter::Match {
                field: "graph".to_owned(),
                value: json!("private"),
            }),
        },
        ["a", "b", "d"],
    )
    .await;

    let first = query(backend, None, 1, None).await;
    assert_eq!(first.len(), 1);
    let cursor = first[0].2.clone().expect("first page has a cursor");
    let second = query(backend, None, 1, Some(cursor)).await;
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].0, "a");
    assert_eq!(second[0].0, "b");

    let thresholded = backend
        .query(VectorQuery {
            score_threshold: Some(0.9999),
            ..base_query(None, 10, None)
        })
        .await
        .expect("threshold query");
    assert_eq!(
        thresholded
            .items
            .iter()
            .map(|hit| short_id(&hit.document_id))
            .collect::<Vec<_>>(),
        ["a".to_owned()]
    );
}

async fn validation_contract(backend: &dyn VectorBackend) {
    for invalid in [
        VectorQuery {
            limit: 0,
            ..base_query(None, 10, None)
        },
        VectorQuery {
            vector: VectorValue::Dense(vec![1.0]),
            ..base_query(None, 10, None)
        },
        VectorQuery {
            vector: VectorValue::Dense(vec![f32::NAN, 0.0]),
            ..base_query(None, 10, None)
        },
        VectorQuery {
            score_threshold: Some(f32::NAN),
            ..base_query(None, 10, None)
        },
        VectorQuery {
            vector: VectorValue::Sparse(SparseVector {
                indices: vec![0],
                values: vec![1.0],
            }),
            ..base_query(None, 10, None)
        },
    ] {
        assert!(backend.query(invalid).await.is_err());
    }

    let mut unsupported = spec("unsupported-distance");
    unsupported.distance = DistanceMetric::Hamming;
    assert!(backend.create_generation(unsupported).await.is_err());
}

async fn assert_ids<const N: usize>(
    backend: &dyn VectorBackend,
    filter: VectorFilter,
    expected: [&str; N],
) {
    let actual = query(backend, Some(filter), 10, None)
        .await
        .into_iter()
        .map(|item| item.0)
        .collect::<BTreeSet<_>>();
    let expected = expected
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

async fn query(
    backend: &dyn VectorBackend,
    filter: Option<VectorFilter>,
    limit: usize,
    cursor: Option<String>,
) -> Vec<(String, f32, Option<String>)> {
    let page = backend
        .query(base_query(filter, limit, cursor))
        .await
        .expect("vector query");
    let next_cursor = page.next_cursor;
    page.items
        .into_iter()
        .enumerate()
        .map(|(index, hit)| {
            (
                short_id(&hit.document_id),
                hit.score,
                (index == 0).then(|| next_cursor.clone()).flatten(),
            )
        })
        .collect()
}

fn base_query(filter: Option<VectorFilter>, limit: usize, cursor: Option<String>) -> VectorQuery {
    VectorQuery {
        index: ARTIFACT.to_owned(),
        vector_name: VECTOR_NAME.to_owned(),
        vector: VectorValue::Dense(vec![1.0, 0.0]),
        filter,
        limit,
        cursor,
        score_threshold: None,
        parameters: BTreeMap::new(),
    }
}

fn spec(generation: &str) -> IndexGenerationSpec {
    IndexGenerationSpec {
        artifact_id: ARTIFACT.to_owned(),
        generation: generation.to_owned(),
        vector_name: VECTOR_NAME.to_owned(),
        dimension: 2,
        distance: DistanceMetric::Cosine,
        parameters: BTreeMap::new(),
    }
}

fn upsert(id: &str, vector: [f32; 2], graph: &str, year: u64, group: &str) -> VectorChange {
    VectorChange::Upsert {
        document_id: document_id(id),
        vectors: BTreeMap::from([(VECTOR_NAME.to_owned(), VectorValue::Dense(vector.to_vec()))]),
        payload: BTreeMap::from([
            ("graph".to_owned(), json!(graph)),
            ("group".to_owned(), json!(group)),
            ("metadata".to_owned(), json!({"year": year})),
        ]),
    }
}

fn document_id(id: &str) -> DocumentId {
    DocumentId(format!("https://example.test/{id}"))
}

fn short_id(id: &DocumentId) -> String {
    id.0.rsplit('/')
        .next()
        .expect("test document id")
        .to_owned()
}

fn ids<const N: usize>(items: &[(String, f32, Option<String>)]) -> [&str; N] {
    items
        .iter()
        .map(|item| item.0.as_str())
        .collect::<Vec<_>>()
        .try_into()
        .expect("expected result count")
}
