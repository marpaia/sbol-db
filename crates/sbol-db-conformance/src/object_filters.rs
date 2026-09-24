//! The object-browser contract used by embedded clients such as GG Circuit.

use sbol_db_core::{GraphId, SerializationFormat};
use sbol_db_storage::{ImportInput, ImportOverwrite, ListObjectsFilter, SbolStore};

const ROOT: &str = "https://example.org/object-filter-conformance/";
const COMPONENT: &str = "http://sbols.org/v3#Component";
const ROLE: &str = "https://identifiers.org/SO:0000167";

fn component(suffix: &str, role: &str) -> String {
    format!(
        r#"<{ROOT}{suffix}> a <{COMPONENT}> ;
        <http://sbols.org/v3#hasNamespace> <{ROOT}> ;
        <http://sbols.org/v3#type> <https://identifiers.org/SBO:0000251> ;
        <http://sbols.org/v3#role> <{role}> ;
        <http://sbols.org/v3#name> "metadata-only-token" .
"#
    )
}

async fn import(store: &dyn SbolStore, body: String) -> GraphId {
    store
        .import_document(ImportInput {
            body,
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: None,
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import object-filter fixture")
        .graph_id
}

async fn iris(store: &dyn SbolStore, filter: &ListObjectsFilter) -> Vec<String> {
    store
        .list_objects(filter)
        .await
        .expect("list filtered objects")
        .into_iter()
        .map(|object| object.iri.into_inner())
        .collect()
}

/// IRI matching composes with graph, class, role, and cursor filters before
/// limiting. Reimporting shared objects must not hide their earlier graph.
pub async fn object_filtering(store: &dyn SbolStore) {
    let matching = [
        "03_GFP_alpha",
        "04_gfp_beta",
        "05_GFP%25_literal",
        "06_GFP_literal",
        "07_GFPÄ",
        "08_GFPä",
    ];
    let mut body = component("00_name_only", ROLE);
    body.push_str(&component(
        "01_GFP_wrong_role",
        "https://identifiers.org/SO:0000323",
    ));
    body.push_str(&format!(
        "<{ROOT}02_GFP_collection> a <http://sbols.org/v3#Collection> ; \
         <http://sbols.org/v3#hasNamespace> <{ROOT}> .\n"
    ));
    for suffix in matching {
        body.push_str(&component(suffix, ROLE));
    }
    let first = import(store, body.clone()).await;
    let second = import(store, body).await;
    assert_ne!(first, second, "separate imports create separate graphs");
    let outside = import(store, component("outside_GFP", ROLE)).await;
    let expected: Vec<_> = matching
        .iter()
        .map(|suffix| format!("{ROOT}{suffix}"))
        .collect();

    for graph_id in [first, second] {
        let filter = ListObjectsFilter {
            sbol_class: Some(COMPONENT.to_owned()),
            role: Some(ROLE.to_owned()),
            graph_id: Some(graph_id),
            iri_contains: Some("gFp".to_owned()),
            limit: 2,
            ..ListObjectsFilter::default()
        };
        let mut cursor = None;
        let mut found = Vec::new();
        for page_number in 0..=3 {
            let page = iris(
                store,
                &ListObjectsFilter {
                    after_iri: cursor.clone(),
                    ..filter.clone()
                },
            )
            .await;
            if page_number == 3 {
                assert!(page.is_empty(), "cursor past the last match is exhausted");
            } else {
                assert_eq!(
                    page.len(),
                    2,
                    "filter before limiting, with no duplicate IRIs: {graph_id:?}, page {page_number}"
                );
                cursor = page.last().cloned();
                found.extend(page);
            }
        }
        assert_eq!(found, expected, "complete, ordered, graph-scoped pages");

        let all = ListObjectsFilter {
            limit: 100,
            ..filter.clone()
        };
        assert_eq!(iris(store, &all).await, expected);
        // A cursor need not identify a matching (or even an existing) object.
        assert_eq!(
            iris(
                store,
                &ListObjectsFilter {
                    after_iri: Some(format!("{ROOT}02_z")),
                    limit: 1,
                    ..filter.clone()
                }
            )
            .await,
            vec![expected[0].clone()]
        );
        for (needle, indices) in [
            ("%", vec![2]),
            ("%25", vec![2]),
            ("_literal", vec![2, 3]),
            ("GFPÄ", vec![4]),
            ("gfpä", vec![5]),
            ("metadata-only-token", vec![]),
            ("no-such-iri", vec![]),
            (" GFP ", vec![]),
            ("' OR 1=1 --", vec![]),
        ] {
            assert_eq!(
                iris(
                    store,
                    &ListObjectsFilter {
                        iri_contains: Some(needle.to_owned()),
                        ..all.clone()
                    }
                )
                .await,
                indices
                    .into_iter()
                    .map(|i| expected[i].clone())
                    .collect::<Vec<_>>(),
                "literal IRI-only matching for {needle:?}"
            );
        }
        let unfiltered = ListObjectsFilter {
            graph_id: Some(graph_id),
            limit: 100,
            ..ListObjectsFilter::default()
        };
        let unfiltered_iris = iris(store, &unfiltered).await;
        assert_eq!(unfiltered_iris.len(), 9);
        assert_eq!(
            iris(
                store,
                &ListObjectsFilter {
                    iri_contains: Some(String::new()),
                    ..unfiltered
                }
            )
            .await,
            unfiltered_iris,
            "an empty query is unrestricted"
        );
        assert_eq!(
            iris(store, &ListObjectsFilter { limit: 0, ..all })
                .await
                .len(),
            1
        );
    }

    let global = ListObjectsFilter {
        iri_contains: Some(ROOT.to_owned()),
        limit: 100,
        ..ListObjectsFilter::default()
    };
    assert_eq!(
        iris(store, &global).await.len(),
        10,
        "global listing deduplicates shared IRIs"
    );
    assert!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(GraphId::new()),
                ..global.clone()
            }
        )
        .await
        .is_empty(),
        "an unknown graph must never become an unrestricted search"
    );
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(outside),
                ..global.clone()
            }
        )
        .await,
        vec![format!("{ROOT}outside_GFP")]
    );

    // References to an object are not subject membership in the referring graph.
    let reference_graph = "urn:sbol-db:object-filter-references";
    store
        .graph_store_write(
            reference_graph,
            &format!(
                "<urn:reference-holder> <urn:references> <{}> .",
                expected[0]
            ),
            SerializationFormat::NTriples,
            sbol_db_storage::GraphWriteMode::Merge,
        )
        .await
        .expect("write reference-only graph");
    let reference_id = store
        .catalog_graphs(&sbol_db_storage::NamedGraphQuery {
            text: Some(reference_graph.to_owned()),
            limit: 100,
            ..Default::default()
        })
        .await
        .expect("resolve reference graph")
        .items
        .into_iter()
        .find(|graph| graph.iri == reference_graph)
        .expect("registered reference graph")
        .id;
    assert!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(reference_id),
                ..global.clone()
            }
        )
        .await
        .is_empty(),
        "being referenced does not make an object a graph member"
    );

    assert!(store.delete_graph(first).await.expect("delete first graph"));
    assert!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(first),
                ..global.clone()
            }
        )
        .await
        .is_empty(),
        "deleted graphs have no matches"
    );
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(second),
                ..global
            }
        )
        .await
        .len(),
        9,
        "deleting the earlier import preserves the later import"
    );
}

/// Object listing must not inherit the catalog's smaller 500-item page cap.
pub async fn object_filtering_large_page(store: &dyn SbolStore) {
    let body = (0..505)
        .map(|i| component(&format!("large/{i:04}"), ROLE))
        .collect::<String>();
    let graph_id = import(store, body).await;
    let filter = ListObjectsFilter {
        graph_id: Some(graph_id),
        iri_contains: Some("/LARGE/".to_owned()),
        limit: 501,
        ..ListObjectsFilter::default()
    };
    let first = iris(store, &filter).await;
    assert_eq!(first.len(), 501, "object limit applies to matching objects");
    let second = iris(
        store,
        &ListObjectsFilter {
            after_iri: first.last().cloned(),
            ..filter
        },
    )
    .await;
    assert_eq!(second.len(), 4);
    assert!(second.iter().all(|iri| iri > first.last().unwrap()));
}
