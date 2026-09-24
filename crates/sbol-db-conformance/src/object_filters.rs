//! Shared object-listing tests for each storage backend. Every identifier below
//! is synthetic fixture data, not a special case in the search implementation.

use sbol_db_core::{GraphId, SerializationFormat};
use sbol_db_storage::{ImportInput, ImportOverwrite, ListObjectsFilter, SbolStore};

const ROOT: &str = "https://example.org/object-filter-conformance/";
const COMPONENT: &str = "http://sbols.org/v3#Component";
const ROLE: &str = "https://example.org/roles/selected";
const OTHER_ROLE: &str = "https://example.org/roles/other";

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

fn expected_iris(suffixes: &[&str]) -> Vec<String> {
    suffixes
        .iter()
        .map(|suffix| format!("{ROOT}{suffix}"))
        .collect()
}

/// Run the IRI matching, filter composition, and graph-membership scenarios.
pub async fn object_filtering(store: &dyn SbolStore) {
    literal_iri_matching(store).await;
    combined_filters_before_pagination(store).await;
    shared_graph_membership(store).await;
}

async fn literal_iri_matching(store: &dyn SbolStore) {
    // Numeric prefixes make the expected order independent of letter case.
    let percent = "literal/01_ITEM%25";
    let underscore = "literal/02_item_tail";
    let wildcard_decoy = "literal/03_itemXtail";
    let uppercase_unicode = "literal/04_itemÄ";
    let lowercase_unicode = "literal/05_itemä";
    let suffixes = [
        percent,
        underscore,
        wildcard_decoy,
        uppercase_unicode,
        lowercase_unicode,
    ];
    let graph_id = import(
        store,
        suffixes
            .iter()
            .map(|suffix| component(suffix, ROLE))
            .collect(),
    )
    .await;
    let filter = ListObjectsFilter {
        graph_id: Some(graph_id),
        limit: 100,
        ..Default::default()
    };

    for (case, query, expected) in [
        (
            "ASCII case-insensitive matching",
            "ItEm",
            suffixes.as_slice(),
        ),
        ("literal percent", "%", &[percent][..]),
        ("no percent decoding", "%25", &[percent][..]),
        ("literal underscore", "_tail", &[underscore][..]),
        (
            "uppercase Unicode is preserved",
            "ITEMÄ",
            &[uppercase_unicode][..],
        ),
        (
            "lowercase Unicode is preserved",
            "ITEMä",
            &[lowercase_unicode][..],
        ),
        (
            "metadata is not an IRI match",
            "metadata-only-token",
            &[][..],
        ),
        ("absent substring", "absent", &[][..]),
        ("whitespace is significant", " item ", &[][..]),
        ("SQL syntax is literal input", "' OR 1=1 --", &[][..]),
        ("empty query is unrestricted", "", suffixes.as_slice()),
    ] {
        assert_eq!(
            iris(
                store,
                &ListObjectsFilter {
                    iri_contains: Some(query.to_owned()),
                    ..filter.clone()
                }
            )
            .await,
            expected_iris(expected),
            "{case}"
        );
    }
    assert_eq!(
        iris(store, &filter).await,
        expected_iris(&suffixes),
        "None is unrestricted"
    );
}

