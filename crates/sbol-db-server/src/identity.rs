//! SBOL Identity OAuth 2.1 and OpenID Connect provider, plus MCP
//! protected-resource discovery.
//!
//! The browser session authenticates the resource owner at the authorization
//! endpoint. Public clients use authorization code plus mandatory S256 PKCE;
//! issued access tokens are opaque, short-lived, audience-bound, and scoped.
//! OpenID Connect adds signed ID tokens and UserInfo for ecosystem-wide
//! "Sign in with SBOL". Dynamic client registration gives public clients a
//! standards-defined bootstrap path without shared secrets.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, PRAGMA,
    WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{Duration, Utc};
use sbol_db_core::{DomainError, OAuthClient, User};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};
use url::Url;

use crate::v2::auth::Identity;
use crate::AppState;

pub(crate) const SCOPE_READ: &str = "sbol:read";
pub(crate) const SCOPE_WRITE: &str = "sbol:write";
pub(crate) const SCOPE_SHARE: &str = "sbol:share";
pub(crate) const SCOPE_REVIEW: &str = "sbol:review";
pub(crate) const SCOPE_OPENID: &str = "openid";
pub(crate) const SCOPE_PROFILE: &str = "profile";
pub(crate) const SCOPE_EMAIL: &str = "email";
pub(crate) const SUPPORTED_SCOPES: &[&str] = &[
    SCOPE_OPENID,
    SCOPE_PROFILE,
    SCOPE_EMAIL,
    SCOPE_READ,
    SCOPE_WRITE,
    SCOPE_SHARE,
    SCOPE_REVIEW,
];
const SBOL_SCOPES: &[&str] = &[SCOPE_READ, SCOPE_WRITE, SCOPE_SHARE, SCOPE_REVIEW];

pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/api/v2",
            get(api_protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route(
            "/.well-known/openid-configuration",
            get(openid_configuration),
        )
        .route("/oauth/register", post(register_client))
        .route(
            "/oauth/authorize",
            get(authorize_page).post(authorize_decision),
        )
        .route("/oauth/token", post(exchange_token))
        .route("/oauth/revoke", post(revoke_token))
        .route("/oauth/jwks", get(jwks))
        .route("/oauth/userinfo", get(userinfo).post(userinfo))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            crate::v2::auth::attach_browser_identity,
        ))
        // OAuth token, registration, discovery, JWKS, and UserInfo responses
        // are consumed by public browser clients. They carry no ambient
        // cookie authority, so wildcard CORS is safe and enables PKCE SPAs.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([AUTHORIZATION, CONTENT_TYPE]),
        )
}

async fn protected_resource_metadata(State(state): State<AppState>) -> Response {
    let Some(resource) = mcp_resource(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    let Some(issuer) = issuer(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    no_store_json(json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        // The 401 challenge asks only for read. Tool calls use incremental
        // scope challenges to request the least additional access.
        "scopes_supported": SBOL_SCOPES
    }))
}

async fn api_protected_resource_metadata(State(state): State<AppState>) -> Response {
    let Some(resource) = api_resource(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    let Some(issuer) = issuer(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    no_store_json(json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": SBOL_SCOPES
    }))
}

