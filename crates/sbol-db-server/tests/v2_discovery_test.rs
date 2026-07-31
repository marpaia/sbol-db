//! Native discovery contract over a production-shaped SQLite router.
//!
//! These tests prove that combined biological/ownership/date facets are owned
//! by the application facade, V2 totals and paging are deterministic, visible
//! facet counts are exact, malformed values retain the V2 error envelope, and
//! the normalized type journey agrees with the compatibility search surface.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_search::ranked_text::{IndexedPart, RankedTextIndex};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{GraphWriteMode, ImportInput, ImportOverwrite};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";
const COMPONENT: &str = "http://sbols.org/v2#ComponentDefinition";
const SEQUENCE: &str = "http://sbols.org/v2#Sequence";
const PROMOTER: &str = "http://identifiers.org/so/SO:0000167";
const CDS: &str = "http://identifiers.org/so/SO:0000316";
const COLLECTION: &str = "http://example.org/public/designs/1";
const OWNER: &str = "http://example.org/user/cu-boulder";
const ALPHA: &str = "http://example.org/public/alpha/1";
const BETA: &str = "http://example.org/public/beta/1";
const GAMMA: &str = "http://example.org/public/gamma/1";
const NATIVE_COMPONENT: &str = "http://example.org/native/promoter";
const NATIVE_FEATURE: &str = "http://example.org/native/promoter-feature";
const NATIVE_RANGE: &str = "http://example.org/native/promoter-range";
const NATIVE_SEQUENCE: &str = "http://example.org/native/promoter-sequence";
const NATIVE_COMPONENT_CLASS: &str = "http://sbols.org/v3#Component";
const NATIVE_SEQUENCE_CLASS: &str = "http://sbols.org/v3#Sequence";
const NATIVE_ROLE: &str = "https://identifiers.org/SO:0000167";