async fn combined_filters_before_pagination(store: &dyn SbolStore) {
    // Put excluded rows first so limiting before filtering cannot pass.
    let mut body = format!(
        "<{ROOT}combined/00_match_collection> a <http://sbols.org/v3#Collection> ; \
         <http://sbols.org/v3#hasNamespace> <{ROOT}> .\n"
    );
    body.push_str(&component("combined/01_match_wrong_role", OTHER_ROLE));
    body.push_str(&component("combined/02_other", ROLE));
    let matching = [
        "combined/03_match_first",
        "combined/04_MATCH_second",
        "combined/05_match_third",
    ];
    for suffix in matching {
        body.push_str(&component(suffix, ROLE));
    }
    let graph_id = import(store, body).await;
    import(store, component("combined/outside_match", ROLE)).await;
    let filter = ListObjectsFilter {
        sbol_class: Some(COMPONENT.to_owned()),
        role: Some(ROLE.to_owned()),
        graph_id: Some(graph_id),
        iri_contains: Some("match".to_owned()),
        limit: 2,
        ..Default::default()
    };
    let first = iris(store, &filter).await;
    assert_eq!(first, expected_iris(&matching[..2]));
    let second = iris(
        store,
        &ListObjectsFilter {
            after_iri: first.last().cloned(),
            ..filter.clone()
        },
    )
    .await;
    assert_eq!(second, expected_iris(&matching[2..]));
    assert!(iris(
        store,
        &ListObjectsFilter {
            after_iri: second.last().cloned(),
            ..filter.clone()
        }
    )
    .await
    .is_empty());

    // A cursor need not identify a matching (or even an existing) object.
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                after_iri: Some(format!("{ROOT}combined/02_z")),
                limit: 1,
                ..filter.clone()
            }
        )
        .await,
        expected_iris(&matching[..1])
    );
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                limit: 0,
                ..filter.clone()
            }
        )
        .await,
        expected_iris(&matching[..1])
    );
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                limit: 100,
                ..filter.clone()
            }
        )
        .await,
        expected_iris(&matching)
    );

    // Hydrate the same native summary whose class and role were filtered.
    let records = store
        .list_objects(&filter)
        .await
        .expect("filtered native records");
    for record in records {
        assert_eq!(record.sbol_class, COMPONENT);
        assert!(record.roles.iter().any(|role| role == ROLE));
        assert_eq!(record.name.as_deref(), Some("metadata-only-token"));
    }
}

async fn shared_graph_membership(store: &dyn SbolStore) {
    let shared = ["membership/a", "membership/b"];
    let body = shared
        .iter()
        .map(|suffix| component(suffix, ROLE))
        .collect::<String>();
    let first = import(store, body.clone()).await;
    let second = import(store, body).await;
    assert_ne!(first, second, "separate imports create separate graphs");
    let outside = import(store, component("membership/outside", ROLE)).await;
    let filter = ListObjectsFilter {
        iri_contains: Some(format!("{ROOT}membership/")),
        limit: 100,
        ..Default::default()
    };

    for graph_id in [first, second] {
        assert_eq!(
            iris(
                store,
                &ListObjectsFilter {
                    graph_id: Some(graph_id),
                    ..filter.clone()
                }
            )
            .await,
            expected_iris(&shared),
            "shared objects remain in both imported graphs"
        );
    }
    assert_eq!(
        iris(store, &filter).await,
        expected_iris(&[shared[0], shared[1], "membership/outside"]),
        "global listing deduplicates shared IRIs"
    );
    assert_eq!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(outside),
                ..filter.clone()
            }
        )
        .await,
        expected_iris(&["membership/outside"])
    );
    assert!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(GraphId::new()),
                ..filter.clone()
            }
        )
        .await
        .is_empty(),
        "an unknown graph must not become an unrestricted search"
    );

    // A reference to an object does not make it a subject in the referring graph.
    let reference_graph = "urn:sbol-db:object-filter-references";
    store
        .graph_store_write(
            reference_graph,
            &format!(
                "<urn:reference-holder> <urn:references> <{ROOT}{}> .",
                shared[0]
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
                ..filter.clone()
            }
        )
        .await
        .is_empty(),
        "being referenced does not establish subject membership"
    );

    assert!(store.delete_graph(first).await.expect("delete first graph"));
    assert!(
        iris(
            store,
            &ListObjectsFilter {
                graph_id: Some(first),
                ..filter.clone()
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
                ..filter
            }
        )
        .await,
        expected_iris(&shared),
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
        ..Default::default()
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
