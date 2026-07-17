//! SynBioHub v1 wire-compatibility adapter.
//!
//! This module is the quarantine layer for classic SynBioHub's HTTP quirks:
//! the `X-authorization` token header, the Accept-negotiated login response,
//! the form-encoded request bodies. Every route here talks only to the
//! [`AppServices`](sbol_db_app::AppServices) facade through `state.app`; none
//! reach into a concrete backend. Containing the quirks here lets the idiomatic
//! V2 surface evolve without inheriting them.

mod admin;
mod attachments;
mod auth;
mod download;
mod edit;
mod mutate;
mod permission;
mod queries;
mod routes;
mod search;
mod sequence;
mod submit;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use sbol_db_core::User;

use crate::AppState;

/// The account a request authenticates as, resolved from the `X-authorization`
/// token by [`attach_current_user`] and read by the V1 routes through an
/// `Extension<CurrentUser>`. Anonymous (`None`) when no valid token is present.
#[derive(Clone, Debug, Default)]
pub struct CurrentUser(pub Option<User>);

/// The V1 SynBioHub-compatible auth routes, with the `X-authorization`
/// middleware applied so every route observes a resolved [`CurrentUser`]. The
/// Basic-auth `/sparql-auth*` path is unrelated and stays in [`crate::auth`]
/// for Virtuoso-protocol clients.
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/register", post(auth::register))
        .route(
            "/profile",
            get(auth::get_profile).post(auth::update_profile),
        )
        .route("/resetPassword", post(auth::reset_password))
        .route("/setNewPassword", post(auth::set_new_password))
        .route("/admin/reindex", post(admin::reindex))
        // Submission: mint an SBOL document into the caller's own user graph.
        // Identity-gated; anonymous callers are rejected.
        .route("/submit", post(submit::submit))
        // Destructive object verbs. Classic triggers these with GET on the
        // object path (a browser-form quirk); the facade holds the real verbs
        // and gates every one on caller ownership.
        .route(
            "/user/:userId/:collectionId/:displayId/:version/makePublic",
            post(mutate::user_make_public),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/remove",
            get(mutate::user_remove).post(mutate::user_remove),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/replace",
            get(mutate::user_replace).post(mutate::user_replace),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/removeCollection",
            get(mutate::user_remove_collection).post(mutate::user_remove_collection),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/icon",
            post(mutate::user_icon),
        )
        .route(
            "/public/:collectionId/:displayId/:version/remove",
            get(mutate::public_remove).post(mutate::public_remove),
        )
        .route(
            "/public/:collectionId/:displayId/:version/replace",
            get(mutate::public_replace).post(mutate::public_replace),
        )
        .route(
            "/public/:collectionId/:displayId/:version/removeCollection",
            get(mutate::public_remove_collection).post(mutate::public_remove_collection),
        )
        .route(
            "/public/:collectionId/:displayId/:version/icon",
            post(mutate::public_icon),
        )
        // Field-edit surface. The mutable text fields and citations take the
        // target uri in the body; the generic edit/add/remove of a field and the
        // membership verbs take it in the path. Every one is owner-gated and
        // refreshes dcterms:modified.
        .route(
            "/updateMutableDescription",
            post(edit::update_mutable_description),
        )
        .route("/updateMutableNotes", post(edit::update_mutable_notes))
        .route("/updateMutableSource", post(edit::update_mutable_source))
        .route("/updateCitations", post(edit::update_citations))
        .route(
            "/user/:userId/:collectionId/:displayId/:version/edit/:field",
            post(edit::user_edit_field),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/add/:field",
            post(edit::user_add_field),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/remove/:field",
            post(edit::user_remove_field),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/addToCollection",
            post(edit::user_add_to_collection),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/removeMembership",
            post(edit::user_remove_membership),
        )
        .route(
            "/public/:collectionId/:displayId/:version/edit/:field",
            post(edit::public_edit_field),
        )
        .route(
            "/public/:collectionId/:displayId/:version/add/:field",
            post(edit::public_add_field),
        )
        .route(
            "/public/:collectionId/:displayId/:version/remove/:field",
            post(edit::public_remove_field),
        )
        .route(
            "/public/:collectionId/:displayId/:version/addToCollection",
            post(edit::public_add_to_collection),
        )
        .route(
            "/public/:collectionId/:displayId/:version/removeMembership",
            post(edit::public_remove_membership),
        )
        // Object-sharing (permission) surface: grant and revoke another user's
        // view access. Owner-gated through the facade.
        .route(
            "/user/:userId/:collectionId/:displayId/:version/addOwner",
            post(permission::user_add_owner),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/removeOwner/:username",
            get(permission::user_remove_owner).post(permission::user_remove_owner),
        )
        .route(
            "/public/:collectionId/:displayId/:version/addOwner",
            post(permission::public_add_owner),
        )
        .route(
            "/public/:collectionId/:displayId/:version/removeOwner/:username",
            get(permission::public_remove_owner).post(permission::public_remove_owner),
        )
        // Read/query surface. Free-text relevance runs over the ranked index;
        // facets, counts, members, uses, twins, and metadata are SPARQL over
        // the shared engine, all under the caller's authorized graph scope.
        .route("/search", get(routes::search_root))
        .route("/search/*query", get(routes::search))
        .route("/searchCount", get(routes::search_count_root))
        .route("/searchCount/*query", get(routes::search_count))
        .route("/:type/count", get(routes::type_count))
        .route("/rootCollections", get(routes::root_collections))
        .route("/manage", get(routes::manage))
        .route("/shared", get(routes::shared))
        .route(
            "/public/:collectionId/:displayId/:version/uses",
            get(routes::public_uses),
        )
        .route(
            "/public/:collectionId/:displayId/:version/usesCount",
            get(routes::public_uses_count),
        )
        .route(
            "/public/:collectionId/:displayId/:version/similar",
            get(sequence::public_similar),
        )
        .route(
            "/public/:collectionId/:displayId/:version/similarCount",
            get(sequence::public_similar_count),
        )
        .route(
            "/public/:collectionId/:displayId/:version/twins",
            get(routes::public_twins),
        )
        .route(
            "/public/:collectionId/:displayId/:version/twinsCount",
            get(routes::public_twins_count),
        )
        .route(
            "/public/:collectionId/:displayId/:version/subCollections",
            get(routes::public_sub_collections),
        )
        .route(
            "/public/:collectionId/:displayId/:version/metadata",
            get(routes::public_metadata),
        )
        // Download surface: the object's closure rendered in each exchange
        // format, ACL-scoped through the shared downloader and P3 serializers.
        .route(
            "/public/:collectionId/:displayId/:version/sbol",
            get(download::public_sbol),
        )
        .route(
            "/public/:collectionId/:displayId/:version/sbolnr",
            get(download::public_sbolnr),
        )
        .route(
            "/public/:collectionId/:displayId/:version/gb",
            get(download::public_genbank),
        )
        .route(
            "/public/:collectionId/:displayId/:version/fasta",
            get(download::public_fasta),
        )
        .route(
            "/public/:collectionId/:displayId/:version/gff",
            get(download::public_gff),
        )
        .route(
            "/public/:collectionId/:displayId/:version/omex",
            get(download::public_omex),
        )
        .route(
            "/public/:collectionId/:displayId/:version/summary",
            get(download::public_summary),
        )
        // Attachment surface: store an uploaded file or an external URL as a
        // first-class attachment, and stream a stored blob back. The attach
        // verbs are owner-gated; the download read is ACL-scoped.
        .route(
            "/public/:collectionId/:displayId/:version/attach",
            post(attachments::public_attach),
        )
        .route(
            "/public/:collectionId/:displayId/:version/attachURL",
            post(attachments::public_attach_url),
        )
        .route(
            "/public/:collectionId/:displayId/:version/download",
            get(attachments::public_download),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/uses",
            get(routes::user_uses),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/usesCount",
            get(routes::user_uses_count),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/similar",
            get(sequence::user_similar),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/similarCount",
            get(sequence::user_similar_count),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/twins",
            get(routes::user_twins),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/twinsCount",
            get(routes::user_twins_count),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/subCollections",
            get(routes::user_sub_collections),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/metadata",
            get(routes::user_metadata),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/sbol",
            get(download::user_sbol),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/sbolnr",
            get(download::user_sbolnr),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/gb",
            get(download::user_genbank),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/fasta",
            get(download::user_fasta),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/gff",
            get(download::user_gff),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/omex",
            get(download::user_omex),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/summary",
            get(download::user_summary),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/attach",
            post(attachments::user_attach),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/attachURL",
            post(attachments::user_attach_url),
        )
        .route(
            "/user/:userId/:collectionId/:displayId/:version/download",
            get(attachments::user_download),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            attach_current_user,
        ))
}

