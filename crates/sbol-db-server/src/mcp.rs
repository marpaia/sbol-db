//! SBOL Identity-authorized, stateless MCP Streamable HTTP adapter.
//!
//! Every credential is an OAuth access token issued specifically for this MCP
//! resource. Tool calls combine those granted scopes with the same application
//! services and ACL model used by the V2 and compatibility APIs.

mod resources;

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
    AlignMode, AlignOptions, DiscoveryQuery, FieldValue, MakePublicRequest,
    PreparedMutationBinding, ReviewDecision, SubmitRequest,
};
use sbol_db_core::{IriString, PreparedMutation, SerializationFormat, User};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_storage::{ConditionalContentWrite, ImportOverwrite};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::identity::{
    mcp_resource, protected_resource_metadata_url, SCOPE_READ, SCOPE_REVIEW, SCOPE_SHARE,
    SCOPE_WRITE,
};
use crate::v2::auth::RequestPrincipal;
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
        "resources/list" => Ok(resources::list(&principal)),
        "resources/templates/list" => Ok(resources::templates()),
        "resources/read" => {
            let required = resources::required_scopes(&params);
            if !principal.has_scopes(required) {
                return insufficient_scope(&state, required);
            }
            resources::read(&state, &principal, &params).await
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return json_rpc_error(id, -32602, "tools/call requires a tool name");
            };
            let required = required_scopes(name);
            if !principal.has_scopes(required) {
                return insufficient_scope(&state, required);
            }
            call_tool(&state, &principal, params).await
        }
        _ => return json_rpc_error(id, -32601, "method not found"),
    };
    match result {
        Ok(result) => json_rpc_result(id, result),
        Err(DispatchError::Protocol(message)) => json_rpc_error(id, -32602, &message),
        Err(DispatchError::Tool(message)) if method == "tools/call" => {
            json_rpc_result(id, tool_error_result(message))
        }
        Err(DispatchError::Tool(message)) => json_rpc_error(id, -32002, &message),
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
            "tools": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false }
        },
        "serverInfo": {
            "name": "sbol-db",
            "title": "SBOL DB",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Permission-aware biological design discovery, contribution, sharing, and review"
        },
        "instructions": "Use search_designs before get_design when you do not already know a canonical IRI. Every result and mutation is scoped to the signed-in SBOL Identity account. Mutation tools first prepare a short-lived change without altering registry data. Show the returned effect to the user, then call apply_prepared_change with only its one-time plan token."
    }))
}