async fn authorization_server_metadata(State(state): State<AppState>) -> Response {
    let Some(issuer) = issuer(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    no_store_json(json!({
        "issuer": issuer,
        "authorization_endpoint": absolute(&issuer, "/oauth/authorize"),
        "token_endpoint": absolute(&issuer, "/oauth/token"),
        "registration_endpoint": absolute(&issuer, "/oauth/register"),
        "revocation_endpoint": absolute(&issuer, "/oauth/revoke"),
        "jwks_uri": absolute(&issuer, "/oauth/jwks"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": SUPPORTED_SCOPES,
        "client_id_metadata_document_supported": false,
        "service_documentation": absolute(&issuer, "/connect#mcp")
    }))
}

async fn openid_configuration(State(state): State<AppState>) -> Response {
    let Some(issuer) = issuer(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    no_store_json(json!({
        "issuer": issuer,
        "authorization_endpoint": absolute(&issuer, "/oauth/authorize"),
        "token_endpoint": absolute(&issuer, "/oauth/token"),
        "userinfo_endpoint": absolute(&issuer, "/oauth/userinfo"),
        "jwks_uri": absolute(&issuer, "/oauth/jwks"),
        "registration_endpoint": absolute(&issuer, "/oauth/register"),
        "revocation_endpoint": absolute(&issuer, "/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["EdDSA"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": SUPPORTED_SCOPES,
        "claims_supported": [
            "iss", "sub", "aud", "exp", "iat", "nonce",
            "preferred_username", "name", "email", "affiliation"
        ],
        "service_documentation": absolute(&issuer, "/connect#identity")
    }))
}

async fn jwks(State(state): State<AppState>) -> Response {
    no_store_json(json!({ "keys": [state.config.identity_signing_key.jwk()] }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RegistrationRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    token_endpoint_auth_method: Option<String>,
}

async fn register_client(
    State(state): State<AppState>,
    Json(request): Json<RegistrationRequest>,
) -> Response {
    if let Err(message) = validate_registration(&request) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", &message);
    }
    match state
        .app
        .oauth
        .register_public_client(request.client_name.trim().to_owned(), request.redirect_uris)
        .await
    {
        Ok(client) => (
            StatusCode::CREATED,
            [(CACHE_CONTROL, "no-store"), (PRAGMA, "no-cache")],
            Json(json!({
                "client_id": client.client_id,
                "client_name": client.client_name,
                "redirect_uris": client.redirect_uris,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none"
            })),
        )
            .into_response(),
        Err(error) => internal_oauth_error(error),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AuthorizationRequest {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
}

async fn authorize_page(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    uri: axum::http::Uri,
    Query(request): Query<AuthorizationRequest>,
) -> Response {
    let validated = match validate_authorization_request(&state, &request).await {
        Ok(validated) => validated,
        Err(response) => return response,
    };
    let Some(user) = identity.0 else {
        let next = uri
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_else(|| "/oauth/authorize".to_owned());
        let query = serde_urlencoded::to_string([("next", next.as_str())])
            .expect("one local query pair always serializes");
        return redirect(&format!("/login?{query}"));
    };
    consent_page(&user, &validated.client, &request, &validated.scopes)
}

#[derive(Debug, Deserialize)]
struct AuthorizationDecision {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
    decision: String,
}

async fn authorize_decision(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: AuthorizationDecision = match parse_form_or_json(&headers, &body) {
        Ok(request) => request,
        Err(message) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", &message),
    };
    let authorization = AuthorizationRequest {
        response_type: request.response_type,
        client_id: request.client_id,
        redirect_uri: request.redirect_uri,
        code_challenge: request.code_challenge,
        code_challenge_method: request.code_challenge_method,
        resource: request.resource,
        scope: request.scope,
        state: request.state,
        nonce: request.nonce,
    };
    let validated = match validate_authorization_request(&state, &authorization).await {
        Ok(validated) => validated,
        Err(response) => return response,
    };
    let Some(user) = identity.0 else {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            "login_required",
            "the SBOL DB browser session expired",
        );
    };
    if request.decision != "allow" {
        return authorization_redirect(
            &authorization.redirect_uri,
            "access_denied",
            "the resource owner declined access",
            authorization.state.as_deref(),
        );
    }
    let code = match state
        .app
        .oauth
        .issue_authorization_code(
            user.id,
            &authorization.client_id,
            &authorization.redirect_uri,
            &validated.resource,
            validated.scopes,
            &authorization.code_challenge,
            authorization.nonce,
        )
        .await
    {
        Ok(code) => code,
        Err(error) => return internal_oauth_error(error),
    };
    let mut location = Url::parse(&authorization.redirect_uri)
        .expect("registered redirect URI was validated before persistence");
    location.query_pairs_mut().append_pair("code", &code);
    if let Some(state) = authorization.state.as_deref() {
        location.query_pairs_mut().append_pair("state", state);
    }
    redirect(location.as_str())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
    scope: Option<String>,
}

async fn exchange_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_form(&headers) {
        return oauth_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "the token endpoint requires application/x-www-form-urlencoded",
        );
    }
    let request: TokenRequest = match serde_urlencoded::from_bytes(&body) {
        Ok(request) => request,
        Err(error) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &format!("invalid token request: {error}"),
            )
        }
    };
    if !matches!(
        request.grant_type.as_str(),
        "authorization_code" | "refresh_token"
    ) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "supported grants are authorization_code and refresh_token",
        );
    }
    let result = fulfill_token_request(&state, &request).await;
    match result {
        Ok(pair) => match token_response(&state, pair).await {
            Ok(response) => no_store_json(response),
            Err(error) => internal_oauth_error(error),
        },
        Err(DomainError::Validation(message)) | Err(DomainError::InvalidInput(message)) => {
            oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", &message)
        }
        Err(error) => internal_oauth_error(error),
    }
}

