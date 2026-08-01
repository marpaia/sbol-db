//! SBOL Identity-authorized, stateless MCP Streamable HTTP adapter.
//!
//! Every credential is an OAuth access token issued specifically for this MCP
//! resource. Tool calls combine those granted scopes with the same application
//! services and ACL model used by the V2 and compatibility APIs.

use std::collections::BTreeSet;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use sbol_db_app::{
    AlignMode, AlignOptions, DiscoveryQuery, FieldValue, MakePublicRequest, ReviewDecision,
    SubmitRequest,
};
use sbol_db_core::{IriString, OAuthAccessToken, SerializationFormat, User};
use sbol_db_storage::ImportOverwrite;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::identity::{
    mcp_resource, protected_resource_metadata_url, SCOPE_READ, SCOPE_REVIEW, SCOPE_SHARE,
    SCOPE_WRITE,
};
use crate::v2::download::{serialize_download, DownloadFormat};
use crate::AppState;

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_EMBEDDED_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/mcp", get(get_stream).post(post_message))
}

/// This stateless transport does not open unsolicited SSE streams. Returning
/// 405 is the transport-defined way to tell a client to use one POST per
/// request.
async fn get_stream(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_is_allowed(&state, &headers) {
        return http_error(
            StatusCode::FORBIDDEN,
            "the request Origin does not match this SBOL DB instance",
        );
    }
    let principal = match authenticated_principal(&state, &headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if !principal.has_scopes(&[SCOPE_READ]) {
        return insufficient_scope(&state, &[SCOPE_READ]);
    }
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "POST")],
        "this SBOL DB MCP server is stateless; send JSON-RPC requests with POST",
    )
        .into_response()
}

async fn post_message(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !origin_is_allowed(&state, &headers) {
        return http_error(
            StatusCode::FORBIDDEN,
            "the request Origin does not match this SBOL DB instance",
        );
    }
    if !accepts_streamable_http(&headers) {
        return http_error(
            StatusCode::NOT_ACCEPTABLE,
            "MCP requests must accept application/json and text/event-stream",
        );
    }
    if !is_json(&headers) {
        return http_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "MCP request content type must be application/json",
        );
    }
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
    {
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
            return http_error(
                StatusCode::BAD_REQUEST,
                "unsupported MCP-Protocol-Version header",
            );
        }
    }
    let principal = match authenticated_principal(&state, &headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };

    let message: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(message)) => Value::Object(message),
        Ok(_) => return json_rpc_error(Value::Null, -32600, "request must be a JSON object"),
        Err(error) => {
            return json_rpc_error(Value::Null, -32700, &format!("invalid JSON: {error}"))
        }
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return json_rpc_error(Value::Null, -32600, "jsonrpc must equal 2.0");
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return json_rpc_error(Value::Null, -32600, "method is required");
    };
    let Some(id) = message.get("id").cloned() else {
        // Notifications carry no response body. This includes the normal
        // notifications/initialized lifecycle message.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    if !principal.has_scopes(&[SCOPE_READ]) {
        return insufficient_scope(&state, &[SCOPE_READ]);
    }

    let result = match method {
        "initialize" => initialize(&params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return json_rpc_error(id, -32602, "tools/call requires a tool name");
            };
            let required = required_scopes(name);
            if !principal.has_scopes(required) {
                return insufficient_scope(&state, required);
            }
            call_tool(&state, &principal.user, params).await
        }
        _ => return json_rpc_error(id, -32601, "method not found"),
    };
    match result {
        Ok(result) => json_rpc_result(id, result),
        Err(DispatchError::Protocol(message)) => json_rpc_error(id, -32602, &message),
        Err(DispatchError::Tool(message)) => json_rpc_result(id, tool_error_result(message)),
    }
}

fn initialize(params: &Value) -> Result<Value, DispatchError> {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Protocol("protocolVersion is required".to_owned()))?;
    let protocol_version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        LATEST_PROTOCOL_VERSION
    };
    Ok(json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "sbol-db",
            "title": "SBOL DB",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Permission-aware biological design discovery, contribution, sharing, and review"
        },
        "instructions": "Use search_designs before get_design when you do not already know a canonical IRI. Every result and mutation is scoped to the signed-in SBOL Identity account. Preview uploads before committing them, and do not set a write tool's confirm flag until the user has reviewed its stated effect."
    }))
}

async fn call_tool(state: &AppState, user: &User, params: Value) -> Result<Value, DispatchError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Protocol("tools/call requires a tool name".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "search_designs" => search_designs(state, user, arguments).await,
        "get_design" => get_design(state, user, arguments).await,
        "download_design" => download_design(state, user, arguments).await,
        "search_sequences" => search_sequences(state, user, arguments).await,
        "find_similar_designs" => find_similar_designs(state, user, arguments).await,
        "validate_design_upload" => validate_design_upload(state, user, arguments).await,
        "upload_design_collection" => upload_design_collection(state, user, arguments).await,
        "update_design_metadata" => update_design_metadata(state, user, arguments).await,
        "publish_design" => publish_design(state, user, arguments).await,
        "list_design_collaborators" => list_design_collaborators(state, user, arguments).await,
        "share_design" => share_design(state, user, arguments).await,
        "list_reviews" => list_reviews(state, user, arguments).await,
        "start_design_review" => start_design_review(state, user, arguments).await,
        "record_review_decision" => record_review_decision(state, user, arguments).await,
        "get_design_activity" => get_design_activity(state, user, arguments).await,
        _ => Err(DispatchError::Protocol(format!("unknown tool `{name}`"))),
    }
}