async fn call_tool(
    state: &AppState,
    principal: &RequestPrincipal,
    params: Value,
) -> Result<Value, DispatchError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Protocol("tools/call requires a tool name".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    tracing::info!(
        target: "sbol_db::machine_access",
        user_id = %user.id,
        oauth_client_id = principal.oauth_client_id.as_deref().unwrap_or(""),
        audience = principal.audience.as_deref().unwrap_or(""),
        authentication_method = ?principal.authentication_method,
        tool = name,
        "MCP tool invocation"
    );
    let result = match name {
        "search_designs" => search_designs(state, user, arguments).await,
        "get_design" => get_design(state, user, arguments).await,
        "download_design" => download_design(state, user, arguments).await,
        "get_collection_sync_state" => get_collection_sync_state(state, user, arguments).await,
        "search_sequences" => search_sequences(state, user, arguments).await,
        "find_similar_designs" => find_similar_designs(state, user, arguments).await,
        "validate_design_upload" => validate_design_upload(state, principal, arguments).await,
        "upload_design_collection" => upload_design_collection(state, principal, arguments).await,
        "prepare_design_metadata_update" => {
            prepare_design_metadata_update(state, principal, arguments).await
        }
        "prepare_design_publication" => {
            prepare_design_publication(state, principal, arguments).await
        }
        "prepare_collection_update" => prepare_collection_update(state, principal, arguments).await,
        "list_design_collaborators" => list_design_collaborators(state, user, arguments).await,
        "prepare_design_sharing" => prepare_design_sharing(state, principal, arguments).await,
        "list_reviews" => list_reviews(state, user, arguments).await,
        "prepare_design_review" => prepare_design_review(state, principal, arguments).await,
        "prepare_review_decision" => prepare_review_decision(state, principal, arguments).await,
        "apply_prepared_change" => apply_prepared_change(state, principal, arguments).await,
        "get_design_activity" => get_design_activity(state, user, arguments).await,
        _ => Err(DispatchError::Protocol(format!("unknown tool `{name}`"))),
    };
    tracing::info!(
        target: "sbol_db::machine_access",
        user_id = %user.id,
        oauth_client_id = principal.oauth_client_id.as_deref().unwrap_or(""),
        tool = name,
        outcome = if result.is_ok() { "succeeded" } else { "failed" },
        "MCP tool outcome"
    );
    result
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

async fn get_collection_sync_state(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: CollectionSyncStateArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri)
        .map_err(|error| DispatchError::Tool(format!("invalid collection IRI: {error}")))?;
    let format = parse_collection_format(&arguments.format)?;
    let content = state
        .app
        .collection_sync_service()
        .read(Some(&user.graph_uri), iri.as_str())
        .await
        .map_err(tool_error)?
        .ok_or_else(|| {
            DispatchError::Tool(
                "collection was not found or is not visible to this account".to_owned(),
            )
        })?;
    let serialized = triples_to_rdf(&content.triples, format).map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Read synchronized biological content for {} at content ETag {}.",
            iri, content.content_etag
        ),
        Ok(json!({
            "collection_uri": iri.as_str(),
            "content_etag": content.content_etag,
            "triple_count": content.triples.len(),
            "format": collection_format_name(format),
            "content_type": collection_media_type(format),
            "content": serialized
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
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let arguments: ValidateDesignArgs = tool_arguments(arguments)?;
    let payload = serde_json::to_value(&arguments).map_err(tool_error)?;
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    let request = submit_request(user, arguments)?;
    if request.overwrite != ImportOverwrite::Fail {
        return Err(DispatchError::Tool(
            "initial agent uploads are create-only; update an existing synchronized collection with its content ETag"
                .to_owned(),
        ));
    }
    let preview = state
        .app
        .submission_service()
        .preview(&request)
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    if preview.collision {
        return Err(DispatchError::Tool(format!(
            "a collection already exists at {}; choose a new id or version, or use collection synchronization with its content ETag",
            preview.collection_uri
        )));
    }
    let effect = json!({
        "action": "create_collection",
        "collection_uri": preview.collection_uri,
        "members": preview.members,
        "triple_count": preview.triple_count,
        "collision_policy": "fail"
    });
    let receipt = state
        .app
        .prepared_mutation_service()
        .prepare(
            &prepared_binding(principal)?,
            "collection.upload",
            Some(preview.collection_uri.clone()),
            None,
            vec![SCOPE_READ.to_owned(), SCOPE_WRITE.to_owned()],
            effect,
            payload,
        )
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    let mut structured = serde_json::to_value(&preview).map_err(tool_error)?;
    structured["prepared_change"] = serde_json::to_value(&receipt).map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Upload is valid. It would create {} with {} members and {} triples. No registry data was changed; the prepared change expires at {}.",
            preview.collection_uri,
            preview.members.len(),
            preview.triple_count,
            receipt.expires_at
        ),
        Ok(structured),
    )
}

async fn upload_design_collection(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "upload a design")?;
    let arguments: UploadDesignArgs = tool_arguments(arguments)?;
    let plan = state
        .app
        .prepared_mutation_service()
        .consume(&arguments.plan_token, &prepared_binding(principal)?)
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    execute_upload_plan(state, user, plan).await
}

