//! Authenticated, stateless MCP Streamable HTTP adapter.
//!
//! The endpoint deliberately starts with read and validation capabilities. It
//! shares the same [`AppServices`](sbol_db_app::AppServices) facade and ACL
//! scope as the V2 API, but unlike public V2 browsing it rejects missing,
//! malformed, stale, and unknown bearer credentials. Mutating tools are not
//! exposed until SBOL Identity scopes and per-call confirmation semantics are
//! available.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sbol_db_app::{DiscoveryQuery, SubmitRequest};
use sbol_db_core::{IriString, SerializationFormat, User};
use sbol_db_storage::ImportOverwrite;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::AppState;

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/mcp", get(get_stream).post(post_message))
}

/// This stateless first slice does not open unsolicited SSE streams. Returning
/// 405 is the transport-defined way to tell a client to use one POST per
/// request.
async fn get_stream(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_is_allowed(&state, &headers) {
        return http_error(
            StatusCode::FORBIDDEN,
            "the request Origin does not match this SBOL DB instance",
        );
    }
    if let Err(response) = authenticated_user(&state, &headers).await {
        return response;
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
    let user = match authenticated_user(&state, &headers).await {
        Ok(user) => user,
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

    let result = match method {
        "initialize" => initialize(&params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(&state, &user, params).await,
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
            "description": "Permission-aware biological design discovery and validation"
        },
        "instructions": "Use search_designs before get_design when you do not already know a canonical IRI. Every result is scoped to the signed-in SBOL DB account. validate_design_upload never changes registry data."
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
        "validate_design_upload" => validate_design_upload(state, user, arguments).await,
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

async fn validate_design_upload(
    state: &AppState,
    user: &User,
    arguments: Value,
) -> Result<Value, DispatchError> {
    if !user.is_member && !user.is_admin {
        return Err(DispatchError::Tool(
            "an active member account is required to validate an upload".to_owned(),
        ));
    }
    let arguments: ValidateDesignArgs = tool_arguments(arguments)?;
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
    let request = SubmitRequest {
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
    };
    let preview = state
        .app
        .submission_service()
        .preview(&request)
        .await
        .map_err(|error| DispatchError::Tool(error.to_string()))?;
    structured_tool_result(
        format!(
            "Upload is valid. It would {} {} with {} members and {} triples. No registry data was changed.",
            consequence_label(preview.consequence),
            preview.collection_uri,
            preview.members.len(),
            preview.triple_count
        ),
        serde_json::to_value(preview),
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
            }
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
            }
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
            }
        }),
    ]
}

fn tool_arguments<T: DeserializeOwned>(arguments: Value) -> Result<T, DispatchError> {
    serde_json::from_value(arguments)
        .map_err(|error| DispatchError::Tool(format!("invalid tool arguments: {error}")))
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

async fn authenticated_user(state: &AppState, headers: &HeaderMap) -> Result<User, Response> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .ok_or_else(unauthorized)?;
    let user_id = state
        .app
        .auth
        .resolve_token(token)
        .await
        .map_err(|_| {
            http_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
            )
        })?
        .ok_or_else(unauthorized)?;
    state
        .app
        .users
        .get_by_id(user_id)
        .await
        .map_err(|_| {
            http_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
            )
        })?
        .ok_or_else(unauthorized)
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

fn unauthorized() -> Response {
    let mut response = http_error(
        StatusCode::UNAUTHORIZED,
        "a valid SBOL DB bearer token is required",
    );
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"SBOL DB MCP\""),
    );
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
        assert_eq!(tools[2]["name"], "validate_design_upload");
        assert!(tools
            .iter()
            .all(|tool| tool["annotations"]["readOnlyHint"] == true));
    }

    #[test]
    fn bearer_parser_is_strict_and_case_insensitive() {
        assert_eq!(bearer_token("Bearer token"), Some("token"));
        assert_eq!(bearer_token("bearer  token "), Some("token"));
        assert_eq!(bearer_token("Basic token"), None);
        assert_eq!(bearer_token("Bearer "), None);
    }
}