async fn search_designs(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: SearchDesignsArgs = tool_arguments(arguments)?;
    let limit = arguments.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(DispatchError::Tool(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    let page = state
        .app
        .discover(
            &DiscoveryQuery {
                text: non_empty(arguments.query),
                object_type: non_empty(arguments.object_type),
                role: non_empty(arguments.role),
                offset: arguments.offset.unwrap_or(0),
                limit,
                ..DiscoveryQuery::default()
            },
            scope,
        )
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    let summary = if page.items.is_empty() {
        "No visible designs matched the request.".to_owned()
    } else {
        let names = page
            .items
            .iter()
            .take(5)
            .map(|hit| {
                hit.name
                    .as_deref()
                    .or(hit.display_id.as_deref())
                    .unwrap_or(hit.uri.as_str())
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Found {} visible designs; returning {}. First results: {names}",
            page.total,
            page.items.len()
        )
    };
    structured_tool_result(summary, serde_json::to_value(page))
}

async fn get_design(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: GetDesignArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    let Some(details) = state
        .app
        .object_details(iri.as_str(), scope)
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?
    else {
        return Err(DispatchError::Tool(
            "design was not found or is not visible to this account".to_owned(),
        ));
    };
    let title = details
        .name
        .as_deref()
        .or(details.display_id.as_deref())
        .unwrap_or(details.iri.as_str());
    structured_tool_result(
        format!("Opened {title} ({})", details.iri),
        serde_json::to_value(details),
    )
}

async fn download_design(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: DownloadDesignArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let (format, sbol2, binary) = parse_download_format(&arguments.format)?;
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let serialized = serialize_download(state, iri.as_str(), format, sbol2, scope)
        .await
        .map_err(tool_error)?;
    let byte_length = serialized.bytes.len();
    if byte_length > MAX_EMBEDDED_DOWNLOAD_BYTES {
        return Err(DispatchError::Tool(format!(
            "the rendered design is {} bytes, above the MCP inline limit of {}; use the authenticated REST download endpoint for this artifact",
            byte_length,
            MAX_EMBEDDED_DOWNLOAD_BYTES
        )));
    }
    let content_type = serialized.content_type;
    let (encoding, content) = if binary {
        ("base64", BASE64_STANDARD.encode(&serialized.bytes))
    } else {
        let content = String::from_utf8(serialized.bytes).map_err(|_| {
            DispatchError::Tool("the textual design was not valid UTF-8".to_owned())
        })?;
        ("utf-8", content)
    };
    structured_tool_result(
        format!("Downloaded {} as {}.", iri, arguments.format),
        Ok(json!({
            "iri": iri.as_str(),
            "format": arguments.format,
            "content_type": content_type,
            "encoding": encoding,
            "byte_length": byte_length,
            "content": content
        })),
    )
}

async fn search_sequences(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: SearchSequencesArgs = tool_arguments(arguments)?;
    if arguments.sequence.trim().is_empty() {
        return Err(DispatchError::Tool("sequence must not be empty".to_owned()));
    }
    let limit = bounded_limit(arguments.limit, 20, 100)?;
    let mode = match arguments.mode.as_deref().unwrap_or("global") {
        "global" => AlignMode::GlobalAlign,
        "substring" => AlignMode::Substring,
        "exact" => AlignMode::Exact,
        other => {
            return Err(DispatchError::Tool(format!(
                "unsupported alignment mode `{other}`"
            )))
        }
    };
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let hits = state
        .app
        .sequence()
        .align(
            arguments.sequence.trim(),
            AlignOptions {
                mode,
                max_accepts: limit as u32,
                ..AlignOptions::default()
            },
            &scope,
        )
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Found {} visible sequences with alignment evidence.",
            hits.len()
        ),
        Ok(json!({ "items": hits, "total": hits.len() })),
    )
}

async fn find_similar_designs(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: SimilarDesignsArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let limit = bounded_limit(arguments.limit, 20, 100)?;
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let mut hits = state
        .app
        .sequence()
        .similar(iri.as_str(), &scope)
        .await
        .map_err(tool_error)?;
    hits.truncate(limit);
    let items = hits
        .into_iter()
        .map(|hit| json!({ "iri": hit.iri, "pagerank": hit.pagerank }))
        .collect::<Vec<_>>();
    structured_tool_result(
        format!("Found {} visible cluster-related designs.", items.len()),
        Ok(json!({ "items": items, "total": items.len() })),
    )
}

async fn validate_design_upload(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: ValidateDesignArgs = tool_arguments(arguments)?;
    let request = submit_request(user, arguments)?;
    let preview = state
        .app
        .submission_service()
        .preview(&request)
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    let consequence = consequence_label(preview.consequence);
    let mut structured =
        serde_json::to_value(&preview).map_err(|error| DispatchError::Tool(error.to_string()))?;
    structured["confirmation"] = json!({
        "expected_collection_uri": preview.collection_uri,
        "expected_consequence": consequence
    });
    structured_tool_result(
        format!(
            "Upload is valid. It would {} {} with {} members and {} triples. No registry data was changed.",
            consequence,
            preview.collection_uri,
            preview.members.len(),
            preview.triple_count
        ),
        Ok(structured),
    )
}

async fn upload_design_collection(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    require_member(user, "upload a design")?;
    let arguments: UploadDesignArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the user reviews validate_design_upload".to_owned(),
        ));
    }
    let request = submit_request(user, arguments.design)?;
    let preview = state
        .app
        .submission_service()
        .preview(&request)
        .await
        .map_err(tool_error)?;
    if preview.collection_uri != arguments.expected_collection_uri
        || consequence_label(preview.consequence) != arguments.expected_consequence
    {
        return Err(DispatchError::Tool(
            "the live upload preview no longer matches the confirmed collection URI or consequence; preview again before committing"
                .to_owned(),
        ));
    }
    if preview.consequence == sbol_db_app::SubmitConsequence::RejectConflict {
        return Err(DispatchError::Tool(
            "the confirmed upload would be rejected by its fail-on-collision policy".to_owned(),
        ));
    }
    let outcome = state
        .app
        .submission_service()
        .submit(request)
        .await
        .map_err(tool_error)?;
    let result = json!({
        "collection_uri": outcome.collection_uri.as_str(),
        "persistent_identity": outcome.collection_persistent_identity.as_str(),
        "members": outcome.members.iter().map(|iri| iri.as_str()).collect::<Vec<_>>(),
        "graph": outcome.graph_iri,
        "triple_count": outcome.triple_count,
        "consequence": arguments.expected_consequence
    });
    structured_tool_result(
        format!(
            "Uploaded {} with {} members and {} triples.",
            outcome.collection_uri,
            outcome.members.len(),
            outcome.triple_count
        ),
        Ok(result),
    )
}