/// Resolve the `X-authorization` token to the caller's account and attach it to
/// the request as a [`CurrentUser`] extension. A missing or unrecognized token
/// yields an anonymous `CurrentUser(None)` rather than a rejection; each route
/// decides whether it requires authentication.
pub async fn attach_current_user(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get("x-authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let user = match token {
        Some(token) if !token.is_empty() => resolve_user(&state, &token).await,
        _ => None,
    };
    req.extensions_mut().insert(CurrentUser(user));
    next.run(req).await
}

/// Resolve a plaintext token to the account it authenticates, tolerating a
/// stale or bogus token (which resolves to `None`).
async fn resolve_user(state: &AppState, token: &str) -> Option<User> {
    let user_id = state.app.auth.resolve_token(token).await.ok().flatten()?;
    state.app.users.get_by_id(user_id).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::extract::Extension;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use sbol_db_app::{AppServices, Registration};
    use sbol_db_backend::Backend;
    use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;
    use crate::{Metrics, SchemaCache, ServerConfig};

    /// A probe handler reporting the resolved identity: the authenticated
    /// username, or `anonymous` when the middleware attached no account.
    async fn whoami(Extension(CurrentUser(user)): Extension<CurrentUser>) -> String {
        user.map(|u| u.username)
            .unwrap_or_else(|| "anonymous".to_owned())
    }

    /// Build a real [`AppState`] over a fresh SQLite backend, register one
    /// account, and mint it a token. Returns the state, the account's plaintext
    /// token, its username, and the `TempDir` owning the database file.
    async fn state_with_user() -> (AppState, String, String, TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mw.db");
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
        let app = Arc::new(AppServices::from_backend(&backend));
        let user = app
            .auth
            .register(Registration {
                username: "alice".to_owned(),
                name: "Alice".to_owned(),
                email: "alice@example.org".to_owned(),
                affiliation: None,
                password: "s3cret".to_owned(),
                is_admin: false,
                is_curator: false,
                is_member: true,
            })
            .await
            .expect("register");
        let token = app.auth.issue_token(user.id).await.expect("issue token");

        let state = AppState {
            service: backend.store.clone(),
            sparql,
            sparql_update,
            app,
            metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
            jobs: backend.jobs.clone(),
            lab: backend.lab.clone(),
            config: ServerConfig::default(),
            backend_kind: backend.kind,
            sql_console: backend.sql_console.clone(),
            db_stats: backend.db_stats.clone(),
            lsm_stats: backend.lsm_stats.clone(),
            schema_cache: Arc::new(SchemaCache::new()),
        };
        (state, token, user.username, dir)
    }

    /// A router that runs `attach_current_user` ahead of the `whoami` probe.
    fn probe(state: AppState) -> Router {
        Router::new()
            .route("/whoami", get(whoami))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                attach_current_user,
            ))
            .with_state(state)
    }

    async fn whoami_with(app: &Router, header: Option<&str>) -> (StatusCode, String) {
        let mut builder = Request::builder().method("GET").uri("/whoami");
        if let Some(token) = header {
            builder = builder.header("x-authorization", token);
        }
        let res = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .expect("whoami request");
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 64 * 1024).await.expect("body");
        (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
    }

    #[tokio::test]
    async fn valid_token_resolves_current_user() {
        let (state, token, username, _dir) = state_with_user().await;
        let app = probe(state);
        let (status, body) = whoami_with(&app, Some(&token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, username, "the token should resolve to its account");
    }

    #[tokio::test]
    async fn missing_or_bad_token_is_anonymous() {
        let (state, _token, _username, _dir) = state_with_user().await;
        let app = probe(state);

        // No header: anonymous, not an error.
        let (status, body) = whoami_with(&app, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous");

        // An unrecognized token also resolves to anonymous.
        let (status, body) = whoami_with(&app, Some("not-a-real-token")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "anonymous");
    }

    /// Build the full V1 router and drive a read route and an auth-gated route,
    /// which also exercises route registration (a conflict would panic here).
    async fn v1_router() -> (Router, TempDir) {
        let (state, _token, _username, dir) = state_with_user().await;
        (router(state.clone()).with_state(state), dir)
    }

    async fn send_get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(token) = token {
            builder = builder.header("x-authorization", token);
        }
        let res = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .expect("request");
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 256 * 1024).await.expect("body");
        (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
    }

    #[tokio::test]
    async fn read_routes_register_and_respond() {
        let (app, _dir) = v1_router().await;

        // An empty store still answers the SPARQL-backed count as a
        // well-formed SPARQL-results JSON document with a `count` binding.
        let (status, body) = send_get(&app, "/ComponentDefinition/count", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"count\""), "count response: {body}");

        // The ranked free-text path returns the six-column search projection.
        let (status, body) = send_get(&app, "/search/plasmid", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("displayId"), "search response: {body}");

        // Root collections is SPARQL over the accelerator.
        let (status, _body) = send_get(&app, "/rootCollections", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn manage_and_shared_require_identity() {
        let (app, _dir) = v1_router().await;
        let (status, _body) = send_get(&app, "/manage", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _body) = send_get(&app, "/shared", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