async fn execute_upload_plan(
    state: &AppState,
    user: &User,
    plan: PreparedMutation,
) -> Result<Value, DispatchError> {
    if plan.operation != "collection.upload" {
        return Err(DispatchError::Tool(
            "the prepared change token is not an upload plan".to_owned(),
        ));
    }
    let design: ValidateDesignArgs = serde_json::from_value(plan.payload)
        .map_err(|error| DispatchError::Tool(format!("invalid stored upload plan: {error}")))?;
    let request = submit_request(user, design)?;
    if request.overwrite != ImportOverwrite::Fail {
        return Err(DispatchError::Tool(
            "the stored upload plan does not use fail-on-collision creation".to_owned(),
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
        "consequence": "create",
        "input_hash": plan.input_hash
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

async fn prepare_design_metadata_update(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "update a design")?;
    let arguments: UpdateDesignArgs = tool_arguments(arguments)?;
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
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let (before, snapshot) = design_snapshot(state, user, iri.as_str()).await?;
    if arguments
        .expected_name
        .as_ref()
        .is_some_and(|expected| before["name"].as_str() != Some(expected))
        || arguments
            .expected_description
            .as_ref()
            .is_some_and(|expected| before["description"].as_str() != Some(expected))
    {
        return Err(DispatchError::Tool(
            "the design metadata changed since it was inspected; open it again before preparing the update"
                .to_owned(),
        ));
    }
    let effect = json!({
        "action": "update_design_metadata",
        "iri": iri.as_str(),
        "expected_design_snapshot": snapshot,
        "changes": metadata_changes(&arguments)
    });
    prepare_agent_change(
        state,
        principal,
        "design.metadata.update",
        Some(iri.as_str().to_owned()),
        &[SCOPE_READ, SCOPE_WRITE],
        effect,
        serde_json::to_value(arguments).map_err(tool_error)?,
        "Prepared a metadata update without changing the design",
    )
    .await
}

async fn prepare_design_publication(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "publish a design")?;
    let arguments: PublishDesignArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    if arguments.id.trim().is_empty() || arguments.version.trim().is_empty() {
        return Err(DispatchError::Tool(
            "public id and version must not be empty".to_owned(),
        ));
    }
    parse_collision_policy(arguments.collision.as_deref())?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let (_, snapshot) = design_snapshot(state, user, iri.as_str()).await?;
    let effect = json!({
        "action": "publish_design",
        "source_iri": iri.as_str(),
        "public_id": arguments.id,
        "version": arguments.version,
        "collision_policy": arguments.collision.as_deref().unwrap_or("fail"),
        "expected_design_snapshot": snapshot
    });
    prepare_agent_change(
        state,
        principal,
        "design.publish",
        Some(iri.as_str().to_owned()),
        &[SCOPE_READ, SCOPE_WRITE],
        effect,
        serde_json::to_value(arguments).map_err(tool_error)?,
        "Prepared publication without creating a public design",
    )
    .await
}

async fn prepare_collection_update(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "update a synchronized collection")?;
    let arguments: CollectionUpdateArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid collection IRI: {error}")))?;
    if arguments.content.trim().is_empty() {
        return Err(DispatchError::Tool(
            "serialized collection content must not be empty".to_owned(),
        ));
    }
    if arguments.expected_content_etag.trim().is_empty() {
        return Err(DispatchError::Tool(
            "expected_content_etag must be the exact strong ETag returned by get_collection_sync_state"
                .to_owned(),
        ));
    }
    let format = parse_collection_format(&arguments.format)?;
    let preview = state
        .app
        .collection_sync_service()
        .preview_update(
            &user.graph_uri,
            user.is_admin,
            iri.as_str(),
            &arguments.content,
            format,
            &arguments.expected_content_etag,
        )
        .await
        .map_err(tool_error)?;
    let effect = json!({
        "action": "replace_collection_biological_content",
        "collection_uri": iri.as_str(),
        "expected_content_etag": preview.current_content_etag,
        "proposed_content_etag": preview.proposed_content_etag,
        "triple_count": preview.triple_count,
        "server_managed_metadata_preserved": true
    });
    let receipt = state
        .app
        .prepared_mutation_service()
        .prepare(
            &prepared_binding(principal)?,
            "collection.update",
            Some(iri.as_str().to_owned()),
            Some(preview.current_content_etag),
            vec![SCOPE_READ.to_owned(), SCOPE_WRITE.to_owned()],
            effect,
            serde_json::to_value(arguments).map_err(tool_error)?,
        )
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!(
            "Prepared a whole-collection update without changing registry data. Review the proposed biological content ETag, then apply the one-time token before {}.",
            receipt.expires_at
        ),
        Ok(json!({ "prepared_change": receipt })),
    )
}

