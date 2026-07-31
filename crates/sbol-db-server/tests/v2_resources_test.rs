//! HTTP-level tests for the V2 idiomatic resource surface over a SQLite backend,
//! driving the real axum router via `oneshot`. They pin the verb semantics that
//! distinguish V2 from V1: `POST /collections` creates, `PATCH /objects/{iri}`
//! edits in place, `DELETE /objects/{iri}` removes, and a `GET` never mutates.
//! Every mutating verb is identity-gated: an anonymous caller is `403`. All
//! delegate to the same `sbol-db-app` facade verbs the V1 adapter calls.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol::v3::Document;
use sbol_db_app::{AppServices, PUBLIC_GRAPH};
use sbol_db_backend::Backend;
use sbol_db_core::SerializationFormat;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::GraphWriteMode;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// A compliant SBOL2 document (Turtle) submitted as the collection `content`: a
/// ComponentDefinition with a nested SequenceAnnotation, plus a Sequence.
const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    sbol:role <http://identifiers.org/so/SO:0000167> ;
    sbol:sequence <http://example.org/seq/1> ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" ;
    sbol:location <http://example.org/cd/anno/range/1> .

<http://example.org/cd/anno/range/1>
    a sbol:Range ;
    sbol:displayId "range" ;
    sbol:persistentIdentity <http://example.org/cd/anno/range> ;
    sbol:version "1" ;
    sbol:start "1"^^xsd:integer ;
    sbol:end "4"^^xsd:integer .

<http://example.org/seq/1>
    a sbol:Sequence ;
    sbol:displayId "seq" ;
    sbol:persistentIdentity <http://example.org/seq> ;
    sbol:version "1" ;
    sbol:elements "atgc" ;
    sbol:encoding <http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html> .
"#;

const GENBANK_FIXTURE: &str = r#"
LOCUS       BBa_B0034                 12 bp    DNA     linear       20-May-2026
DEFINITION  RBS (Elowitz 1999)
ACCESSION   BBa_B0034
VERSION     BBa_B0034.1
FEATURES             Location/Qualifiers
     misc_feature    5..8
                     /label=conserved
ORIGIN
        1 aaagaggaga aa
//
"#;

const FASTA_FIXTURE: &str = ">BBa_B0034 RBS\naaagaggagaaa\n";

/// Build a router over a fresh SQLite backend. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app_with_services() -> (axum::Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2res.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite backend has a migrator")
        .run_migrations()
        .await
        .expect("run migrations");

    let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let config = ServerConfig::default();
    let services = Arc::new(AppServices::from_backend(&backend));
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: services.clone(),
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
    (router(state, config), services, dir)
}

async fn app() -> (axum::Router, TempDir) {
    let (app, _services, dir) = app_with_services().await;
    (app, dir)
}

/// Percent-encode an IRI into a single `/objects/{iri}` path segment.
fn encode_iri(iri: &str) -> String {
    iri.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn body_string(res: Response<Body>) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn response_bytes(res: Response<Body>) -> Vec<u8> {
    to_bytes(res.into_body(), BODY_LIMIT)
        .await
        .expect("body")
        .to_vec()
}

/// Register a user through the V1 `/register` route and return a fresh token
/// from `/login`, usable as a V2 `Authorization: Bearer` credential (both
/// surfaces resolve tokens through the same store).
async fn register_and_login(app: &axum::Router, username: &str, email: &str) -> String {
    let form = serde_urlencoded::to_string([
        ("name", username),
        ("username", username),
        ("email", email),
        ("password", "s3cret"),
    ])
    .expect("encode form");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("register");
    assert_eq!(res.status(), StatusCode::OK, "registration succeeds");

    let form = serde_urlencoded::to_string([("email", email), ("password", "s3cret")])
        .expect("encode form");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("login");
    assert_eq!(res.status(), StatusCode::OK, "login succeeds");
    body_string(res).await.trim().to_owned()
}

/// Issue one request, returning the status, body, and any `Location` header.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    content_type: Option<&str>,
    body: Option<String>,
) -> (StatusCode, String, Option<String>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let req = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    let status = res.status();
    let location = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (status, body_string(res).await, location)
}