async fn fulfill_token_request(
    state: &AppState,
    request: &TokenRequest,
) -> Result<sbol_db_app::OAuthTokenPair, DomainError> {
    match request.grant_type.as_str() {
        "authorization_code" => {
            state
                .app
                .oauth
                .exchange_authorization_code(
                    required(request.code.as_deref(), "code")?,
                    required(request.client_id.as_deref(), "client_id")?,
                    required(request.redirect_uri.as_deref(), "redirect_uri")?,
                    request.resource.as_deref(),
                    required(request.code_verifier.as_deref(), "code_verifier")?,
                )
                .await
        }
        "refresh_token" => {
            let scopes = request.scope.as_deref().map(parse_scopes);
            state
                .app
                .oauth
                .refresh(
                    required(request.refresh_token.as_deref(), "refresh_token")?,
                    required(request.client_id.as_deref(), "client_id")?,
                    request.resource.as_deref(),
                    scopes,
                )
                .await
        }
        _ => unreachable!("grant type checked before fulfillment"),
    }
}

async fn token_response(
    state: &AppState,
    pair: sbol_db_app::OAuthTokenPair,
) -> Result<Value, DomainError> {
    let id_token = if has_scope(&pair.scopes, SCOPE_OPENID) {
        let user = state
            .app
            .users
            .get_by_id(pair.user_id)
            .await?
            .ok_or_else(|| {
                DomainError::Validation("the authorizing account no longer exists".to_owned())
            })?;
        let issuer = issuer(state).ok_or_else(|| {
            DomainError::Unavailable("the instance has no canonical public origin".to_owned())
        })?;
        let now = Utc::now();
        let mut claims = identity_claims(&user, &pair.scopes);
        let claims = claims
            .as_object_mut()
            .expect("identity claims are always a JSON object");
        claims.insert("iss".to_owned(), json!(issuer));
        claims.insert("aud".to_owned(), json!(pair.client_id));
        claims.insert("iat".to_owned(), json!(now.timestamp()));
        claims.insert(
            "exp".to_owned(),
            json!((now + Duration::minutes(5)).timestamp()),
        );
        if let Some(nonce) = pair.nonce {
            claims.insert("nonce".to_owned(), json!(nonce));
        }
        Some(
            state
                .config
                .identity_signing_key
                .sign_claims(&Value::Object(claims.clone()))
                .map_err(DomainError::Serialization)?,
        )
    } else {
        None
    };

    let mut response = json!({
        "access_token": pair.access_token,
        "token_type": "Bearer",
        "expires_in": pair.expires_in,
        "refresh_token": pair.refresh_token,
        "scope": pair.scopes.join(" "),
        "resource": pair.resource
    });
    if let Some(id_token) = id_token {
        response
            .as_object_mut()
            .expect("token response is always an object")
            .insert("id_token".to_owned(), json!(id_token));
    }
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct RevokeRequest {
    token: String,
    #[allow(dead_code)]
    token_type_hint: Option<String>,
}

async fn revoke_token(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_form(&headers) {
        return oauth_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "the revocation endpoint requires application/x-www-form-urlencoded",
        );
    }
    let request: RevokeRequest = match serde_urlencoded::from_bytes(&body) {
        Ok(request) => request,
        Err(error) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &format!("invalid revocation request: {error}"),
            )
        }
    };
    match state.app.oauth.revoke(&request.token).await {
        Ok(()) => (StatusCode::OK, [(CACHE_CONTROL, "no-store")]).into_response(),
        Err(error) => internal_oauth_error(error),
    }
}