async fn prepare_design_sharing(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "manage design sharing")?;
    let arguments: ShareDesignArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let target = resolve_member(state, &arguments.user).await?;
    if target.id == user.id {
        return Err(DispatchError::Tool(
            "the owner cannot be added or removed as a read-only collaborator".to_owned(),
        ));
    }
    if !matches!(arguments.action.as_str(), "grant" | "revoke") {
        return Err(DispatchError::Tool(
            "sharing action must be grant or revoke".to_owned(),
        ));
    }
    let effect = json!({
        "action": format!("{}_design_access", arguments.action),
        "iri": iri.as_str(),
        "collaborator": collaborator_json(&target)
    });
    prepare_agent_change(
        state,
        principal,
        "design.share",
        Some(iri.as_str().to_owned()),
        &[SCOPE_READ, SCOPE_SHARE],
        effect,
        serde_json::to_value(arguments).map_err(tool_error)?,
        "Prepared a sharing change without changing access",
    )
    .await
}

async fn prepare_design_review(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    require_member(user, "start a design review")?;
    let arguments: StartReviewArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    authorize_design_management(state, user, iri.as_str()).await?;
    let curator = resolve_member(state, &arguments.curator).await?;
    if !curator.is_curator && !curator.is_admin {
        return Err(DispatchError::Tool(format!(
            "SBOL account `{}` is not an active curator",
            curator.username
        )));
    }
    let effect = json!({
        "action": "start_design_review",
        "iri": iri.as_str(),
        "curator": collaborator_json(&curator),
        "note": arguments.note
    });
    prepare_agent_change(
        state,
        principal,
        "design.review.start",
        Some(iri.as_str().to_owned()),
        &[SCOPE_READ, SCOPE_REVIEW],
        effect,
        serde_json::to_value(arguments).map_err(tool_error)?,
        "Prepared a review request without opening a review",
    )
    .await
}