async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("discovery.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("migrations");

    backend
        .store
        .graph_store_write(
            PUBLIC_GRAPH,
            &corpus(),
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed corpus");

    let text_index = Arc::new(RankedTextIndex::in_ram().expect("text index"));
    text_index
        .rebuild([
            indexed(ALPHA, "alpha", "Alpha promoter", COMPONENT, "promoter"),
            indexed(BETA, "beta", "Beta promoter", COMPONENT, "promoter"),
            indexed(GAMMA, "gamma", "Gamma coding sequence", SEQUENCE, "coding"),
            indexed(
                COLLECTION,
                "designs",
                "CU Boulder designs",
                "http://sbols.org/v2#Collection",
                "collection",
            ),
        ])
        .expect("rebuild index");

    (app_router(&backend, text_index), dir)
}

async fn native_app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("native-discovery.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("migrations");
    backend
        .store
        .import_document(ImportInput {
            body: native_corpus().to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: None,
            document_iri: Some(IriString::new(PUBLIC_GRAPH.to_owned()).expect("document IRI")),
            created_by: None,
            name: None,
            description: None,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import native corpus");

    let text_index = Arc::new(RankedTextIndex::in_ram().expect("text index"));
    (app_router(&backend, text_index), dir)
}

fn app_router(backend: &Backend, text_index: Arc<RankedTextIndex>) -> axum::Router {
    let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let services = AppServices::from_backend(&backend).with_text_search(text_index);
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: Arc::new(services),
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
        jobs: backend.jobs.clone(),
        lab: backend.lab.clone(),
        config: config.clone(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    router(state, config)
}

fn indexed(
    subject: &str,
    display_id: &str,
    name: &str,
    object_type: &str,
    keywords: &str,
) -> IndexedPart {
    IndexedPart {
        subject: subject.to_owned(),
        graph: PUBLIC_GRAPH.to_owned(),
        display_id: Some(display_id.to_owned()),
        name: Some(name.to_owned()),
        description: None,
        version: Some("1".to_owned()),
        type_iris: vec![object_type.to_owned()],
        keywords: keywords.to_owned(),
        pagerank: 1.0,
    }
}

fn corpus() -> String {
    format!(
        r#"
<{COLLECTION}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#Collection> .
<{COLLECTION}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <{COLLECTION}> .
<{COLLECTION}> <http://sbols.org/v2#displayId> "designs" .
<{COLLECTION}> <http://purl.org/dc/terms/title> "CU Boulder designs" .
<{COLLECTION}> <http://sbols.org/v2#member> <{ALPHA}> .
<{COLLECTION}> <http://sbols.org/v2#member> <{BETA}> .

<{ALPHA}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{COMPONENT}> .
<{ALPHA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <{ALPHA}> .
<{ALPHA}> <http://sbols.org/v2#displayId> "alpha" .
<{ALPHA}> <http://sbols.org/v2#version> "1" .
<{ALPHA}> <http://purl.org/dc/terms/title> "Alpha promoter" .
<{ALPHA}> <http://purl.org/dc/terms/description> "A constitutive promoter" .
<{ALPHA}> <http://sbols.org/v2#role> <{PROMOTER}> .
<{ALPHA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy> <{OWNER}> .
<{ALPHA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableProvenance> "Built in the Boulder teaching lab" .
<{ALPHA}> <http://purl.org/dc/terms/created> "2026-01-02T10:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<{ALPHA}> <http://purl.org/dc/terms/modified> "2026-02-02T10:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .

<{BETA}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{COMPONENT}> .
<{BETA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <{BETA}> .
<{BETA}> <http://sbols.org/v2#displayId> "beta" .
<{BETA}> <http://sbols.org/v2#version> "1" .
<{BETA}> <http://purl.org/dc/terms/title> "Beta promoter" .
<{BETA}> <http://sbols.org/v2#role> <{PROMOTER}> .
<{BETA}> <http://sbols.org/v2#sequenceAnnotation> <http://example.org/public/beta/feature> .
<{BETA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy> <{OWNER}> .
<{BETA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableProvenance> "Built in the Boulder teaching lab" .
<{BETA}> <http://purl.org/dc/terms/created> "2026-03-03T10:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<{BETA}> <http://purl.org/dc/terms/modified> "2026-04-04T10:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .

<http://example.org/public/beta/feature> <http://sbols.org/v2#displayId> "unplaced-feature" .

<{GAMMA}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{SEQUENCE}> .
<{GAMMA}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <{GAMMA}> .
<{GAMMA}> <http://sbols.org/v2#displayId> "gamma" .
<{GAMMA}> <http://purl.org/dc/terms/title> "Gamma coding sequence" .
<{GAMMA}> <http://sbols.org/v2#role> <{CDS}> .
"#
    )
}

fn native_corpus() -> &'static str {
    r#"
@prefix sbol: <http://sbols.org/v3#> .
@prefix SBO: <https://identifiers.org/SBO:> .
@prefix SO: <https://identifiers.org/SO:> .
@prefix EDAM: <https://identifiers.org/edam:> .

<http://example.org/native/promoter>
    a sbol:Component ;
    sbol:displayId "promoter" ;
    sbol:name "Native promoter" ;
    sbol:description "Imported through the native SBOL DB document API" ;
    sbol:hasNamespace <http://example.org/native> ;
    sbol:type SBO:0000251 ;
    sbol:role SO:0000167 ;
    sbol:hasSequence <http://example.org/native/promoter-sequence> ;
    sbol:hasFeature <http://example.org/native/promoter-feature> .

<http://example.org/native/promoter-feature>
    a sbol:SequenceFeature ;
    sbol:displayId "promoter-feature" ;
    sbol:hasNamespace <http://example.org/native> ;
    sbol:role SO:0000167 ;
    sbol:hasLocation <http://example.org/native/promoter-range> .

<http://example.org/native/promoter-range>
    a sbol:Range ;
    sbol:displayId "promoter-range" ;
    sbol:hasNamespace <http://example.org/native> ;
    sbol:start 1 ;
    sbol:end 35 ;
    sbol:orientation sbol:inline .

<http://example.org/native/promoter-sequence>
    a sbol:Sequence ;
    sbol:displayId "promoter-sequence" ;
    sbol:hasNamespace <http://example.org/native> ;
    sbol:elements "ttgacagctagctcagtcctaggtataatgctagc" ;
    sbol:encoding EDAM:format_1207 .
"#
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "expected JSON from {uri} ({status}), got {}: {error}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, value)
}

fn query(parameters: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(parameters).expect("query encoding")
}

fn encode_iri(iri: &str) -> String {
    iri.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn uris(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["uri"].as_str().expect("uri").to_owned())
        .collect()
}

#[tokio::test]
async fn combined_facets_sort_and_page_have_exact_stable_totals() {
    let (app, _dir) = app().await;
    let parameters = query(&[
        ("type", COMPONENT),
        ("role", PROMOTER),
        ("collection", COLLECTION),
        ("owner", OWNER),
        ("provenance", "BOULDER"),
        ("created_after", "2026-01-01"),
        ("created_before", "2026-12-31"),
        ("sort", "name"),
        ("direction", "asc"),
        ("limit", "1"),
    ]);

    let (status, first) = get(&app, &format!("/api/v2/search?{parameters}")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["total"], 2);
    assert_eq!(first["sort"], "name");
    assert_eq!(first["direction"], "asc");
    assert_eq!(uris(&first), vec![ALPHA]);

    let second_parameters = format!("{parameters}&offset=1");
    let (status, second) = get(&app, &format!("/api/v2/search?{second_parameters}")).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["total"], 2);
    assert_eq!(uris(&second), vec![BETA]);

    let all: BTreeSet<_> = uris(&first).into_iter().chain(uris(&second)).collect();
    assert_eq!(all, BTreeSet::from([ALPHA.to_owned(), BETA.to_owned()]));
}

#[tokio::test]
async fn text_and_role_intersect_without_losing_metadata() {
    let (app, _dir) = app().await;
    let parameters = query(&[("q", "promoter"), ("role", PROMOTER)]);
    let (status, body) = get(&app, &format!("/api/v2/search?{parameters}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(
        uris(&body).into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([ALPHA.to_owned(), BETA.to_owned(),])
    );
    assert!(body["items"][0]["score"].as_f64().is_some());
    assert_eq!(body["items"][0]["roles"][0], PROMOTER);
    assert_eq!(body["items"][0]["owners"][0], OWNER);
}

#[tokio::test]
async fn visible_facets_report_exact_type_and_role_counts() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/api/v2/search/facets").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let component = body["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|facet| facet["iri"] == COMPONENT)
        .expect("component facet");
    assert_eq!(component["count"], 2);
    assert_eq!(component["label"], "ComponentDefinition");

    let promoter = body["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|facet| facet["iri"] == PROMOTER)
        .expect("promoter facet");
    assert_eq!(promoter["count"], 2);
    assert_eq!(promoter["label"], "SO:0000167");
}

#[tokio::test]
async fn native_imports_without_compatibility_markers_are_discoverable() {
    let (app, _dir) = native_app().await;

    let (status, browse) = get(&app, "/api/v2/search?sort=iri").await;
    assert_eq!(status, StatusCode::OK, "{browse}");
    assert_eq!(browse["total"], 4);
    assert_eq!(
        uris(&browse),
        vec![
            NATIVE_COMPONENT.to_owned(),
            NATIVE_FEATURE.to_owned(),
            NATIVE_RANGE.to_owned(),
            NATIVE_SEQUENCE.to_owned()
        ]
    );

    let parameters = query(&[("type", NATIVE_COMPONENT_CLASS)]);
    let (status, components) = get(&app, &format!("/api/v2/search?{parameters}")).await;
    assert_eq!(status, StatusCode::OK, "{components}");
    assert_eq!(components["total"], 1);
    assert_eq!(uris(&components), vec![NATIVE_COMPONENT]);
    assert_eq!(components["items"][0]["roles"][0], NATIVE_ROLE);

    let (status, facets) = get(&app, "/api/v2/search/facets").await;
    assert_eq!(status, StatusCode::OK, "{facets}");
    assert_eq!(
        facet_count(&facets["types"], NATIVE_COMPONENT_CLASS),
        Some(1)
    );
    assert_eq!(
        facet_count(&facets["types"], NATIVE_SEQUENCE_CLASS),
        Some(1)
    );
    assert_eq!(facet_count(&facets["roles"], NATIVE_ROLE), Some(2));
}

#[tokio::test]
async fn details_normalize_identity_provenance_and_inverse_collections() {
    let (app, _dir) = app().await;
    let path = format!("/api/v2/objects/{}/details", encode_iri(ALPHA));
    let (status, details) = get(&app, &path).await;

    assert_eq!(status, StatusCode::OK, "{details}");
    assert_eq!(details["iri"], ALPHA);
    assert_eq!(details["display_id"], "alpha");
    assert_eq!(details["object_type"], COMPONENT);
    assert_eq!(details["source_graph"], PUBLIC_GRAPH);
    assert_eq!(details["visibility"], "public");
    assert_eq!(details["owners"][0], OWNER);
    assert_eq!(
        details["provenance"]["mutable_source"][0],
        "Built in the Boulder teaching lab"
    );
    assert_eq!(details["collections"]["state"], "available");
    assert_eq!(details["collections"]["items"][0]["uri"], COLLECTION);
    assert_eq!(details["sequences"]["state"], "empty");
    assert_eq!(details["visualization"]["state"], "empty");
    assert_eq!(details["members"]["state"], "unsupported");
    assert!(details["properties"].as_array().unwrap().len() >= 8);

    let beta_path = format!("/api/v2/objects/{}/details", encode_iri(BETA));
    let (status, beta) = get(&app, &beta_path).await;
    assert_eq!(status, StatusCode::OK, "{beta}");
    assert_eq!(beta["visualization"]["state"], "partial");
    assert_eq!(beta["visualization"]["features"][0]["start"], Value::Null);
    assert_eq!(beta["visualization"]["features"][0]["glyph"], "unspecified");
}

#[tokio::test]
async fn native_details_expand_logical_public_scope_and_expose_sequence_content() {
    let (app, _dir) = native_app().await;

    let component_path = format!("/api/v2/objects/{}/details", encode_iri(NATIVE_COMPONENT));
    let (status, component) = get(&app, &component_path).await;
    assert_eq!(status, StatusCode::OK, "{component}");
    assert_eq!(component["source_graph"], PUBLIC_GRAPH);
    assert_eq!(component["visibility"], "public");
    assert_eq!(component["sequences"]["state"], "available");
    assert_eq!(component["sequences"]["items"][0]["uri"], NATIVE_SEQUENCE);
    assert_eq!(component["visualization"]["state"], "available");
    assert_eq!(component["visualization"]["sequence_length"], 35);
    assert_eq!(component["visualization"]["features"][0]["start"], 1);
    assert_eq!(component["visualization"]["features"][0]["end"], 35);
    assert_eq!(
        component["visualization"]["features"][0]["glyph"],
        "promoter"
    );

    let sequence_path = format!("/api/v2/objects/{}/details", encode_iri(NATIVE_SEQUENCE));
    let (status, sequence) = get(&app, &sequence_path).await;
    assert_eq!(status, StatusCode::OK, "{sequence}");
    assert_eq!(sequence["sequence_content"]["state"], "available");
    assert_eq!(sequence["visualization"]["state"], "unsupported");
    assert_eq!(sequence["sequence_content"]["length"], 35);
    assert_eq!(
        sequence["sequence_content"]["elements"],
        "ttgacagctagctcagtcctaggtataatgctagc"
    );
}

#[tokio::test]
async fn malformed_discovery_values_use_the_v2_error_envelope() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/api/v2/search?created_after=07-30-2026").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_input");
    assert_eq!(body["error"]["status"], 400);
}

#[tokio::test]
async fn normalized_type_discovery_matches_the_v1_member_set() {
    let (app, _dir) = app().await;
    let parameters = query(&[("type", COMPONENT), ("sort", "iri")]);
    let (status, native) = get(&app, &format!("/api/v2/search?{parameters}")).await;
    assert_eq!(status, StatusCode::OK, "{native}");

    let (status, classic) = get(&app, "/search/objectType=ComponentDefinition&").await;
    assert_eq!(status, StatusCode::OK, "{classic}");
    let classic_uris: BTreeSet<_> = classic
        .as_array()
        .expect("classic array")
        .iter()
        .map(|row| row["uri"].as_str().expect("classic uri").to_owned())
        .collect();
    assert_eq!(
        uris(&native).into_iter().collect::<BTreeSet<_>>(),
        classic_uris
    );
}

fn facet_count(values: &Value, iri: &str) -> Option<u64> {
    values
        .as_array()?
        .iter()
        .find(|value| value["iri"] == iri)
        .and_then(|value| value["count"].as_u64())
}
