//! HTTP Basic authentication for the write SPARQL endpoints
//! (`/sparql-auth`, `/sparql-graph-crud-auth/`).
//!
//! Virtuoso protects writes with credentials (default `dba`/`dba`). SynBioHub
//! sends them: synbiohub3 uses HTTP Basic directly; classic SynBioHub uses
//! `sendImmediately: false`, meaning it issues the request unauthenticated,
//! and on a `401` with a `WWW-Authenticate` challenge it retries with
//! credentials matching the offered scheme. By challenging with `Basic` we
//! satisfy both: synbiohub3's preemptive Basic header and classic's
//! challenge-response (which answers a Basic challenge with Basic).
//!
//! Reads (`/sparql`) stay unauthenticated, matching Virtuoso's public endpoint.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

use crate::AppState;

const REALM: &str = "sbol-db";

/// Middleware guarding the authenticated SPARQL endpoints. Lets the request
/// through when `sparql_auth_disabled` is set or the `Authorization: Basic`
/// header carries the configured credentials; otherwise returns a `401` with a
/// Basic challenge.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let cfg = &state.config;
    if cfg.sparql_auth_disabled {
        return next.run(req).await;
    }
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|h| check_basic(h, &cfg.sparql_auth_user, &cfg.sparql_auth_password))
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        challenge()
    }
}

/// Validate an `Authorization: Basic <base64(user:pass)>` header against the
/// expected credentials.
fn check_basic(header_value: &str, user: &str, password: &str) -> bool {
    let encoded = match header_value
        .strip_prefix("Basic ")
        .or_else(|| header_value.strip_prefix("basic "))
    {
        Some(rest) => rest.trim(),
        None => return false,
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(creds) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let Some((u, p)) = creds.split_once(':') else {
        return false;
    };
    // Non-short-circuiting compares so a wrong username can't be distinguished
    // from a wrong password by timing.
    constant_eq(u.as_bytes(), user.as_bytes()) & constant_eq(p.as_bytes(), password.as_bytes())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn challenge() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, format!("Basic realm=\"{REALM}\""))],
        "authentication required",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(user: &str, pass: &str) -> String {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {token}")
    }

    #[test]
    fn accepts_correct_credentials() {
        assert!(check_basic(&basic("dba", "dba"), "dba", "dba"));
    }

    #[test]
    fn rejects_wrong_password_and_user() {
        assert!(!check_basic(&basic("dba", "nope"), "dba", "dba"));
        assert!(!check_basic(&basic("nope", "dba"), "dba", "dba"));
    }

    #[test]
    fn rejects_malformed_header() {
        assert!(!check_basic("Bearer abc", "dba", "dba"));
        assert!(!check_basic("Basic !!!notbase64", "dba", "dba"));
        assert!(!check_basic("Basic", "dba", "dba"));
    }
}