async fn prepare_review_decision(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    if !user.is_curator && !user.is_admin {
        return Err(DispatchError::Tool(
            "an active curator account is required to record a review decision".to_owned(),
        ));
    }
    let arguments: ReviewDecisionArgs = tool_arguments(arguments)?;
    let iri = IriString::new(arguments.iri.clone())
        .map_err(|error| DispatchError::Tool(format!("invalid design IRI: {error}")))?;
    if !matches!(arguments.decision.as_str(), "approve" | "request_changes") {
        return Err(DispatchError::Tool(
            "decision must be approve or request_changes".to_owned(),
        ));
    }
    let effect = json!({
        "action": "record_review_decision",
        "iri": iri.as_str(),
        "decision": arguments.decision,
        "note": arguments.note
    });
    prepare_agent_change(
        state,
        principal,
        "design.review.decide",
        Some(iri.as_str().to_owned()),
        &[SCOPE_READ, SCOPE_REVIEW],
        effect,
        serde_json::to_value(arguments).map_err(tool_error)?,
        "Prepared a curator decision without changing review state",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_agent_change(
    state: &AppState,
    principal: &RequestPrincipal,
    operation: &str,
    target_iri: Option<String>,
    required_scopes: &[&str],
    effect: Value,
    payload: Value,
    summary: &str,
) -> Result<Value, DispatchError> {
    let receipt = state
        .app
        .prepared_mutation_service()
        .prepare(
            &prepared_binding(principal)?,
            operation,
            target_iri,
            None,
            required_scopes
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
            effect,
            payload,
        )
        .await
        .map_err(tool_error)?;
    structured_tool_result(
        format!(
            "{summary}. Review the prepared effect, then apply its one-time token before {}.",
            receipt.expires_at
        ),
        Ok(json!({ "prepared_change": receipt })),
    )
}

async fn apply_prepared_change(
    state: &AppState,
    principal: &RequestPrincipal,
    arguments: Value,
) -> Result<Value, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    let arguments: ApplyPreparedArgs = tool_arguments(arguments)?;
    let plan = state
        .app
        .prepared_mutation_service()
        .consume(&arguments.plan_token, &prepared_binding(principal)?)
        .await
        .map_err(tool_error)?;
    match plan.operation.as_str() {
        "collection.upload" => execute_upload_plan(state, user, plan).await,
        "collection.update" => execute_collection_update_plan(state, user, plan).await,
        "design.metadata.update" => {
            verify_prepared_design_snapshot(state, user, &plan).await?;
            update_design_metadata(state, user, confirmed_payload(plan.payload)?).await
        }
        "design.publish" => {
            verify_prepared_design_snapshot(state, user, &plan).await?;
            publish_design(state, user, confirmed_payload(plan.payload)?).await
        }
        "design.share" => share_design(state, user, confirmed_payload(plan.payload)?).await,
        "design.review.start" => {
            start_design_review(state, user, confirmed_payload(plan.payload)?).await
        }
        "design.review.decide" => {
            record_review_decision(state, user, confirmed_payload(plan.payload)?).await
        }
        operation => Err(DispatchError::Tool(format!(
            "unsupported prepared change operation `{operation}`"
        ))),
    }
}

async fn execute_collection_update_plan(
    state: &AppState,
    user: &User,
    plan: PreparedMutation,
) -> Result<Value, DispatchError> {
    let expected = plan.expected_content_etag.clone().ok_or_else(|| {
        DispatchError::Tool("prepared collection update has no content ETag".to_owned())
    })?;
    let arguments: CollectionUpdateArgs = serde_json::from_value(plan.payload)
        .map_err(|error| DispatchError::Tool(format!("invalid stored update plan: {error}")))?;
    let format = parse_collection_format(&arguments.format)?;
    let outcome = state
        .app
        .collection_sync_service()
        .write(
            &user.graph_uri,
            user.is_admin,
            &arguments.iri,
            &arguments.content,
            format,
            Some(&expected),
        )
        .await
        .map_err(tool_error)?;
    match outcome {
        ConditionalContentWrite::Applied {
            triple_count,
            content_etag,
        } => structured_tool_result(
            format!(
                "Updated synchronized biological content for {} at content ETag {}.",
                arguments.iri, content_etag
            ),
            Ok(json!({
                "collection_uri": arguments.iri,
                "content_etag": content_etag,
                "triple_count": triple_count,
                "input_hash": plan.input_hash
            })),
        ),
        ConditionalContentWrite::PreconditionFailed {
            current_content_etag,
        } => Err(DispatchError::Tool(format!(
            "the collection changed after this update was prepared; no data was overwritten. Current content ETag: {current_content_etag:?}"
        ))),
    }
}

async fn design_snapshot(
    state: &AppState,
    user: &User,
    iri: &str,
) -> Result<(Value, String), DispatchError> {
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await
        .map_err(tool_error)?;
    let details = state
        .app
        .object_details(iri, scope)
        .await
        .map_err(tool_error)?
        .ok_or_else(|| {
            DispatchError::Tool("design was not found or is not visible to this account".to_owned())
        })?;
    let value = serde_json::to_value(details).map_err(tool_error)?;
    let bytes = serde_json::to_vec(&value).map_err(tool_error)?;
    let snapshot = hex::encode(Sha256::digest(bytes));
    Ok((value, snapshot))
}

async fn verify_prepared_design_snapshot(
    state: &AppState,
    user: &User,
    plan: &PreparedMutation,
) -> Result<(), DispatchError> {
    let expected = plan
        .effect
        .get("expected_design_snapshot")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Tool("prepared change has no design snapshot".to_owned()))?;
    let iri = plan
        .target_iri
        .as_deref()
        .ok_or_else(|| DispatchError::Tool("prepared change has no target design".to_owned()))?;
    let (_, current) = design_snapshot(state, user, iri).await?;
    if current != expected {
        return Err(DispatchError::Tool(
            "the design changed after this operation was prepared; no change was applied"
                .to_owned(),
        ));
    }
    Ok(())
}