async fn userinfo(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(resource) = userinfo_resource(&state) else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        );
    };
    let Some(token) = bearer_token(&headers) else {
        return userinfo_challenge("invalid_token", "a bearer access token is required");
    };
    let grant = match state.app.oauth.resolve_access_token(token, &resource).await {
        Ok(Some(grant)) if has_scope(&grant.scopes, SCOPE_OPENID) => grant,
        Ok(_) => {
            return userinfo_challenge(
                "invalid_token",
                "the token is expired, revoked, or not issued for SBOL UserInfo",
            )
        }
        Err(error) => return internal_oauth_error(error),
    };
    let user = match state.app.users.get_by_id(grant.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return userinfo_challenge(
                "invalid_token",
                "the account associated with this token no longer exists",
            )
        }
        Err(error) => return internal_oauth_error(error),
    };
    no_store_json(identity_claims(&user, &grant.scopes))
}

fn identity_claims(user: &User, scopes: &[String]) -> Value {
    let mut claims = serde_json::Map::new();
    claims.insert("sub".to_owned(), json!(user.id.to_string()));
    if has_scope(scopes, SCOPE_PROFILE) {
        claims.insert(
            "preferred_username".to_owned(),
            json!(user.username.clone()),
        );
        claims.insert("name".to_owned(), json!(user.name.clone()));
        if let Some(affiliation) = &user.affiliation {
            claims.insert("affiliation".to_owned(), json!(affiliation));
        }
    }
    if has_scope(scopes, SCOPE_EMAIL) {
        claims.insert("email".to_owned(), json!(user.email.clone()));
    }
    Value::Object(claims)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let authorization = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = authorization.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn userinfo_challenge(error: &str, description: &str) -> Response {
    let mut response = oauth_error(StatusCode::UNAUTHORIZED, error, description);
    let challenge = format!(
        "Bearer realm=\"SBOL Identity\", error=\"{}\", error_description=\"{}\"",
        error,
        description.replace(['"', '\\'], "")
    );
    if let Ok(challenge) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
    }
    response
}

struct ValidatedAuthorization {
    client: OAuthClient,
    scopes: Vec<String>,
    resource: String,
}

async fn validate_authorization_request(
    state: &AppState,
    request: &AuthorizationRequest,
) -> Result<ValidatedAuthorization, Response> {
    if request.response_type != "code" {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_response_type",
            "response_type must be code",
        ));
    }
    if request.code_challenge_method != "S256" || request.code_challenge.is_empty() {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "authorization code requests require PKCE with code_challenge_method=S256",
        ));
    }
    if request.code_challenge.len() != 43
        || !request
            .code_challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "code_challenge must be a base64url-encoded SHA-256 digest",
        ));
    }
    let client = match state.app.oauth.client(&request.client_id).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return Err(oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "unknown OAuth client",
            ))
        }
        Err(error) => return Err(internal_oauth_error(error)),
    };
    if !client
        .redirect_uris
        .iter()
        .any(|uri| uri == &request.redirect_uri)
    {
        return Err(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri is not registered for this client",
        ));
    }
    let scopes = request
        .scope
        .as_deref()
        .map(parse_scopes)
        .unwrap_or_else(|| vec![SCOPE_READ.to_owned()]);
    if scopes.is_empty()
        || scopes
            .iter()
            .any(|scope| !SUPPORTED_SCOPES.contains(&scope.as_str()))
    {
        return Err(authorization_redirect(
            &request.redirect_uri,
            "invalid_scope",
            "one or more requested SBOL Identity scopes are unsupported",
            request.state.as_deref(),
        ));
    }
    let openid = has_scope(&scopes, SCOPE_OPENID);
    if !openid && (has_scope(&scopes, SCOPE_PROFILE) || has_scope(&scopes, SCOPE_EMAIL)) {
        return Err(authorization_redirect(
            &request.redirect_uri,
            "invalid_scope",
            "profile and email require the openid scope",
            request.state.as_deref(),
        ));
    }
    let Some(mcp_resource) = mcp_resource(state) else {
        return Err(oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        ));
    };
    let Some(userinfo_resource) = userinfo_resource(state) else {
        return Err(oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        ));
    };
    let Some(api_resource) = api_resource(state) else {
        return Err(oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the instance has no canonical public origin",
        ));
    };
    let resource = match request.resource.as_deref() {
        Some(resource)
            if resource == mcp_resource
                || resource == api_resource
                || resource == userinfo_resource =>
        {
            resource.to_owned()
        }
        None if openid => userinfo_resource.clone(),
        None => {
            return Err(oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                "delegated SBOL API and MCP requests must identify their protected resource",
            ))
        }
        Some(_) => {
            return Err(oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                "the requested resource is not served by this SBOL Identity provider",
            ))
        }
    };
    if resource == userinfo_resource
        && (!openid
            || scopes
                .iter()
                .any(|scope| SBOL_SCOPES.contains(&scope.as_str())))
    {
        return Err(authorization_redirect(
            &request.redirect_uri,
            "invalid_scope",
            "SBOL design scopes require the API or MCP resource; UserInfo accepts identity scopes only",
            request.state.as_deref(),
        ));
    }
    if (resource == mcp_resource || resource == api_resource)
        && !scopes
            .iter()
            .any(|scope| SBOL_SCOPES.contains(&scope.as_str()))
    {
        return Err(authorization_redirect(
            &request.redirect_uri,
            "invalid_scope",
            "SBOL DB API and MCP resources require at least one SBOL design scope",
            request.state.as_deref(),
        ));
    }
    Ok(ValidatedAuthorization {
        client,
        scopes,
        resource,
    })
}

