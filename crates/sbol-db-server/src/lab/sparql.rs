//! `POST /lab/api/sparql/execute` — a JSON-envelope wrapper around
//! the canonical SPARQL endpoint.
//!
//! The Vite dev server only proxies traffic under `/lab/api/*`; without
//! this shim, the SPA in dev mode can't reach the SPARQL engine
//! without a manual `/sparql` proxy rule. Keeping all lab traffic
//! under one prefix also gives us a clean place to put the
//! corresponding `/lab/api/sparql/validate` endpoint (PR 4).
//!
//! Wire shape:
//!
//! ```json
//! { "query": "SELECT ?s WHERE { ?s ?p ?o } LIMIT 5" }
//! ```
//!
//! Response:
//!
//! ```json
//! {
//!   "content_type": "application/sparql-results+json",
//!   "body": { /* parsed JSON if json, else string */ },
//!   "elapsed_ms": 4,
//!   "truncated": false
//! }
//! ```
//!
//! Everything else (parsing, evaluation, format negotiation) flows
//! through the existing `SparqlEngine`, so behavior stays identical to
//! the canonical endpoint.

use std::time::Instant;

use axum::extract::State;
use axum::Json;
use sbol_db_sparql::{ResultFormat, SparqlOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;
use crate::AppState;

use super::validate::{ValidateError, ValidateResp};

#[derive(Deserialize)]
pub struct ExecuteReq {
    pub query: String,
    /// Optional override for the engine's response format. Accepts the
    /// same strings as `ResultFormat::from_str` (`"json"`, `"turtle"`,
    /// `"csv"`, …). Default: form-appropriate (`json` for SELECT/ASK,
    /// `turtle` for CONSTRUCT/DESCRIBE).
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Serialize)]
pub struct ExecuteResp {
    pub content_type: String,
    /// Parsed JSON when `content_type` is `application/sparql-results+json`,
    /// otherwise the body as a UTF-8 string. Keeps the SPA's
    /// rendering logic uniform.
    pub body: Value,
    pub elapsed_ms: u64,
    pub truncated: bool,
}

pub async fn execute(
    State(state): State<AppState>,
    Json(req): Json<ExecuteReq>,
) -> Result<Json<ExecuteResp>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError::BadRequest("empty query".into()));
    }

    let format = match req.format.as_deref() {
        None => None,
        Some(s) => Some(
            s.parse::<ResultFormat>()
                .map_err(|e| ApiError::BadRequest(e.to_string()))?,
        ),
    };
    let options = SparqlOptions::default();

    let started = Instant::now();
    let outcome = state.sparql.execute(&req.query, format, &options).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let content_type = outcome.payload.content_type.to_string();
    let body = if content_type.contains("application/sparql-results+json")
        || content_type.contains("application/json")
    {
        serde_json::from_slice::<Value>(&outcome.payload.body).unwrap_or_else(|_| {
            // The engine should always emit valid JSON for the
            // sparql-results+json content type, but degrade gracefully
            // rather than 500 if it doesn't.
            Value::String(String::from_utf8_lossy(&outcome.payload.body).into_owned())
        })
    } else {
        Value::String(String::from_utf8_lossy(&outcome.payload.body).into_owned())
    };

    Ok(Json(ExecuteResp {
        content_type,
        body,
        elapsed_ms,
        truncated: outcome.payload.truncated,
    }))
}

#[derive(Deserialize)]
pub struct ValidateReq {
    pub query: String,
}

/// `POST /lab/api/sparql/validate` — parse via spargebra without
/// executing. Returns the canonical [`ValidateResp`] envelope. The
/// parser is the same one `/sparql` and `/lab/api/sparql/execute`
/// invoke, so validate-time errors match execute-time errors exactly.
pub async fn validate(Json(req): Json<ValidateReq>) -> Json<ValidateResp> {
    if req.query.trim().is_empty() {
        return Json(ValidateResp::ok());
    }
    match spargebra::SparqlParser::new().parse_query(&req.query) {
        Ok(_) => Json(ValidateResp::ok()),
        Err(err) => Json(ValidateResp::err(spargebra_to_validate(&err))),
    }
}

/// spargebra's `Display` format starts with `"error at <line>:<col>: ..."`
/// (with a leading "parse error" prefix in some versions). Pull the
/// `line:col` out so Monaco can render the squiggle in the right place;
/// fall back to (1, 1) if the format ever changes.
fn spargebra_to_validate(err: &spargebra::SparqlSyntaxError) -> ValidateError {
    let message = err.to_string();
    let (line, column) = parse_at_position(&message).unwrap_or((1, 1));
    ValidateError {
        message,
        line,
        column,
        end_line: None,
        end_column: None,
    }
}

fn parse_at_position(msg: &str) -> Option<(u32, u32)> {
    // Look for the first `<digits>:<digits>` pair in the message.
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b':' {
                let line_str = &msg[start..i];
                i += 1;
                let col_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > col_start {
                    let col_str = &msg[col_start..i];
                    if let (Ok(l), Ok(c)) = (line_str.parse::<u32>(), col_str.parse::<u32>()) {
                        return Some((l, c));
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}