fn confirmed_payload(mut payload: Value) -> Result<Value, DispatchError> {
    let object = payload.as_object_mut().ok_or_else(|| {
        DispatchError::Tool("prepared change payload is not an object".to_owned())
    })?;
    object.insert("confirm".to_owned(), Value::Bool(true));
    Ok(payload)
}

fn metadata_changes(arguments: &UpdateDesignArgs) -> Value {
    json!({
        "name": arguments.name,
        "description": arguments.description,
        "mutable_description": arguments.mutable_description,
        "mutable_notes": arguments.mutable_notes,
        "mutable_source": arguments.mutable_source,
        "citations": arguments.citations
    })
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

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CollectionSyncStateArgs {
    iri: String,
    format: String,
}

impl Default for CollectionSyncStateArgs {
    fn default() -> Self {
        Self {
            iri: String::new(),
            format: "turtle".to_owned(),
        }
    }
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
struct CollectionUpdateArgs {
    iri: String,
    format: String,
    content: String,
    expected_content_etag: String,
}

impl Default for CollectionUpdateArgs {
    fn default() -> Self {
        Self {
            iri: String::new(),
            format: "turtle".to_owned(),
            content: String::new(),
            expected_content_etag: String::new(),
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

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
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
    plan_token: String,
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
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

#[derive(Clone, Deserialize, serde::Serialize)]
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

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
struct ShareDesignArgs {
    iri: String,
    user: String,
    action: String,
    confirm: bool,
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
struct StartReviewArgs {
    iri: String,
    curator: String,
    note: Option<String>,
    confirm: bool,
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
struct ReviewDecisionArgs {
    iri: String,
    decision: String,
    note: Option<String>,
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPreparedArgs {
    plan_token: String,
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
            "name": "get_collection_sync_state",
            "title": "Read a synchronized collection",
            "description": "Read one visible collection's biological SBOL content and representation-independent content ETag. Server-managed ownership, sharing, review, audit, and timestamps are excluded.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "format": { "type": "string", "enum": ["turtle", "rdfxml", "jsonld", "ntriples"], "default": "turtle" }
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
            "title": "Prepare a design upload",
            "description": "Validate and mint a create-only upload without changing registry designs, then issue a short-lived one-time prepared-change token bound to this signed-in client.",
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
                    "collision": { "type": "string", "const": "fail", "default": "fail" },
                    "content": { "type": "string", "description": "Serialized design content." }
                },
                "required": ["id", "content"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false
            },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "upload_design_collection",
            "title": "Upload a reviewed design collection",
            "description": "Consume the exact one-time plan returned by validate_design_upload and atomically create its reviewed collection. The token is user, OAuth-client, audience, scope, payload, and expiry bound.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "plan_token": { "type": "string", "pattern": "^sbol_plan_", "description": "One-time token returned in prepared_change by validate_design_upload." }
                },
                "required": ["plan_token"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "prepare_design_metadata_update",
            "title": "Prepare a design metadata update",
            "description": "Inspect and prepare an owned design metadata, notes, provenance, or citation change without applying it. Returns a short-lived one-time prepared-change token.",
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
                    "expected_description": { "type": "string" }
                },
                "required": ["iri"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "prepare_design_publication",
            "title": "Prepare publication of a design",
            "description": "Inspect and prepare publication of an owned private design under an explicit public id, version, and collision policy without creating public data.",
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
                    "collision": { "type": "string", "enum": ["fail", "replace", "merge"], "default": "fail" }
                },
                "required": ["iri", "id"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_WRITE] }
        }),
        json!({
            "name": "prepare_collection_update",
            "title": "Prepare a synchronized collection update",
            "description": "Validate a complete biological SBOL replacement against the exact current content ETag without changing registry data. Ownership, sharing, review, audit, and server metadata remain server-managed.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "format": { "type": "string", "enum": ["turtle", "rdfxml", "jsonld", "ntriples"], "default": "turtle" },
                    "expected_content_etag": { "type": "string", "description": "Exact strong ETag returned by get_collection_sync_state." },
                    "content": { "type": "string", "description": "Complete biological SBOL collection document." }
                },
                "required": ["iri", "expected_content_etag", "content"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
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
            "name": "prepare_design_sharing",
            "title": "Prepare a design access change",
            "description": "Resolve the recipient and prepare a grant or revocation of read-only design access without changing permissions.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "user": { "type": "string", "description": "Recipient username or email." },
                    "action": { "type": "string", "enum": ["grant", "revoke"] }
                },
                "required": ["iri", "user", "action"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
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
            "name": "prepare_design_review",
            "title": "Prepare a design review",
            "description": "Resolve the curator and prepare a review request for an owned design without opening the review.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "curator": { "type": "string", "description": "Curator username or email." },
                    "note": { "type": "string" }
                },
                "required": ["iri", "curator"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
        json!({
            "name": "prepare_review_decision",
            "title": "Prepare a curator decision",
            "description": "Prepare approval or a request for changes on a pending review without recording the decision.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "iri": { "type": "string", "format": "uri" },
                    "decision": { "type": "string", "enum": ["approve", "request_changes"] },
                    "note": { "type": "string" }
                },
                "required": ["iri", "decision"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ, SCOPE_REVIEW] }
        }),
        json!({
            "name": "apply_prepared_change",
            "title": "Apply a reviewed prepared change",
            "description": "Consume one exact one-time plan token after its effect has been shown to the user. The server rechecks user, OAuth client, audience, scopes, expiry, and any captured design state before applying it.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "plan_token": { "type": "string", "pattern": "^sbol_plan_" }
                },
                "required": ["plan_token"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
            "_meta": { "io.sbol/requiredScopes": [SCOPE_READ], "io.sbol/preparedScopesEnforced": true }
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
        "validate_design_upload"
        | "upload_design_collection"
        | "prepare_design_metadata_update"
        | "prepare_design_publication"
        | "prepare_collection_update" => &[SCOPE_READ, SCOPE_WRITE],
        "list_design_collaborators" | "prepare_design_sharing" => &[SCOPE_READ, SCOPE_SHARE],
        "list_reviews"
        | "prepare_design_review"
        | "prepare_review_decision"
        | "get_design_activity" => &[SCOPE_READ, SCOPE_REVIEW],
        "apply_prepared_change" => &[SCOPE_READ],
        _ => &[SCOPE_READ],
    }
}

fn prepared_binding(
    principal: &RequestPrincipal,
) -> Result<PreparedMutationBinding, DispatchError> {
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;
    Ok(PreparedMutationBinding {
        user_id: user.id,
        oauth_client_id: principal.oauth_client_id.clone(),
        audience: principal.audience.clone(),
        scopes: principal.scopes.clone(),
    })
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

fn parse_collection_format(value: &str) -> Result<SerializationFormat, DispatchError> {
    match value {
        "turtle" => Ok(SerializationFormat::Turtle),
        "rdfxml" => Ok(SerializationFormat::RdfXml),
        "jsonld" => Ok(SerializationFormat::JsonLd),
        "ntriples" => Ok(SerializationFormat::NTriples),
        other => Err(DispatchError::Tool(format!(
            "unsupported collection RDF format `{other}`"
        ))),
    }
}

fn collection_format_name(format: SerializationFormat) -> &'static str {
    match format {
        SerializationFormat::Turtle => "turtle",
        SerializationFormat::RdfXml => "rdfxml",
        SerializationFormat::JsonLd => "jsonld",
        SerializationFormat::NTriples => "ntriples",
        _ => unreachable!("collection formats are constrained before serialization"),
    }
}

fn collection_media_type(format: SerializationFormat) -> &'static str {
    match format {
        SerializationFormat::Turtle => "text/turtle",
        SerializationFormat::RdfXml => "application/rdf+xml",
        SerializationFormat::JsonLd => "application/ld+json",
        SerializationFormat::NTriples => "application/n-triples",
        _ => unreachable!("collection formats are constrained before serialization"),
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

async fn authenticated_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<RequestPrincipal, Response> {
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
    Ok(RequestPrincipal::oauth(user, &grant))
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
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn tool_order_and_annotations_are_stable() {
        let tools = tools();
        assert_eq!(tools[0]["name"], "search_designs");
        assert_eq!(tools[1]["name"], "get_design");
        assert_eq!(tools[2]["name"], "download_design");
        assert_eq!(tools[3]["name"], "get_collection_sync_state");
        assert_eq!(tools[6]["name"], "validate_design_upload");
        assert_eq!(tools[16]["name"], "apply_prepared_change");
        assert_eq!(tools.len(), 18);
        assert!(tools
            .iter()
            .all(|tool| tool["_meta"]["io.sbol/requiredScopes"].is_array()));
        assert!(tools
            .iter()
            .any(|tool| tool["annotations"]["readOnlyHint"] == false));

        let mut names = HashSet::new();
        for tool in &tools {
            let name = tool["name"].as_str().expect("tool name");
            assert!(names.insert(name), "duplicate MCP tool name: {name}");
            assert!(!tool["description"].as_str().unwrap_or("").is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{name} must reject undeclared mutation inputs"
            );
            let scopes = tool["_meta"]["io.sbol/requiredScopes"]
                .as_array()
                .expect("required scope array");
            assert!(!scopes.is_empty(), "{name} has no required scope");
            assert!(scopes.iter().all(|scope| scope.as_str().is_some()));
            for annotation in [
                "readOnlyHint",
                "destructiveHint",
                "idempotentHint",
                "openWorldHint",
            ] {
                assert!(
                    tool["annotations"][annotation].is_boolean(),
                    "{name} omits {annotation}"
                );
            }
            if name.starts_with("prepare_") {
                assert_eq!(tool["annotations"]["destructiveHint"], false, "{name}");
            }
        }
        assert_eq!(
            tools[16]["annotations"]["destructiveHint"], true,
            "applying a prepared change is the explicit commit boundary"
        );
        for removed_direct_mutation in [
            "update_design_metadata",
            "publish_design",
            "share_design",
            "start_design_review",
            "record_review_decision",
        ] {
            assert!(!names.contains(removed_direct_mutation));
        }
    }

    #[test]
    fn bearer_parser_is_strict_and_case_insensitive() {
        assert_eq!(bearer_token("Bearer token"), Some("token"));
        assert_eq!(bearer_token("bearer  token "), Some("token"));
        assert_eq!(bearer_token("Basic token"), None);
        assert_eq!(bearer_token("Bearer "), None);
    }
}