async fn update_design_metadata(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    require_member(user, "update a design")?;
    let arguments: UpdateDesignArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the user reviews the requested metadata changes".to_owned(),
        ));
    }
    if arguments.name.is_none()
        && arguments.description.is_none()
        && arguments.mutable_description.is_none()
        && arguments.mutable_notes.is_none()
        && arguments.mutable_source.is_none()
        && arguments.citations.is_none()
    {
        return Err(DispatchError::Tool(
            "at least one metadata field must be supplied".to_owned(),
        ));
    }
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let before = state
        .app
        .object_details(iri.as_str(), scope.clone())
        .await
        .map_err(tool_error)?
        .ok_or_else(|| {
            DispatchError::Tool("design was not found or is not visible to this account".to_owned())
        })?;
    if arguments
        .expected_name
        .as_ref()
        .is_some_and(|expected| before.name.as_ref() != Some(expected))
        || arguments
            .expected_description
            .as_ref()
            .is_some_and(|expected| before.description.as_ref() != Some(expected))
    {
        return Err(DispatchError::Tool(
            "the design metadata changed since it was inspected; open it again before updating"
                .to_owned(),
        ));
    }
    let service = state.app.edit_service();
    if let Some(value) = arguments.name.as_deref() {
        service
            .edit_field(
                &user.graph_uri,
                user.is_admin,
                iri.as_str(),
                DCTERMS_TITLE,
                &FieldValue::Literal(value.to_owned()),
                None,
            )
            .await
            .map_err(tool_error)?;
    }
    if let Some(value) = arguments.description.as_deref() {
        service
            .edit_field(
                &user.graph_uri,
                user.is_admin,
                iri.as_str(),
                DCTERMS_DESCRIPTION,
                &FieldValue::Literal(value.to_owned()),
                None,
            )
            .await
            .map_err(tool_error)?;
    }
    if let Some(value) = arguments.mutable_description.as_deref() {
        service
            .update_mutable_description(&user.graph_uri, user.is_admin, iri.as_str(), value)
            .await
            .map_err(tool_error)?;
    }
    if let Some(value) = arguments.mutable_notes.as_deref() {
        service
            .update_mutable_notes(&user.graph_uri, user.is_admin, iri.as_str(), value)
            .await
            .map_err(tool_error)?;
    }
    if let Some(value) = arguments.mutable_source.as_deref() {
        service
            .update_mutable_source(&user.graph_uri, user.is_admin, iri.as_str(), value)
            .await
            .map_err(tool_error)?;
    }
    if let Some(citations) = arguments.citations.as_ref() {
        service
            .update_citations(&user.graph_uri, user.is_admin, iri.as_str(), citations)
            .await
            .map_err(tool_error)?;
    }
    let after = state
        .app
        .object_details(iri.as_str(), scope)
        .await
        .map_err(tool_error)?
        .ok_or_else(|| DispatchError::Tool("updated design could not be read back".to_owned()))?;
    structured_tool_result(
        format!("Updated metadata for {}.", iri),
        serde_json::to_value(after),
    )
}

