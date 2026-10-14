//! Wire-level coverage for the optional SBOLExplorer-compatible listener.
//!
//! These tests build the main and compatibility routers from one SQLite-backed
//! [`AppState`], then prove that the compatibility surface enforces the graph
//! dataset SynBioHub sends, persists its legacy config view, exposes control
//! endpoints, and emits a listener-specific request metric.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_search::ranked_text::IndexedPart;
use sbol_db_server::{explorer_router, router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlOptions, SparqlUpdateEngine};
use sbol_db_storage::RankRow;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";
const PRIVATE_GRAPH: &str = "http://synbiohub.org/user/alice";

async fn apps() -> (axum::Router, axum::Router, TempDir) {
    let (main, explorer, dir, _) = apps_with_services().await;
    (main, explorer, dir)
}

async fn apps_with_services() -> (axum::Router, axum::Router, TempDir, Arc<AppServices>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("explorer.db");
    let database_url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&database_url).await.expect("open backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("migrate");

    let services = Arc::new(AppServices::from_backend(&backend));
    services
        .text_search
        .rebuild([
            indexed(
                "http://synbiohub.org/public/public_promoter/1",
                PUBLIC_GRAPH,
            ),
            indexed(
                "http://synbiohub.org/user/alice/private_promoter/1",
                PRIVATE_GRAPH,
            ),
        ])
        .expect("build ranked index");

    seed_part(&services, PRIVATE_GRAPH, "private_promoter", "promoter").await;
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
        sparql_update: Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        )),
        app: services.clone(),
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION"))
            .with_jobs_repo(backend.jobs.clone()),
        jobs: backend.jobs.clone(),
        config: config.clone(),
        lab: backend.lab.clone(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    (
        router(state.clone(), config.clone()),
        explorer_router(state, config),
        dir,
        services,
    )
}

async fn app() -> (axum::Router, TempDir) {
    let (_, explorer, dir) = apps().await;
    (explorer, dir)
}

fn indexed(subject: &str, graph: &str) -> IndexedPart {
    IndexedPart {
        subject: subject.to_owned(),
        graph: graph.to_owned(),
        display_id: Some(subject.rsplit('/').nth(1).unwrap().to_owned()),
        name: Some("promoter".to_owned()),
        description: Some("compatibility test promoter".to_owned()),
        version: Some("1".to_owned()),
        type_iris: vec!["http://sbols.org/v2#ComponentDefinition".to_owned()],
        keywords: "promoter".to_owned(),
        pagerank: 1.0,
    }
}

fn text_query(from: &str) -> String {
    format!(
        "PREFIX sbol2: <http://sbols.org/v2#> PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type\n\
         {from} WHERE {{\n\
         FILTER (CONTAINS(lcase(?displayId), lcase('promoter')))\n\
         ?subject a ?type . ?subject sbh:topLevel ?subject . OPTIONAL {{ ?subject sbol2:displayId ?displayId }}\n\
         }} LIMIT 50 OFFSET 0"
    )
}

fn browse_query(count: bool) -> String {
    let projection = if count {
        "select (sum(?tempcount) as ?count) WHERE { { SELECT (count(distinct ?subject) as ?tempcount)"
    } else {
        "SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type"
    };
    let close = if count { "} }" } else { "" };
    format!(
        "PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> PREFIX sbol2: <http://sbols.org/v2#> {projection} WHERE {{\n\
         ?subject a ?type .\n\
         ?subject sbh:topLevel ?subject .\n\
         OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}\n\
         }} {close} LIMIT 50 OFFSET 0"
    )
}

fn search_uri(query: &str, default_graph: Option<&str>) -> String {
    let mut pairs = vec![("query", query)];
    if let Some(default_graph) = default_graph {
        pairs.push(("default-graph-uri", default_graph));
    }
    format!(
        "/?{}",
        serde_urlencoded::to_string(pairs).expect("encode query")
    )
}

async fn request(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("request")
}