fn validate_registration(request: &RegistrationRequest) -> Result<(), String> {
    if request.client_name.trim().is_empty() || request.client_name.trim().len() > 120 {
        return Err("client_name must contain 1 to 120 characters".to_owned());
    }
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > 10 {
        return Err("redirect_uris must contain between 1 and 10 entries".to_owned());
    }
    for redirect_uri in &request.redirect_uris {
        validate_redirect_uri(redirect_uri)?;
    }
    if request.grant_types.as_ref().is_some_and(|values| {
        values
            .iter()
            .any(|value| value != "authorization_code" && value != "refresh_token")
    }) {
        return Err("only authorization_code and refresh_token grants are supported".to_owned());
    }
    if request
        .response_types
        .as_ref()
        .is_some_and(|values| values.iter().any(|value| value != "code"))
    {
        return Err("only the code response type is supported".to_owned());
    }
    if request
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|value| value != "none")
    {
        return Err("public clients must use token_endpoint_auth_method=none".to_owned());
    }
    Ok(())
}

fn validate_redirect_uri(value: &str) -> Result<(), String> {
    let uri = Url::parse(value).map_err(|_| "redirect URI is not an absolute URL".to_owned())?;
    if uri.fragment().is_some() || !uri.username().is_empty() || uri.password().is_some() {
        return Err("redirect URIs cannot contain fragments or credentials".to_owned());
    }
    if uri.scheme() == "https" {
        return Ok(());
    }
    let loopback = matches!(uri.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if uri.scheme() == "http" && loopback {
        return Ok(());
    }
    Err("redirect URIs must use HTTPS or HTTP on a loopback host".to_owned())
}

fn parse_scopes(value: &str) -> Vec<String> {
    let mut scopes = value
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

fn has_scope(scopes: &[String], expected: &str) -> bool {
    scopes.iter().any(|scope| scope == expected)
}

fn consent_page(
    user: &User,
    client: &OAuthClient,
    request: &AuthorizationRequest,
    scopes: &[String],
) -> Response {
    let scope_rows = scopes
        .iter()
        .map(|scope| {
            let (title, description) = scope_description(scope);
            format!(
                "<li><strong>{}</strong><span>{}</span></li>",
                escape_html(title),
                escape_html(description)
            )
        })
        .collect::<String>();
    let mut hidden = vec![
        ("response_type", request.response_type.as_str()),
        ("client_id", request.client_id.as_str()),
        ("redirect_uri", request.redirect_uri.as_str()),
        ("code_challenge", request.code_challenge.as_str()),
        (
            "code_challenge_method",
            request.code_challenge_method.as_str(),
        ),
    ];
    if let Some(resource) = request.resource.as_deref() {
        hidden.push(("resource", resource));
    }
    if let Some(scope) = request.scope.as_deref() {
        hidden.push(("scope", scope));
    }
    if let Some(state) = request.state.as_deref() {
        hidden.push(("state", state));
    }
    if let Some(nonce) = request.nonce.as_deref() {
        hidden.push(("nonce", nonce));
    }
    let hidden = hidden
        .into_iter()
        .map(|(name, value)| {
            format!(
                "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
                escape_html(name),
                escape_html(value)
            )
        })
        .collect::<String>();
    let redirect_host = Url::parse(&request.redirect_uri)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| request.redirect_uri.clone());
    let body = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Authorize {client_name} · SBOL Identity</title>
<style>
:root{{color-scheme:light dark;font-family:Inter,ui-sans-serif,system-ui,sans-serif;background:#07110f;color:#ecfdf5}}*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:radial-gradient(circle at 80% 0%,#0f766e33,transparent 35%),#07110f;padding:24px}}main{{width:min(560px,100%);border:1px solid #ffffff1a;border-radius:24px;background:#0d1b18;box-shadow:0 24px 80px #0008;overflow:hidden}}header{{padding:28px 30px 22px;border-bottom:1px solid #ffffff12}}.eyebrow{{margin:0 0 10px;color:#5eead4;font-size:12px;font-weight:700;letter-spacing:.16em;text-transform:uppercase}}h1{{font-size:27px;line-height:1.15;margin:0;letter-spacing:-.025em}}.lede{{color:#a7bdb6;line-height:1.55;margin:12px 0 0}}section{{padding:24px 30px}}.account{{display:flex;justify-content:space-between;gap:16px;border:1px solid #ffffff12;background:#ffffff08;border-radius:14px;padding:14px 16px;font-size:13px}}.muted{{color:#8da49d}}ul{{list-style:none;margin:20px 0 0;padding:0;border:1px solid #ffffff12;border-radius:14px;overflow:hidden}}li{{padding:14px 16px;border-bottom:1px solid #ffffff12;display:grid;gap:4px}}li:last-child{{border:0}}li strong{{font-size:14px}}li span{{font-size:13px;color:#9bb0aa;line-height:1.45}}.notice{{font-size:12px;color:#80978f;line-height:1.5;margin:18px 2px}}footer{{display:flex;gap:10px;padding:0 30px 28px}}button{{flex:1;border-radius:12px;padding:12px 16px;font:inherit;font-weight:700;cursor:pointer}}.deny{{border:1px solid #ffffff1f;background:transparent;color:#d1e4dd}}.allow{{border:1px solid #5eead4;background:#5eead4;color:#05201a}}code{{font-size:12px;color:#99f6e4}}
</style></head><body><main><header><p class="eyebrow">SBOL Identity</p><h1>Allow {client_name} to work with your designs?</h1><p class="lede">Review what this application is asking to do as <strong>{user_name}</strong>.</p></header><section><div class="account"><span><span class="muted">Signed in as</span><br>{username}</span><span><span class="muted">Returning to</span><br><code>{redirect_host}</code></span></div><ul>{scope_rows}</ul><p class="notice">SBOL DB applies your existing ownership and sharing rules to every request. You can revoke this connection later.</p></section><form method="post" action="/oauth/authorize">{hidden}<footer><button class="deny" name="decision" value="deny">Cancel</button><button class="allow" name="decision" value="allow">Allow access</button></footer></form></main></body></html>"#,
        client_name = escape_html(&client.client_name),
        user_name = escape_html(&user.name),
        username = escape_html(&user.username),
        redirect_host = escape_html(&redirect_host),
    );
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn scope_description(scope: &str) -> (&'static str, &'static str) {
    match scope {
        SCOPE_OPENID => (
            "Confirm your SBOL identity",
            "Let this application identify your stable SBOL account.",
        ),
        SCOPE_PROFILE => (
            "Read your basic profile",
            "Share your display name, username, and affiliation.",
        ),
        SCOPE_EMAIL => (
            "Read your email address",
            "Share the email address associated with your SBOL account.",
        ),
        SCOPE_READ => (
            "Find and inspect designs",
            "Search, open, download, and compare public, shared, and private designs visible to you.",
        ),
        SCOPE_WRITE => (
            "Prepare and publish changes",
            "Validate, upload, edit, and publish designs after explicit confirmation.",
        ),
        SCOPE_SHARE => (
            "Manage design sharing",
            "Grant or revoke another SBOL account's read access to designs you own.",
        ),
        SCOPE_REVIEW => (
            "Participate in reviews",
            "Start reviews, record curator decisions, and inspect review activity available to you.",
        ),
        _ => ("Use SBOL DB", "Use the requested SBOL Identity permission."),
    }
}

fn parse_form_or_json<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, String> {
    if is_form(headers) {
        serde_urlencoded::from_bytes(body).map_err(|error| error.to_string())
    } else if headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
    {
        serde_json::from_slice(body).map_err(|error| error.to_string())
    } else {
        Err("expected application/x-www-form-urlencoded or application/json".to_owned())
    }
}

fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        == Some("application/x-www-form-urlencoded")
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, DomainError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DomainError::InvalidInput(format!("{name} is required")))
}

pub(crate) fn mcp_resource(state: &AppState) -> Option<String> {
    issuer(state).map(|issuer| absolute(&issuer, "/mcp"))
}

pub(crate) fn api_resource(state: &AppState) -> Option<String> {
    issuer(state).map(|issuer| absolute(&issuer, "/api/v2"))
}

fn userinfo_resource(state: &AppState) -> Option<String> {
    issuer(state).map(|issuer| absolute(&issuer, "/oauth/userinfo"))
}

pub(crate) fn protected_resource_metadata_url(state: &AppState) -> Option<String> {
    issuer(state).map(|issuer| absolute(&issuer, "/.well-known/oauth-protected-resource/mcp"))
}

pub(crate) fn api_protected_resource_metadata_url(state: &AppState) -> Option<String> {
    issuer(state).map(|issuer| absolute(&issuer, "/.well-known/oauth-protected-resource/api/v2"))
}

fn issuer(state: &AppState) -> Option<String> {
    state
        .config
        .public_origin
        .as_deref()
        .map(|origin| origin.trim_end_matches('/').to_owned())
}

fn absolute(origin: &str, path: &str) -> String {
    format!("{}{}", origin.trim_end_matches('/'), path)
}

fn authorization_redirect(
    redirect_uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> Response {
    let Ok(mut location) = Url::parse(redirect_uri) else {
        return oauth_error(StatusCode::BAD_REQUEST, error, description);
    };
    location
        .query_pairs_mut()
        .append_pair("error", error)
        .append_pair("error_description", description);
    if let Some(state) = state {
        location.query_pairs_mut().append_pair("state", state);
    }
    redirect(location.as_str())
}

fn redirect(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(location) => (StatusCode::FOUND, [(LOCATION, location)]).into_response(),
        Err(_) => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect target is not a valid HTTP header value",
        ),
    }
}