async fn publish_design(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    require_member(user, "publish a design")?;
    let arguments: PublishDesignArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the user reviews the public identity and collision policy"
                .to_owned(),
        ));
    }
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    if arguments.id.trim().is_empty() || arguments.version.trim().is_empty() {
        return Err(DispatchError::Tool(
            "public id and version must not be empty".to_owned(),
        ));
    }
    let request = MakePublicRequest {
        source_uri: iri.as_str().to_owned(),
        owner_username: user.username.clone(),
        public_id: arguments.id,
        version: arguments.version,
        name: arguments.name.and_then(non_blank),
        description: arguments.description.and_then(non_blank),
        creator_name: Some(user.name.clone()),
        citations: arguments.citations,
        overwrite: parse_collision_policy(arguments.collision.as_deref())?,
    };
    let outcome = state
        .app
        .mutation_service()
        .make_public(&user.graph_uri, user.is_admin, request)
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Published the stable public identity {}.",
            outcome.collection_uri
        ),
        Ok(json!({
            "collection_uri": outcome.collection_uri.as_str(),
            "members": outcome.members.iter().map(|iri| iri.as_str()).collect::<Vec<_>>(),
            "triple_count": outcome.triple_count
        })),
    )
}

async fn list_design_collaborators(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: DesignIriArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let details = state
        .app
        .object_details(iri.as_str(), scope)
        .await
        .map_err(tool_error)?
        .ok_or_else(|| DispatchError::Tool("design was not found".to_owned()))?;
    let viewers = state
        .app
        .object_viewer_graphs(iri.as_str())
        .await
        .map_err(tool_error)?;
    let owners = resolve_collaborators(state, details.owners).await?;
    let viewers = resolve_collaborators(state, viewers).await?;
    structured_tool_result(
        format!(
            "{} has {} owners and {} read-only collaborators.",
            iri,
            owners.len(),
            viewers.len()
        ),
        Ok(json!({ "iri": iri.as_str(), "owners": owners, "viewers": viewers })),
    )
}

async fn share_design(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    require_member(user, "manage design sharing")?;
    let arguments: ShareDesignArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the user reviews the recipient and sharing action"
                .to_owned(),
        ));
    }
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let target = resolve_member(state, &arguments.user).await?;
    if target.id == user.id {
        return Err(DispatchError::Tool(
            "the owner cannot be added or removed as a read-only collaborator".to_owned(),
        ));
    }
    match arguments.action.as_str() {
        "grant" => state
            .app
            .permission_service()
            .grant_view(
                &user.graph_uri,
                user.is_admin,
                iri.as_str(),
                &target.graph_uri,
            )
            .await
            .map_err(tool_error)?,
        "revoke" => state
            .app
            .permission_service()
            .revoke_view(
                &user.graph_uri,
                user.is_admin,
                iri.as_str(),
                &target.graph_uri,
            )
            .await
            .map_err(tool_error)?,
        other => {
            return Err(DispatchError::Tool(format!(
                "sharing action must be grant or revoke, got `{other}`"
            )))
        }
    }
    structured_tool_result(
        format!(
            "{} read access to {} for {}.",
            if arguments.action == "grant" {
                "Granted"
            } else {
                "Revoked"
            },
            iri,
            target.username
        ),
        Ok(json!({
            "iri": iri.as_str(),
            "action": arguments.action,
            "collaborator": collaborator_json(&target)
        })),
    )
}

async fn list_reviews(
    state: &AppState,
    user: &User,
    _arguments: Value,
) -> Result<Value, DispatchError> {
    let items = state
        .app
        .review_service()
        .list_for(&user.graph_uri, user.is_admin)
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Found {} review cases relevant to this account.",
            items.len()
        ),
        Ok(json!({ "items": items, "total": items.len() })),
    )
}

async fn start_design_review(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    require_member(user, "start a design review")?;
    let arguments: StartReviewArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the user reviews the design, curator, and note".to_owned(),
        ));
    }
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let curator = resolve_member(state, &arguments.curator).await?;
    let case = state
        .app
        .review_service()
        .request(
            &user.graph_uri,
            user.is_admin,
            iri.as_str(),
            &curator.graph_uri,
            curator.is_curator,
            arguments.note.as_deref(),
        )
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!("Started a review of {} with {}.", iri, curator.username),
        serde_json::to_value(case),
    )
}

async fn record_review_decision(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: ReviewDecisionArgs = tool_arguments(arguments)?;
    if !arguments.confirm {
        return Err(DispatchError::Tool(
            "confirm must be true after the curator reviews the decision and note".to_owned(),
        ));
    }
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    let decision = match arguments.decision.as_str() {
        "approve" => ReviewDecision::Approve,
        "request_changes" => ReviewDecision::RequestChanges,
        other => {
            return Err(DispatchError::Tool(format!(
                "decision must be approve or request_changes, got `{other}`"
            )))
        }
    };
    let case = state
        .app
        .review_service()
        .decide(
            &user.graph_uri,
            user.is_curator,
            user.is_admin,
            iri.as_str(),
            decision,
            arguments.note.as_deref(),
        )
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!("Recorded the {} decision for {}.", arguments.decision, iri),
        serde_json::to_value(case),
    )
}