async fn body(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("response body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn subjects(response: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(response).expect("SPARQL JSON");
    value["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|binding| binding["subject"]["value"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn explorer_shaped_query_is_ranked_only_on_the_compatibility_listener() {
    let (main, explorer, _dir) = apps().await;
    let query = text_query("");

    // The main server is the triplestore role. Its public graph is deliberately
    // empty, so this ordinary SPARQL query must return no rows even though the
    // process-local ranked index contains a matching document.
    let main_uri = format!(
        "/sparql?{}",
        serde_urlencoded::to_string([
            ("query", query.as_str()),
            ("default-graph-uri", PUBLIC_GRAPH),
        ])
        .expect("encode main SPARQL query")
    );
    let main_response = request(&main, Request::get(main_uri).body(Body::empty()).unwrap()).await;
    assert_eq!(main_response.status(), StatusCode::OK);
    assert!(
        subjects(&body(main_response).await).is_empty(),
        "the triplestore endpoint must evaluate Explorer-shaped SPARQL literally"
    );

    // The dedicated listener owns the SBOLExplorer compatibility semantics and
    // therefore answers the identical query from the ranked index.
    let explorer_response = request(
        &explorer,
        Request::get(search_uri(&query, Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(explorer_response.status(), StatusCode::OK);
    assert_eq!(
        subjects(&body(explorer_response).await),
        vec!["http://synbiohub.org/public/public_promoter/1"]
    );
}

#[tokio::test]
async fn root_listener_enforces_default_and_from_graphs() {
    let (app, _dir) = app().await;

    let public = request(
        &app,
        Request::get(search_uri(&text_query(""), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(
        subjects(&body(public).await),
        vec!["http://synbiohub.org/public/public_promoter/1"]
    );

    let both_query = text_query(&format!("FROM <{PUBLIC_GRAPH}> FROM <{PRIVATE_GRAPH}>"));
    let both = request(
        &app,
        Request::get(search_uri(&both_query, Some("http://ignored/default")))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(both.status(), StatusCode::OK);
    let mut both_subjects = subjects(&body(both).await);
    both_subjects.sort();
    assert_eq!(
        both_subjects,
        vec![
            "http://synbiohub.org/public/public_promoter/1",
            "http://synbiohub.org/user/alice/private_promoter/1",
        ]
    );

    let no_dataset = request(
        &app,
        Request::get(search_uri(&text_query(""), None))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_dataset.status(), StatusCode::OK);
    assert!(subjects(&body(no_dataset).await).is_empty());
}

#[tokio::test]
async fn empty_synbiohub_search_is_a_scoped_ranked_browse() {
    let (app, _dir) = app().await;

    let rows = request(
        &app,
        Request::get(search_uri(&browse_query(false), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(rows.status(), StatusCode::OK);
    assert_eq!(
        subjects(&body(rows).await),
        vec!["http://synbiohub.org/public/public_promoter/1"]
    );

    let count = request(
        &app,
        Request::get(search_uri(&browse_query(true), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(count.status(), StatusCode::OK);
    let count: Value = serde_json::from_str(&body(count).await).unwrap();
    assert_eq!(count["results"]["bindings"][0]["count"]["value"], "1");
}

#[tokio::test]
async fn config_round_trips_and_control_endpoints_are_compatible() {
    let (app, _dir) = app().await;

    let initial = request(&app, Request::get("/config").body(Body::empty()).unwrap()).await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial: Value = serde_json::from_str(&body(initial).await).unwrap();
    assert_eq!(initial["engine"], "sbol-db");
    assert_eq!(initial["uclust_identity"], "0.8");

    let updated = request(
        &app,
        Request::post("/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"uclust_identity":"0.9","custom":"kept"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated: Value = serde_json::from_str(&body(updated).await).unwrap();
    assert_eq!(updated["uclust_identity"], "0.9");
    assert_eq!(updated["custom"], "kept");
    assert_eq!(updated["engine"], "sbol-db");

    for path in [
        "/info",
        "/indexinginfo",
        "/healthz",
        "/readyz",
        "/incrementalremove?subject=http%3A%2F%2Fexample.org%2Fx",
        "/incrementalremovecollection?subject=http%3A%2F%2Fexample.org%2Fx",
    ] {
        let response = request(&app, Request::get(path).body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }

    let incremental = request(
        &app,
        Request::post("/incrementalupdate")
            .header("content-type", "application/json")
            .body(Body::from("[]"))
            .unwrap(),
    )
    .await;
    assert_eq!(incremental.status(), StatusCode::OK);

    let rebuild = request(&app, Request::get("/update").body(Body::empty()).unwrap()).await;
    assert_eq!(rebuild.status(), StatusCode::ACCEPTED);
    assert!(rebuild.headers().contains_key("x-sbol-db-job-id"));

    let indexing = request(
        &app,
        Request::get("/indexinginfo").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(indexing.status(), StatusCode::OK);
    let indexing = body(indexing).await;
    assert!(indexing.contains("job_id="), "{indexing}");
    assert!(indexing.contains("status=queued"), "{indexing}");
}

#[tokio::test]
async fn metrics_prove_the_compatibility_listener_received_requests() {
    let (app, _dir) = app().await;
    let _ = request(&app, Request::get("/info").body(Body::empty()).unwrap()).await;
    let metrics = request(&app, Request::get("/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics = body(metrics).await;
    assert!(
        metrics.contains("sbol_db_explorer_requests_total"),
        "listener-specific counter absent: {metrics}"
    );
    assert!(metrics.contains("route=\"/info\""), "route label absent");
}

async fn seed_part(services: &AppServices, graph: &str, id: &str, name: &str) -> String {
    let subject = format!("{graph}/{id}/1");
    let update = format!(
        r#"INSERT DATA {{ GRAPH <{graph}> {{
        <{subject}> a <http://sbols.org/v2#ComponentDefinition> ;
          <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <{subject}> ;
          <http://sbols.org/v2#displayId> "{id}" ;
          <http://purl.org/dc/terms/title> "{name}" .
    }} }}"#
    );
    services
        .sparql_update
        .execute(&update, None, &SparqlOptions::default())
        .await
        .unwrap();
    subject
}

fn policy_query(criteria: &str, count: bool, offset: usize, limit: usize) -> String {
    let body = format!("{criteria} ?subject a ?type . ?subject sbh:topLevel ?subject . OPTIONAL {{ ?subject sbol2:displayId ?displayId }} OPTIONAL {{ ?subject dcterms:title ?name }} OPTIONAL {{ ?subject sbol2:role ?role }}");
    let dataset = format!("FROM <{PUBLIC_GRAPH}> FROM <{PRIVATE_GRAPH}>");
    let select = if count {
        format!("SELECT (SUM(?tempcount) AS ?count) {dataset} WHERE {{ {{ SELECT (COUNT(DISTINCT ?subject) AS ?tempcount) WHERE {{ {body} }} }} }}")
    } else {
        format!(
            "SELECT DISTINCT ?subject ?displayId ?name ?type ?role {dataset} WHERE {{ {body} }}"
        )
    };
    format!("PREFIX sbol2: <http://sbols.org/v2#> PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> PREFIX dcterms: <http://purl.org/dc/terms/> {select} LIMIT {limit} OFFSET {offset}")
}

async fn search_response(
    app: &axum::Router,
    criteria: &str,
    count: bool,
    offset: usize,
    limit: usize,
) -> String {
    let query = policy_query(criteria, count, offset, limit);
    let response = request(
        app,
        Request::get(search_uri(&query, None))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let status = response.status();
    let text = body(response).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    text
}

#[tokio::test]
async fn private_search_is_live_literal_and_precedes_fuzzy_public_results() {
    let (_, app, _dir, services) = apps_with_services().await;
    let exact = seed_part(&services, PRIVATE_GRAPH, "pICSL50009", "exact").await;
    seed_part(&services, PRIVATE_GRAPH, "pICSL50004", "nearby").await;
    seed_part(
        &services,
        "http://synbiohub.org/user/bob",
        "pICSL50009",
        "hidden",
    )
    .await;
    let fuzzy = format!("{PUBLIC_GRAPH}/pICSL50004/1");
    services
        .text_search
        .rebuild([indexed(&fuzzy, PUBLIC_GRAPH)])
        .unwrap();
    let criteria = "FILTER(CONTAINS(LCASE(?displayId), LCASE('pICSL50009')))";
    // The exact private object has no index entry or PageRank yet.
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 0, 50).await),
        [exact, fuzzy.clone()]
    );
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 1, 1).await),
        [fuzzy]
    );
    let count: Value =
        serde_json::from_str(&search_response(&app, criteria, true, 1, 1).await).unwrap();
    assert_eq!(count["results"]["bindings"][0]["count"]["value"], "2");
}

#[tokio::test]
async fn private_matching_preserves_boolean_filters_and_deduplicates_roles() {
    let (_, app, _dir, services) = apps_with_services().await;
    let wanted = seed_part(&services, PRIVATE_GRAPH, "laci_inverter", "wanted").await;
    seed_part(&services, PRIVATE_GRAPH, "other_laci_inverter", "excluded").await;
    services.sparql_update.execute(&format!("INSERT DATA {{ GRAPH <{PRIVATE_GRAPH}> {{ <{wanted}> <http://sbols.org/v2#role> <http://example/role>, <http://identifiers.org/so/SO:0000167> }} }}"), None, &SparqlOptions::default()).await.unwrap();
    let criteria = "FILTER(CONTAINS(LCASE(?displayId), LCASE('LacI_Inv')) && !CONTAINS(LCASE(?name), LCASE('excluded')))";
    let response = search_response(&app, criteria, false, 0, 50).await;
    assert_eq!(subjects(&response), [wanted]);
    let rows: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        rows["results"]["bindings"][0]["role"]["value"],
        "http://identifiers.org/so/SO:0000167"
    );
    let count: Value =
        serde_json::from_str(&search_response(&app, criteria, true, 0, 50).await).unwrap();
    assert_eq!(count["results"]["bindings"][0]["count"]["value"], "1");
}

#[tokio::test]
async fn public_text_is_intersected_with_live_collection_membership() {
    let (_, app, _dir, services) = apps_with_services().await;
    let selected = seed_part(&services, PUBLIC_GRAPH, "pICSL50004", "selected").await;
    let excluded = seed_part(&services, PUBLIC_GRAPH, "pICSL50009", "excluded").await;
    services
        .text_search
        .rebuild([
            indexed(&excluded, PUBLIC_GRAPH),
            indexed(&selected, PUBLIC_GRAPH),
        ])
        .unwrap();
    services.sparql_update.execute(&format!("INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ <http://example/collection> <http://sbols.org/v2#member> <{selected}> }} }}"), None, &SparqlOptions::default()).await.unwrap();
    let criteria = "<http://example/collection> sbol2:member ?subject . FILTER(CONTAINS(LCASE(?displayId), LCASE('pICSL50009')))";
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 0, 50).await),
        [selected]
    );
    assert!(subjects(
        &search_response(
            &app,
            &criteria.replace("http://example/collection", "http://example/empty"),
            false,
            0,
            50
        )
        .await
    )
    .is_empty());
}

#[tokio::test]
async fn structural_search_orders_unranked_private_then_public_then_ranked() {
    let (_, app, _dir, services) = apps_with_services().await;
    let private_new = seed_part(&services, PRIVATE_GRAPH, "new", "new").await;
    let public_new = seed_part(&services, PUBLIC_GRAPH, "new", "new").await;
    let private_ranked = seed_part(&services, PRIVATE_GRAPH, "ranked", "ranked").await;
    let public_ranked = seed_part(&services, PUBLIC_GRAPH, "ranked", "ranked").await;
    services
        .pagerank
        .replace_all_ranks(vec![
            RankRow {
                iri: private_ranked.clone(),
                score: 2.0,
            },
            RankRow {
                iri: public_ranked.clone(),
                score: 9.0,
            },
        ])
        .await
        .unwrap();
    let criteria =
        "?subject a sbol2:ComponentDefinition . FILTER(?displayId != 'private_promoter')";
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 0, 50).await),
        [private_new, public_new, public_ranked, private_ranked]
    );
}

#[tokio::test]
async fn private_removal_and_publication_do_not_use_stale_private_index_entries() {
    let (_, app, _dir, services) = apps_with_services().await;
    let subject = seed_part(&services, PRIVATE_GRAPH, "pICSL50009", "part").await;
    services
        .text_search
        .rebuild([indexed(&subject, PRIVATE_GRAPH)])
        .unwrap();
    let criteria = "FILTER(CONTAINS(LCASE(?displayId), LCASE('pICSL50009')))";
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 0, 50).await).as_slice(),
        std::slice::from_ref(&subject)
    );
    services
        .sparql_update
        .execute(
            &format!("DELETE WHERE {{ GRAPH <{PRIVATE_GRAPH}> {{ <{subject}> ?p ?o }} }}"),
            None,
            &SparqlOptions::default(),
        )
        .await
        .unwrap();
    assert!(subjects(&search_response(&app, criteria, false, 0, 50).await).is_empty());
    let published = seed_part(&services, PUBLIC_GRAPH, "pICSL50009", "part").await;
    services
        .text_search
        .rebuild([
            indexed(&subject, PRIVATE_GRAPH),
            indexed(&published, PUBLIC_GRAPH),
        ])
        .unwrap();
    assert_eq!(
        subjects(&search_response(&app, criteria, false, 0, 50).await),
        [published]
    );
}

#[tokio::test]
async fn empty_browse_puts_indexed_private_results_first() {
    let (_, app, _dir, services) = apps_with_services().await;
    let private = format!("{PRIVATE_GRAPH}/private_promoter/1");
    let public = format!("{PUBLIC_GRAPH}/public_promoter/1");
    let mut public_part = indexed(&public, PUBLIC_GRAPH);
    public_part.pagerank = 100.0;
    services
        .text_search
        .rebuild([public_part, indexed(&private, PRIVATE_GRAPH)])
        .unwrap();
    assert_eq!(
        subjects(&search_response(&app, "", false, 0, 50).await),
        [private, public]
    );
    let count: Value = serde_json::from_str(&search_response(&app, "", true, 1, 1).await).unwrap();
    assert_eq!(count["results"]["bindings"][0]["count"]["value"], "2");
}