fn no_store_json(value: Value) -> Response {
    (
        [(CACHE_CONTROL, "no-store"), (PRAGMA, "no-cache")],
        Json(value),
    )
        .into_response()
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        [(CACHE_CONTROL, "no-store"), (PRAGMA, "no-cache")],
        Json(json!({
            "error": error,
            "error_description": description
        })),
    )
        .into_response()
}

fn internal_oauth_error(error: DomainError) -> Response {
    tracing::error!(%error, "SBOL Identity operation failed");
    oauth_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "temporarily_unavailable",
        "SBOL Identity is temporarily unavailable",
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_require_https_or_loopback() {
        assert!(validate_redirect_uri("https://agent.example/oauth/callback").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:43123/callback").is_ok());
        assert!(validate_redirect_uri("http://localhost:43123/callback").is_ok());
        assert!(validate_redirect_uri("http://agent.example/callback").is_err());
        assert!(validate_redirect_uri("https://user:secret@agent.example/callback").is_err());
        assert!(validate_redirect_uri("https://agent.example/callback#token").is_err());
    }

    #[test]
    fn scope_parser_is_deterministic() {
        assert_eq!(
            parse_scopes("sbol:write  sbol:read sbol:read"),
            vec!["sbol:read", "sbol:write"]
        );
    }
}