async fn get_design_activity(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: DesignIriArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let items = state
        .app
        .audit_service()
        .for_object(iri.as_str())
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!("Found {} activity events for {}.", items.len(), iri),
        Ok(json!({ "iri": iri.as_str(), "items": items, "total": items.len() })),
    )
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SearchDesignsArgs {
    query: Option<String>,
    object_type: Option<String>,
    role: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Default for SearchDesignsArgs {
    fn default() -> Self {
        Self {
            query: None,
            object_type: None,
            role: None,
            offset: Some(0),
            limit: Some(20),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetDesignArgs {
    iri: String,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DownloadDesignArgs {
    iri: String,
    format: String,
}

impl Default for DownloadDesignArgs {
    fn default() -> Self {
        Self {
            iri: String::new(),
            format: "sbol3-turtle".to_owned(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SearchSequencesArgs {
    sequence: String,
    mode: Option<String>,
    limit: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SimilarDesignsArgs {
    iri: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(default)]
struct ValidateDesignArgs {
    id: String,
    version: String,
    name: Option<String>,
    description: Option<String>,
    creator_name: Option<String>,
    citations: Vec<String>,
    format: String,
    collision: Option<String>,
    content: String,
}

#[derive(Deserialize)]
struct UploadDesignArgs {
    #[serde(flatten)]
    design: ValidateDesignArgs,
    confirm: bool,
    expected_collection_uri: String,
    expected_consequence: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UpdateDesignArgs {
    iri: String,
    name: Option<String>,
    description: Option<String>,
    mutable_description: Option<String>,
    mutable_notes: Option<String>,
    mutable_source: Option<String>,
    citations: Option<Vec<String>>,
    expected_name: Option<String>,
    expected_description: Option<String>,
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PublishDesignArgs {
    iri: String,
    id: String,
    version: String,
    name: Option<String>,
    description: Option<String>,
    citations: Vec<String>,
    collision: Option<String>,
    confirm: bool,
}

impl Default for PublishDesignArgs {
    fn default() -> Self {
        Self {
            iri: String::new(),
            id: String::new(),
            version: "1".to_owned(),
            name: None,
            description: None,
            citations: Vec::new(),
            collision: None,
            confirm: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignIriArgs {
    iri: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ShareDesignArgs {
    iri: String,
    user: String,
    action: String,
    confirm: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StartReviewArgs {
    iri: String,
    curator: String,
    note: Option<String>,
    confirm: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReviewDecisionArgs {
    iri: String,
    decision: String,
    note: Option<String>,
    confirm: bool,
}

impl Default for ValidateDesignArgs {
    fn default() -> Self {
        Self {
            id: String::new(),
            version: "1".to_owned(),
            name: None,
            description: None,
            creator_name: None,
            citations: Vec::new(),
            format: "turtle".to_owned(),
            collision: None,
            content: String::new(),
        }
    }
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "search_designs",
            "title": "Search visible biological designs",
            "description": "Search public, shared, and private SBOL designs visible to the signed-in account by text, SBOL type, or biological role.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Free-text biological or metadata query." },
                    "object_type": { "type": "string", "format": "uri", "description": "Optional full rdf:type IRI." },
                    "role": { "type": "string", "format": "uri", "description": "Optional full biological role IRI." },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "get_design",
            "title": "Open a complete design record",
            "description": "Read the normalized biological, provenance, collection, citation, sequence, and collaboration context for one visible canonical design IRI.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri", "description": "Canonical design IRI returned by search_designs." }
                },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "download_design",
            "title": "Download a design",
            "description": "Render the complete ACL-scoped design closure as SBOL 2 or 3, GenBank, FASTA, GFF3, or OMEX. Text is returned as UTF-8 and OMEX as base64.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "format": {
                        "type": "string",
                        "enum": ["sbol3-rdfxml", "sbol3-turtle", "sbol3-jsonld", "sbol3-ntriples", "sbol2-rdfxml", "sbol2-turtle", "genbank", "fasta", "gff3", "omex"],
                        "default": "sbol3-turtle"
                    }
                },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "search_sequences",
            "title": "Find designs by sequence",
            "description": "Align a nucleotide sequence against visible registry sequences and return percent identity, strand, CIGAR, and score evidence.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "sequence": { "type": "string", "minLength": 1 },
                    "mode": { "type": "string", "enum": ["global", "substring", "exact"], "default": "global" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "required": ["sequence"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "find_similar_designs",
            "title": "Find related designs",
            "description": "Find visible cluster-related designs for a canonical design or sequence IRI, ranked by registry PageRank.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "validate_design_upload",
            "title": "Check a design before upload",
            "description": "Run SBOL parsing, validation, identity minting, and collision analysis without changing registry data.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Collection display identifier." },
                    "version": { "type": "string", "default": "1" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "creator_name": { "type": "string" },
                    "citations": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "format": { "type": "string", "enum": ["rdfxml", "turtle", "jsonld", "ntriples", "genbank", "fasta"], "default": "turtle" },
                    "collision": { "type": "string", "enum": ["fail", "replace", "merge"], "default": "fail" },
                    "content": { "type": "string", "description": "Serialized design content." }
                },
                "required": ["id", "content"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ] }
        }),
        json!({
            "name": "upload_design_collection",
            "title": "Upload a reviewed design collection",
            "description": "Re-run the exact upload preview and commit only when its collection identity and collision consequence match the user's confirmed preview.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "id": { "type": "string", "minLength": 1 },
                    "version": { "type": "string", "default": "1" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "creator_name": { "type": "string" },
                    "citations": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "format": { "type": "string", "enum": ["rdfxml", "turtle", "jsonld", "ntriples", "genbank", "fasta"], "default": "turtle" },
                    "collision": { "type": "string", "enum": ["fail", "replace", "merge"], "default": "fail" },
                    "content": { "type": "string", "minLength": 1 },
                    "expected_collection_uri": { "type": "string", "format": "uri", "description": "Exact collection URI returned by validate_design_upload." },
                    "expected_consequence": { "type": "string", "enum": ["create", "reject", "replace", "merge into"], "description": "Exact consequence returned by validate_design_upload." },
                    "confirm": { "const": true, "description": "Set only after the user reviews the matching preview." }
                },
                "required": ["id", "content", "expected_collection_uri", "expected_consequence", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "update_design_metadata",
            "title": "Improve a design record",
            "description": "Update owned design metadata, notes, provenance source, or citations. Optional expected values prevent overwriting a record changed since inspection.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "mutable_description": { "type": "string" },
                    "mutable_notes": { "type": "string" },
                    "mutable_source": { "type": "string" },
                    "citations": { "type": "array", "items": { "type": "string" } },
                    "expected_name": { "type": "string" },
                    "expected_description": { "type": "string" },
                    "confirm": { "const": true }
                },
                "required": ["iri", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "publish_design",
            "title": "Publish a stable public identity",
            "description": "Publish an owned private design under an explicit public id, version, and fail/replace/merge collision policy.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "id": { "type": "string", "minLength": 1 },
                    "version": { "type": "string", "default": "1" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "citations": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "collision": { "type": "string", "enum": ["fail", "replace", "merge"], "default": "fail" },
                    "confirm": { "const": true }
                },
                "required": ["iri", "id", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "list_design_collaborators",
            "title": "Inspect design access",
            "description": "List owners and read-only collaborators for a design the signed-in account can manage.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "iri": { "type": "string", "format": "uri" } },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_SHARE] }
        }),
        json!({
            "name": "share_design",
            "title": "Grant or revoke design access",
            "description": "Grant or revoke one active SBOL account's read-only access without changing ownership.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "user": { "type": "string", "description": "Recipient username or email." },
                    "action": { "type": "string", "enum": ["grant", "revoke"] },
                    "confirm": { "const": true }
                },
                "required": ["iri", "user", "action", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_SHARE] }
        }),
        json!({
            "name": "list_reviews",
            "title": "Open the review queue",
            "description": "List the latest review cases requested by or assigned to the signed-in account.",
            "inputSchema": { "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object", "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
        json!({
            "name": "start_design_review",
            "title": "Start a design review",
            "description": "Open a review cycle for an owned design and assign an active curator.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "curator": { "type": "string", "description": "Curator username or email." },
                    "note": { "type": "string" },
                    "confirm": { "const": true }
                },
                "required": ["iri", "curator", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
        json!({
            "name": "record_review_decision",
            "title": "Record a curator decision",
            "description": "Approve a pending review or request changes, preserving the note in review history.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "decision": { "type": "string", "enum": ["approve", "request_changes"] },
                    "note": { "type": "string" },
                    "confirm": { "const": true }
                },
                "required": ["iri", "decision", "confirm"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
        json!({
            "name": "get_design_activity",
            "title": "Trace design activity",
            "description": "Read ownership, sharing, edit, publication, and review events for a design the account can manage.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "iri": { "type": "string", "format": "uri" } },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
    ]
}

fn tool_arguments<T: DeserializeOwned>(arguments: Value) -> Result<T, DispatchError> {
    serde_json::from_value(arguments)
        .map_err(|error| DispatchError::Tool(format!("invalid tool arguments: {error}")))
}

fn required_scopes(tool: &str) -> &'static [&'static str] {
    match tool {
        "upload_design_collection" | "update_design_metadata" | "publish_design" => {
            &[SCOPE_READ, SCOPE_WRITE]
        }
        "list_design_collaborators" | "share_design" => &[SCOPE_READ, SCOPE_SHARE],
        "list_reviews"
        | "start_design_review"
        | "record_review_decision"
        | "get_design_activity" => &[SCOPE_READ, SCOPE_REVIEW],
        _ => &[SCOPE_READ],
    }
}

fn submit_request(
    user: &User,
    arguments: ValidateDesignArgs,
) -> Result<SubmitRequest, DispatchError> {
    if arguments.id.trim().is_empty() {
        return Err(DispatchError::Tool(
            "collection id must not be empty".to_owned(),
        ));
    }
    if arguments.version.trim().is_empty() {
        return Err(DispatchError::Tool(
            "collection version must not be empty".to_owned(),
        ));
    }
    if arguments.content.trim().is_empty() {
        return Err(DispatchError::Tool(
            "serialized design content must not be empty".to_owned(),
        ));
    }
    Ok(SubmitRequest {
        owner: user.username.clone(),
        id: arguments.id,
        version: arguments.version,
        name: arguments.name.and_then(non_blank),
        description: arguments.description.and_then(non_blank),
        creator_name: arguments.creator_name.and_then(non_blank),
        citations: arguments.citations,
        body: arguments.content,
        format: parse_submission_format(&arguments.format)?,
        overwrite: parse_collision_policy(arguments.collision.as_deref())?,
    })
}

fn parse_download_format(value: &str) -> Result<(DownloadFormat, bool, bool), DispatchError> {
    match value {
        "sbol3-rdfxml" => Ok((DownloadFormat::Sbol, false, false)),
        "sbol3-turtle" => Ok((DownloadFormat::Turtle, false, false)),
        "sbol3-jsonld" => Ok((DownloadFormat::JsonLd, false, false)),
        "sbol3-ntriples" => Ok((DownloadFormat::NTriples, false, false)),
        "sbol2-rdfxml" => Ok((DownloadFormat::Sbol, true, false)),
        "sbol2-turtle" => Ok((DownloadFormat::Turtle, true, false)),
        "genbank" => Ok((DownloadFormat::GenBank, false, false)),
        "fasta" => Ok((DownloadFormat::Fasta, false, false)),
        "gff3" => Ok((DownloadFormat::Gff3, false, false)),
        "omex" => Ok((DownloadFormat::Omex, false, true)),
        other => Err(DispatchError::Tool(format!(
            "unsupported download format `{other}`"
        ))),
    }
}

fn bounded_limit(
    requested: Option<usize>,
    default: usize,
    maximum: usize,
) -> Result<usize, DispatchError> {
    let value = requested.unwrap_or(default);
    if value == 0 || value > maximum {
        return Err(DispatchError::Tool(format!(
            "limit must be between 1 and {maximum}"
        )));
    }
    Ok(value)
}

fn require_member(user: &User, operation: &str) -> Result<(), DispatchError> {
    if user.is_member || user.is_admin {
        Ok(())
    } else {
        Err(DispatchError::Tool(format!(
            "an active member account is required to {operation}"
        )))
    }
}

async fn authorize_design_management(
    state: &AppState,
    user: &User,
    iri: &str,
) -> Result<(), DispatchError> {
    let graph = state
        .app
        .acl_service
        .graph_of_subject(iri)
        .await
        .map_err(tool_error)?
        .ok_or_else(|| DispatchError::Tool("design was not found".to_owned()))?;
    if !state
        .app
        .acl_service
        .can_write(&user.graph_uri, user.is_admin, iri, &graph)
        .await
        .map_err(tool_error)?
    {
        return Err(DispatchError::Tool(
            "this account is not authorized to manage the design".to_owned(),
        ));
    }
    Ok(())
}

async fn resolve_member(state: &AppState, identifier: &str) -> Result<User, DispatchError> {
    let user = state
        .app
        .users
        .find_by_email_or_username(identifier)
        .await
        .map_err(tool_error)?
        .ok_or_else(|| DispatchError::Tool(format!("SBOL account `{identifier}` was not found")))?;
    if !user.is_member && !user.is_admin {
        return Err(DispatchError::Tool(format!(
            "SBOL account `{}` is not an active member",
            user.username
        )));
    }
    Ok(user)
}

async fn resolve_collaborators(
    state: &AppState,
    mut graph_uris: Vec<String>,
) -> Result<Vec<Value>, DispatchError> {
    graph_uris.sort();
    graph_uris.dedup();
    let mut collaborators = Vec::with_capacity(graph_uris.len());
    for graph_uri in graph_uris {
        let identifier = graph_uri.rsplit('/').next().unwrap_or(&graph_uri);
        match state
            .app
            .users
            .find_by_email_or_username(identifier)
            .await
            .map_err(tool_error)?
        {
            Some(user) => collaborators.push(collaborator_json(&user)),
            None => collaborators.push(json!({
                "username": identifier,
                "name": identifier,
                "graph_uri": graph_uri,
                "is_curator": false
            })),
        }
    }
    collaborators.sort_by(|left, right| left["username"].as_str().cmp(&right["username"].as_str()));
    Ok(collaborators)
}

fn collaborator_json(user: &User) -> Value {
    json!({
        "username": user.username,
        "name": user.name,
        "graph_uri": user.graph_uri,
        "is_curator": user.is_curator
    })
}

fn tool_error(error: impl std::fmt::Display) -> DispatchError {
    DispatchError::Tool(error.to_string())
}

fn parse_submission_format(value: &str) -> Result<SerializationFormat, DispatchError> {
    match value {
        "rdfxml" => Ok(SerializationFormat::RdfXml),
        "turtle" => Ok(SerializationFormat::Turtle),
        "jsonld" => Ok(SerializationFormat::JsonLd),
        "ntriples" => Ok(SerializationFormat::NTriples),
        "genbank" => Ok(SerializationFormat::GenBank),
        "fasta" => Ok(SerializationFormat::Fasta),
        _ => Err(DispatchError::Tool(format!(
            "unsupported design format `{value}`"
        ))),
    }
}

fn parse_collision_policy(value: Option<&str>) -> Result<ImportOverwrite, DispatchError> {
    match value.unwrap_or("fail") {
        "fail" => Ok(ImportOverwrite::Fail),
        "replace" => Ok(ImportOverwrite::Replace),
        "merge" => Ok(ImportOverwrite::Merge),
        other => Err(DispatchError::Tool(format!(
            "unsupported collision policy `{other}`"
        ))),
    }
}

fn consequence_label(value: sbol_db_app::SubmitConsequence) -> &'static str {
    match value {
        sbol_db_app::SubmitConsequence::Create => "create",
        sbol_db_app::SubmitConsequence::RejectConflict => "reject",
        sbol_db_app::SubmitConsequence::Replace => "replace",
        sbol_db_app::SubmitConsequence::Merge => "merge into",
    }
}

fn structured_tool_result(
    text: String,
    structured: Result<Value, serde_json::Error>,
) -> Result<Value, DispatchError> {
    let structured = structured.map_err(|error| DispatchError::Tool(error.to_string()))?;
    let structured = structured.as_object().cloned().ok_or_else(|| {
        DispatchError::Tool("tool result was not a structured JSON object".to_owned())
    })?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    }))
}

fn tool_error_result(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(non_blank)
}

fn non_blank(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

struct McpPrincipal {
    user: User,
    grant: OAuthAccessToken,
}

impl McpPrincipal {
    fn has_scopes(&self, required: &[&str]) -> bool {
        let granted = self
            .grant
            .scopes
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        required.iter().all(|scope| granted.contains(scope))
    }
}

async fn authenticated_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<McpPrincipal, Response> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .ok_or_else(|| unauthorized(state))?;
    let resource = mcp_resource(state).ok_or_else(|| {
        http_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SBOL Identity has no configured public origin",
        )
    })?;
    let grant = state
        .app
        .oauth
        .resolve_access_token(token, &resource)
        .await
        .map_err(|_| {
            http_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
            )
        })?
        .ok_or_else(|| unauthorized(state))?;
    let user = state
        .app
        .users
        .get_by_id(grant.user_id)
        .await
        .map_err(|_| {
            http_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
            )
        })?
        .ok_or_else(|| unauthorized(state))?;
    Ok(McpPrincipal { user, grant })
}

fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn origin_is_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Some(configured) = state.config.public_origin.as_deref() else {
        return false;
    };
    let Some(origin) = origin
        .to_str()
        .ok()
        .and_then(|value| Url::parse(value).ok())
    else {
        return false;
    };
    let Ok(configured) = Url::parse(configured) else {
        return false;
    };
    origin.origin() == configured.origin()
}

fn accepts_streamable_http(headers: &HeaderMap) -> bool {
    accepts_media_type(headers, "application/json")
        && accepts_media_type(headers, "text/event-stream")
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn accepts_media_type(headers: &HeaderMap, required: &str) -> bool {
    headers.get_all(ACCEPT).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|media_range| {
                let media_type = media_range.split(';').next().unwrap_or_default().trim();
                media_type == "*/*" || media_type.eq_ignore_ascii_case(required)
            })
        })
    })
}

fn unauthorized(state: &AppState) -> Response {
    let mut response = http_error(
        StatusCode::UNAUTHORIZED,
        "an SBOL Identity access token for this MCP server is required",
    );
    let challenge = protected_resource_metadata_url(state)
        .map(|metadata| format!("Bearer resource_metadata=\"{metadata}\", scope=\"{SCOPE_READ}\""))
        .unwrap_or_else(|| format!("Bearer scope=\"{SCOPE_READ}\""));
    if let Ok(challenge) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
    }
    response
}

fn insufficient_scope(state: &AppState, required: &[&str]) -> Response {
    let mut response = http_error(
        StatusCode::FORBIDDEN,
        "the SBOL Identity token does not grant the scopes required by this operation",
    );
    let scopes = required.join(" ");
    let challenge = protected_resource_metadata_url(state)
        .map(|metadata| {
            format!(
                "Bearer error=\"insufficient_scope\", scope=\"{scopes}\", resource_metadata=\"{metadata}\""
            )
        })
        .unwrap_or_else(|| {
            format!("Bearer error=\"insufficient_scope\", scope=\"{scopes}\"")
        });
    if let Ok(challenge) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
    }
    response
}

fn http_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [
            (CONTENT_TYPE, "application/json"),
            (CACHE_CONTROL, "no-store"),
        ],
        Json(json!({ "error": message })),
    )
        .into_response()
}

fn json_rpc_result(id: Value, result: Value) -> Response {
    json_rpc_response(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
}

fn json_rpc_error(id: Value, code: i32, message: &str) -> Response {
    json_rpc_response(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
}

fn json_rpc_response(body: Value) -> Response {
    (
        [
            (CONTENT_TYPE, "application/json"),
            (CACHE_CONTROL, "no-store"),
        ],
        Json(body),
    )
        .into_response()
}

enum DispatchError {
    Protocol(String),
    Tool(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_order_and_annotations_are_stable() {
        let tools = tools();
        assert_eq!(tools[0]["name"], "search_designs");
        assert_eq!(tools[1]["name"], "get_design");
        assert_eq!(tools[2]["name"], "download_design");
        assert_eq!(tools[5]["name"], "validate_design_upload");
        assert_eq!(tools.len(), 15);
        assert!(tools
            .iter()
            .all(|tool| tool["_meta"]["io.sbol/requiredScopes"].is_array()));
        assert!(tools
            .iter()
            .any(|tool| tool["annotations"]["readOnlyHint"] == false));
    }

    #[test]
    fn bearer_parser_is_strict_and_case_insensitive() {
        assert_eq!(bearer_token("Bearer token"), Some("token"));
        assert_eq!(bearer_token("bearer  token "), Some("token"));
        assert_eq!(bearer_token("Basic token"), None);
        assert_eq!(bearer_token("Bearer "), None);
    }
}