/// Submit a collection through `POST /api/v2/collections` and return its minted
/// root Collection URI.
async fn create_collection(app: &axum::Router, token: &str, id: &str) -> String {
    let body = serde_json::json!({
        "id": id,
        "version": "1",
        "name": "A V2 submission",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body, location) = send(
        app,
        "POST",
        "/api/v2/collections",
        Some(token),
        Some("application/json"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {body}");
    assert!(
        location.is_some(),
        "a created collection carries a Location header"
    );
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    value["collection_uri"]
        .as_str()
        .expect("collection_uri")
        .to_owned()
}

#[tokio::test]
async fn post_collection_creates_a_readable_resource() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub1").await;
    assert_eq!(
        collection,
        "http://synbiohub.org/user/alice/sub1/sub1_collection/1"
    );

    // The created collection's closure downloads back, proving POST wrote it.
    let uri = format!("/api/v2/objects/{}?format=sbol", encode_iri(&collection));
    let (status, body, _) = send(&app, "GET", &uri, Some(&token), None, None).await;
    assert_eq!(status, StatusCode::OK, "download body: {body}");
    assert!(!body.is_empty(), "the closure download is non-empty");
}

#[tokio::test]
async fn validation_previews_identity_and_conflicts_without_writing() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let request = serde_json::json!({
        "id": "preview",
        "version": "1",
        "name": "Preview only",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v2/collections/validate",
        Some(&token),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preview body: {body}");
    let preview: serde_json::Value = serde_json::from_str(&body).expect("preview JSON");
    assert_eq!(preview["valid"], true);
    assert_eq!(preview["source_standard"], "sbol2");
    assert_eq!(preview["normalized_standard"], "sbol2");
    assert_eq!(preview["collision"], false);
    assert_eq!(preview["consequence"], "create");
    assert!(preview["members"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let iri = preview["collection_uri"].as_str().expect("collection IRI");
    let (status, _, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{}?format=sbol", encode_iri(iri)),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "validation must not persist the previewed graph"
    );

    let _ = create_collection(&app, &token, "preview").await;
    let request = serde_json::json!({
        "id": "preview",
        "version": "1",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v2/collections/validate",
        Some(&token),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "conflict preview body: {body}");
    let preview: serde_json::Value = serde_json::from_str(&body).expect("preview JSON");
    assert_eq!(preview["collision"], true);
    assert_eq!(preview["consequence"], "reject_conflict");
    assert!(preview["notices"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|notice| notice["code"] == "identity_conflict")
    }));
}

#[tokio::test]
async fn genbank_and_fasta_validate_as_explicit_sbol3_conversions() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    for (id, format, content) in [
        ("genbank", "genbank", GENBANK_FIXTURE),
        ("fasta", "fasta", FASTA_FIXTURE),
    ] {
        let request = serde_json::json!({
            "id": id,
            "version": "1",
            "format": format,
            "content": content,
        })
        .to_string();
        let (status, body, _) = send(
            &app,
            "POST",
            "/api/v2/collections/validate",
            Some(&token),
            Some("application/json"),
            Some(request),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{format} preview body: {body}");
        let preview: serde_json::Value = serde_json::from_str(&body).expect("preview JSON");
        assert_eq!(preview["source_standard"], format);
        assert_eq!(preview["normalized_standard"], "sbol3");
        assert!(preview["notices"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|notice| notice["code"] == "converted_to_sbol3")
        }));
    }
}

#[tokio::test]
async fn collection_membership_and_removal_are_owner_scoped() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "alice", "alice@example.org").await;
    let bob = register_and_login(&app, "bob", "bob@example.org").await;
    let collection = create_collection(&app, &alice, "membership").await;
    let segment = encode_iri(&collection);
    let member = "http://example.org/external/member";

    let request = serde_json::json!({ "member": member }).to_string();
    let (status, body, _) = send(
        &app,
        "POST",
        &format!("/api/v2/collections/{segment}/members"),
        Some(&bob),
        Some("application/json"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-owner add body: {body}");

    let (status, body, _) = send(
        &app,
        "POST",
        &format!("/api/v2/collections/{segment}/members"),
        Some(&alice),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "owner add body: {body}");

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/details"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "details body: {body}");
    let details: serde_json::Value = serde_json::from_str(&body).expect("details JSON");
    assert!(details["members"]["items"]
        .as_array()
        .is_some_and(|items| { items.iter().any(|item| item["uri"] == member) }));

    let member_segment = encode_iri(member);
    let (status, body, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/collections/{segment}/members/{member_segment}"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "owner remove body: {body}");

    let (status, body, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/collections/{segment}"),
        Some(&bob),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-owner collection removal body: {body}"
    );

    let (status, body, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/collections/{segment}"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "owner collection removal body: {body}"
    );
    let (status, _, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}?format=sbol"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_only_sharing_revocation_and_ownership_transfer_have_distinct_powers() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "alice", "alice@example.org").await;
    let bob = register_and_login(&app, "bob", "bob@example.org").await;
    let collection = create_collection(&app, &alice, "collaboration").await;
    let segment = encode_iri(&collection);
    let details = format!("/api/v2/objects/{segment}/details");

    let (status, _, _) = send(&app, "GET", &details, Some(&bob), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let share = serde_json::json!({ "user": "bob" }).to_string();
    let (status, body, _) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{segment}/shares"),
        Some(&alice),
        Some("application/json"),
        Some(share.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "share body: {body}");

    let (status, _, _) = send(&app, "GET", &details, Some(&bob), None, None).await;
    assert_eq!(status, StatusCode::OK, "recipient can inspect the share");
    let edit = serde_json::json!({ "name": "recipient edit" }).to_string();
    let (status, body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{segment}"),
        Some(&bob),
        Some("application/json"),
        Some(edit.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "read-only recipient: {body}");

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/shares"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "collaborator list: {body}");
    let collaborators: serde_json::Value = serde_json::from_str(&body).expect("share JSON");
    assert_eq!(collaborators["owners"][0]["username"], "alice");
    assert_eq!(collaborators["viewers"][0]["username"], "bob");

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/shares"),
        Some(&bob),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "recipient list: {body}");

    let (status, body, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/objects/{segment}/shares/bob"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "revoke body: {body}");
    let (status, _, _) = send(&app, "GET", &details, Some(&bob), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "revocation is immediate");

    let transfer = serde_json::json!({ "user": "bob" }).to_string();
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v2/objects/{segment}/owner"),
        Some(&alice),
        Some("application/json"),
        Some(transfer),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "transfer body: {body}");

    let (status, _, _) = send(&app, "GET", &details, Some(&alice), None, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "former owner loses private access"
    );
    let (status, _, _) = send(&app, "GET", &details, Some(&bob), None, None).await;
    assert_eq!(status, StatusCode::OK, "new owner reads the object");
    let (status, body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{segment}"),
        Some(&bob),
        Some("application/json"),
        Some(edit),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "new owner edit: {body}");

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/activity"),
        Some(&bob),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "new owner audit: {body}");
    let activity: serde_json::Value = serde_json::from_str(&body).expect("activity JSON");
    assert_eq!(activity["total"], 3);
    assert_eq!(activity["items"][0]["action"], "share_granted");
    assert_eq!(activity["items"][1]["action"], "share_revoked");
    assert_eq!(activity["items"][2]["action"], "ownership_transferred");
}

#[tokio::test]
async fn curator_review_is_role_scoped_and_keeps_append_only_audit_evidence() {
    let (app, services, _dir) = app_with_services().await;
    let alice = register_and_login(&app, "alice", "alice@example.org").await;
    let curator = register_and_login(&app, "curator", "curator@example.org").await;
    let member = register_and_login(&app, "member", "member@example.org").await;
    let admin = register_and_login(&app, "admin", "admin@example.org").await;

    let mut curator_user = services
        .users
        .find_by_email_or_username("curator")
        .await
        .expect("curator lookup")
        .expect("curator");
    curator_user.is_curator = true;
    services
        .users
        .update_user(&curator_user)
        .await
        .expect("grant curator role");
    let mut admin_user = services
        .users
        .find_by_email_or_username("admin")
        .await
        .expect("admin lookup")
        .expect("admin");
    admin_user.is_admin = true;
    admin_user.is_curator = true;
    services
        .users
        .update_user(&admin_user)
        .await
        .expect("grant administrator role");

    let collection = create_collection(&app, &alice, "curator-review").await;
    let segment = encode_iri(&collection);
    let review_path = format!("/api/v2/objects/{segment}/reviews");
    let request = serde_json::json!({
        "curator": "curator",
        "note": "Please verify the design intent."
    })
    .to_string();

    let (status, _, _) = send(
        &app,
        "POST",
        &review_path,
        None,
        Some("application/json"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous cannot request");
    let (status, _, _) = send(
        &app,
        "POST",
        &review_path,
        Some(&member),
        Some("application/json"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-owner cannot request");
    let (status, body, _) = send(
        &app,
        "POST",
        &review_path,
        Some(&alice),
        Some("application/json"),
        Some(serde_json::json!({ "curator": "member" }).to_string()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-curator target: {body}"
    );

    let (status, body, _) = send(
        &app,
        "POST",
        &review_path,
        Some(&alice),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "review request: {body}");
    let pending: serde_json::Value = serde_json::from_str(&body).expect("review JSON");
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["events"].as_array().map(Vec::len), Some(1));

    let details = format!("/api/v2/objects/{segment}/details");
    let (status, _, _) = send(&app, "GET", &details, Some(&curator), None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "assigned curator receives read access"
    );

    for (token, expected_total) in [(&alice, 1), (&curator, 1), (&member, 0), (&admin, 1)] {
        let (status, body, _) = send(&app, "GET", "/api/v2/reviews", Some(token), None, None).await;
        assert_eq!(status, StatusCode::OK, "queue body: {body}");
        let queue: serde_json::Value = serde_json::from_str(&body).expect("queue JSON");
        assert_eq!(queue["total"], expected_total);
    }

    let decision_path = format!("{review_path}/decision");
    let decision = serde_json::json!({
        "decision": "approve",
        "note": "Identifiers and provenance are coherent."
    })
    .to_string();
    let (status, _, _) = send(
        &app,
        "POST",
        &decision_path,
        Some(&member),
        Some("application/json"),
        Some(decision.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member cannot decide");
    let (status, body, _) = send(
        &app,
        "POST",
        &decision_path,
        Some(&admin),
        Some("application/json"),
        Some(decision),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "administrator decision: {body}");
    let approved: serde_json::Value = serde_json::from_str(&body).expect("approved JSON");
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["events"].as_array().map(Vec::len), Some(2));

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/activity"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner audit: {body}");
    let activity: serde_json::Value = serde_json::from_str(&body).expect("activity JSON");
    assert_eq!(activity["total"], 2);
    assert_eq!(activity["items"][0]["action"], "review_requested");
    assert_eq!(activity["items"][1]["action"], "review_approved");
}

#[tokio::test]
async fn every_advertised_download_format_has_a_valid_v2_representation() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "formats").await;
    let base = format!("/api/v2/objects/{}", encode_iri(&collection));

    async fn get_download(
        app: &axum::Router,
        token: &str,
        uri: String,
    ) -> (String, String, Vec<u8>) {
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            app.clone().oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("download timed out: {uri}"))
        .expect("download request");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let disposition = response
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response_bytes(response).await;
        assert!(
            !bytes.is_empty(),
            "an advertised representation is non-empty"
        );
        (content_type, disposition, bytes)
    }

    let (content_type, disposition, bytes) =
        get_download(&app, &token, format!("{base}?format=sbol&version=sbol3")).await;
    assert_eq!(content_type, "application/rdf+xml");
    assert!(disposition.ends_with(".xml\""));
    let rdf = String::from_utf8(bytes).expect("SBOL3 RDF/XML is UTF-8");
    let document = Document::read(&rdf, sbol::RdfFormat::RdfXml)
        .expect("SBOL3 download parses as an SBOL document");
    assert!(document.sequences().any(|sequence| {
        sequence.elements.as_deref().map(str::to_ascii_lowercase) == Some("atgc".to_owned())
    }));

    let (content_type, _, bytes) =
        get_download(&app, &token, format!("{base}?format=sbol&version=sbol2")).await;
    assert_eq!(content_type, "application/rdf+xml");
    assert!(
        String::from_utf8(bytes)
            .expect("SBOL2 RDF/XML is UTF-8")
            .contains("sbols.org/v2#"),
        "the compatibility representation is downgraded to SBOL2"
    );

    let (content_type, _, bytes) =
        get_download(&app, &token, format!("{base}?format=sbolnr&version=sbol3")).await;
    assert_eq!(content_type, "application/rdf+xml");
    Document::read(
        &String::from_utf8(bytes).expect("non-recursive RDF/XML is UTF-8"),
        sbol::RdfFormat::RdfXml,
    )
    .expect("non-recursive SBOL parses");

    let (content_type, _, bytes) = get_download(&app, &token, format!("{base}?format=fasta")).await;
    assert_eq!(content_type, "chemical/x-fasta");
    let fasta = String::from_utf8(bytes).expect("FASTA is UTF-8");
    let (document, _) = sbol_fasta::FastaImporter::new("http://example.org/reimport")
        .expect("FASTA importer")
        .read_str(&fasta)
        .expect("FASTA download parses");
    assert!(document.sequences().any(|sequence| {
        sequence.elements.as_deref().map(str::to_ascii_lowercase) == Some("atgc".to_owned())
    }));

    let (content_type, _, bytes) = get_download(&app, &token, format!("{base}?format=gb")).await;
    assert_eq!(content_type, "chemical/x-genbank");
    let genbank = String::from_utf8(bytes).expect("GenBank is UTF-8");
    let (document, _) = sbol_genbank::GenbankImporter::new("http://example.org/reimport")
        .expect("GenBank importer")
        .read_str(&genbank)
        .expect("GenBank download parses");
    assert!(document.sequences().any(|sequence| {
        sequence.elements.as_deref().map(str::to_ascii_lowercase) == Some("atgc".to_owned())
    }));

    let (content_type, _, bytes) = get_download(&app, &token, format!("{base}?format=gff")).await;
    assert_eq!(content_type, "text/plain; charset=utf-8");
    let gff = String::from_utf8(bytes).expect("GFF3 is UTF-8");
    assert!(gff.starts_with("##gff-version 3\n"));
    assert!(gff.contains("##sequence-region"));

    let (content_type, disposition, bytes) =
        get_download(&app, &token, format!("{base}?format=omex")).await;
    assert_eq!(content_type, "application/zip");
    assert!(disposition.ends_with(".omex\""));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid OMEX zip");
    archive
        .by_name("manifest.xml")
        .expect("OMEX has a manifest");
    let mut rdf = String::new();
    archive
        .by_name("sbol.rdf")
        .expect("OMEX has an SBOL member")
        .read_to_string(&mut rdf)
        .expect("read archived SBOL");
    Document::read(&rdf, sbol::RdfFormat::RdfXml).expect("archived SBOL member parses");
}

#[tokio::test]
async fn anonymous_create_is_forbidden() {
    let (app, _dir) = app().await;
    let body = serde_json::json!({ "id": "x", "version": "1", "content": FIXTURE }).to_string();
    let (status, _body, _) = send(
        &app,
        "POST",
        "/api/v2/collections",
        None,
        Some("application/json"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous POST is 403");
}

#[tokio::test]
async fn patch_edits_the_object_in_place() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub2").await;
    let seg = encode_iri(&collection);

    let patch = serde_json::json!({ "mutable_description": "hello v2" }).to_string();
    let (status, body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{seg}"),
        Some(&token),
        Some("application/json"),
        Some(patch),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "patch body: {body}");

    // The edit is visible in the object's verbatim RDF (Turtle negotiation).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v2/objects/{seg}"))
                .header("authorization", format!("Bearer {token}"))
                .header("accept", "text/turtle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("turtle read");
    assert_eq!(res.status(), StatusCode::OK);
    let turtle = body_string(res).await;
    assert!(
        turtle.contains("hello v2"),
        "the patched mutableDescription is present: {turtle}"
    );
}

#[tokio::test]
async fn anonymous_patch_is_forbidden() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub3").await;
    let seg = encode_iri(&collection);

    let patch = serde_json::json!({ "mutable_description": "nope" }).to_string();
    let (status, _body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{seg}"),
        None,
        Some("application/json"),
        Some(patch),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous PATCH is 403");
}

#[tokio::test]
async fn delete_removes_the_object_and_get_never_mutates() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub4").await;
    let seg = encode_iri(&collection);
    let download = format!("/api/v2/objects/{seg}?format=sbol");

    // Two GETs do not remove the object: the second still succeeds, and the
    // subsequent DELETE proves the object was still present.
    for _ in 0..2 {
        let (status, _b, _) = send(&app, "GET", &download, Some(&token), None, None).await;
        assert_eq!(status, StatusCode::OK, "a GET does not mutate");
    }

    let (status, _b, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/objects/{seg}"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "DELETE returns 204");

    // The object is gone: its closure no longer resolves.
    let (status, _b, _) = send(&app, "GET", &download, Some(&token), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the deleted object is 404");
}

#[tokio::test]
async fn anonymous_delete_is_forbidden() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub5").await;
    let seg = encode_iri(&collection);

    let (status, _b, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/objects/{seg}"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous DELETE is 403");
}

#[tokio::test]
async fn publish_moves_a_private_object_to_the_public_graph() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub6").await;
    let seg = encode_iri(&collection);

    let publish = serde_json::json!({ "id": "pub6", "version": "1" }).to_string();
    let (status, body, location) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{seg}/publish"),
        Some(&token),
        Some("application/json"),
        Some(publish),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "publish body: {body}");
    assert!(location.is_some(), "publish carries a Location header");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    let public = value["collection_uri"].as_str().expect("collection_uri");
    assert!(
        public.starts_with("http://synbiohub.org/public/pub6/"),
        "published under the public prefix: {public}"
    );

    // The public object is now readable by an anonymous caller.
    let uri = format!("/api/v2/objects/{}?format=sbol", encode_iri(public));
    let (status, _b, _) = send(&app, "GET", &uri, None, None, None).await;
    assert_eq!(status, StatusCode::OK, "the published object is public");
}

#[tokio::test]
async fn anonymous_download_uses_the_public_copy_of_a_multi_graph_subject() {
    let (app, services, _dir) = app_with_services().await;
    let iri = "https://example.org/public/imported/1";
    let document = format!(
        "<{iri}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
         <http://sbols.org/v3#Component> .\n\
         <{iri}> <http://sbols.org/v3#displayId> \"imported\" .\n"
    );

    // Reproduce the imported-corpus topology: source provenance is retained in
    // its own graph and the same public subject is projected for discovery.
    for graph in ["graph:document:source", PUBLIC_GRAPH] {
        services
            .store
            .graph_store_write(
                graph,
                &document,
                SerializationFormat::NTriples,
                GraphWriteMode::Merge,
            )
            .await
            .expect("seed graph copy");
    }

    let path = format!(
        "/api/v2/objects/{}?format=sbol&version=sbol3",
        encode_iri(iri)
    );
    let (status, body, _) = send(&app, "GET", &path, None, None, None).await;
    assert_eq!(status, StatusCode::OK, "public download body: {body}");
    assert!(body.contains("imported"));
}

#[tokio::test]
async fn list_and_search_return_a_paginated_envelope() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;

    let (status, body, _) = send(
        &app,
        "GET",
        "/api/v2/objects?limit=5&offset=0",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["items"].is_array(), "items is an array: {body}");
    assert!(value["total"].is_number(), "total is a number");
    assert_eq!(value["limit"], 5);
    assert_eq!(value["offset"], 0);

    let (status, body, _) = send(
        &app,
        "GET",
        "/api/v2/search?q=widget&limit=10",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["items"].is_array());
    assert!(value["total"].is_number());
}

#[tokio::test]
async fn private_object_is_hidden_from_an_anonymous_caller() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub7").await;
    let seg = encode_iri(&collection);

    // The owner reads it; an anonymous caller sees a non-disclosing 404.
    let (status, _b, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{seg}?format=sbol"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner reads the private object");

    let (status, _b, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{seg}?format=sbol"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "hidden from anonymous");
}

#[tokio::test]
async fn contribution_lifecycle_records_acl_and_provenance_at_every_transition() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "alice", "alice@example.org").await;
    let bob = register_and_login(&app, "bob", "bob@example.org").await;
    let request = serde_json::json!({
        "id": "journey",
        "version": "1",
        "name": "Draft journey",
        "description": "A private design under review",
        "creator_name": "Alice Example",
        "citations": ["12345678"],
        "format": "turtle",
        "content": FIXTURE,
        "overwrite": "fail"
    })
    .to_string();

    // Validation names the exact future identities but writes nothing.
    let (status, preview_body, _) = send(
        &app,
        "POST",
        "/api/v2/collections/validate",
        Some(&alice),
        Some("application/json"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preview body: {preview_body}");
    let preview: serde_json::Value = serde_json::from_str(&preview_body).expect("preview JSON");
    let private = preview["collection_uri"]
        .as_str()
        .expect("preview collection URI")
        .to_owned();
    let private_segment = encode_iri(&private);
    let (status, _, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{private_segment}/details"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "preview remains write-free");

    // Commit creates a private, owner-scoped graph at exactly the previewed IRI.
    let (status, created_body, location) = send(
        &app,
        "POST",
        "/api/v2/collections",
        Some(&alice),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {created_body}");
    let created: serde_json::Value = serde_json::from_str(&created_body).expect("created JSON");
    assert_eq!(created["collection_uri"], private);
    assert_eq!(
        location.as_deref(),
        Some(format!("/api/v2/objects/{private_segment}").as_str())
    );

    let details_uri = format!("/api/v2/objects/{private_segment}/details");
    let (status, details_body, _) = send(&app, "GET", &details_uri, Some(&alice), None, None).await;
    assert_eq!(status, StatusCode::OK, "owner details: {details_body}");
    let details: serde_json::Value = serde_json::from_str(&details_body).expect("details JSON");
    assert_eq!(details["visibility"], "restricted");
    assert_eq!(details["source_graph"], private);
    assert!(details["owners"].as_array().is_some_and(|owners| {
        owners
            .iter()
            .any(|owner| owner == "http://synbiohub.org/user/alice")
    }));
    assert_eq!(details["provenance"]["creators"][0], "Alice Example");
    assert_eq!(details["provenance"]["citations"][0], "12345678");

    for (token, expected) in [
        (None, StatusCode::NOT_FOUND),
        (Some(bob.as_str()), StatusCode::NOT_FOUND),
    ] {
        let (status, _, _) = send(&app, "GET", &details_uri, token, None, None).await;
        assert_eq!(status, expected, "private reads are non-disclosing");
    }

    // Revision is owner-only and survives the later remint into public space.
    let patch = serde_json::json!({
        "name": "Reviewed journey",
        "description": "Ready to publish"
    })
    .to_string();
    let (status, forbidden_body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{private_segment}"),
        Some(&bob),
        Some("application/json"),
        Some(patch.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-owner edit: {forbidden_body}"
    );
    let (status, edit_body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{private_segment}"),
        Some(&alice),
        Some("application/json"),
        Some(patch),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner edit: {edit_body}");
    let (status, details_body, _) = send(&app, "GET", &details_uri, Some(&alice), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let details: serde_json::Value = serde_json::from_str(&details_body).expect("details JSON");
    assert_eq!(details["name"], "Reviewed journey");
    assert_eq!(details["description"], "Ready to publish");
    assert!(details["modified_at"].is_string());

    let publish = serde_json::json!({
        "id": "published-journey",
        "version": "1",
        "citations": ["12345678"],
        "overwrite": "fail"
    })
    .to_string();
    let (status, published_body, location) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{private_segment}/publish"),
        Some(&alice),
        Some("application/json"),
        Some(publish),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "publish body: {published_body}"
    );
    let published: serde_json::Value =
        serde_json::from_str(&published_body).expect("published JSON");
    let public = published["collection_uri"]
        .as_str()
        .expect("public collection URI");
    assert_eq!(
        location.as_deref(),
        Some(format!("/api/v2/objects/{}", encode_iri(public)).as_str())
    );
    assert!(public.starts_with("http://synbiohub.org/public/published-journey/"));

    let (status, _, _) = send(&app, "GET", &details_uri, Some(&alice), None, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "publication removes the private source"
    );

    let public_segment = encode_iri(public);
    let (status, public_body, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{public_segment}/details"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "anonymous public details: {public_body}"
    );
    let public_details: serde_json::Value =
        serde_json::from_str(&public_body).expect("public details JSON");
    assert_eq!(public_details["visibility"], "public");
    assert_eq!(public_details["name"], "Reviewed journey");
    assert_eq!(public_details["description"], "Ready to publish");
    assert_eq!(
        public_details["provenance"]["creators"][0], "alice",
        "publication records the authenticated account profile as creator"
    );
    assert_eq!(public_details["provenance"]["citations"][0], "12345678");
    assert!(public_details["owners"].as_array().is_some_and(|owners| {
        owners
            .iter()
            .any(|owner| owner == "http://synbiohub.org/user/alice")
    }));

    // The terminal artifact is a public, anonymously downloadable SBOL document.
    let (status, rdf, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{public_segment}?format=sbol&version=sbol3"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "public download: {rdf}");
    let document =
        Document::read(&rdf, sbol::RdfFormat::RdfXml).expect("published download parses as SBOL3");
    assert!(document.sequences().any(|sequence| {
        sequence.elements.as_deref().map(str::to_ascii_lowercase) == Some("atgc".to_owned())
    }));
}
