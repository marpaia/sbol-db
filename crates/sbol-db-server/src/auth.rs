//! HTTP authentication for the write SPARQL endpoints
//! (`/sparql-auth`, `/sparql-graph-crud-auth/`).
//!
//! Virtuoso protects these endpoints with Digest authentication: it answers
//! unauthenticated requests with a `401` carrying a
//! `WWW-Authenticate: Digest realm="SPARQL", … qop="auth", algorithm=MD5`
//! challenge and rejects preemptive Basic credentials. SynBioHub's clients are
//! challenge-driven (classic SynBioHub via the `request` library with
//! `sendImmediately: false`, synbiohub3 via Apache HttpClient 5) and answer
//! whatever scheme the challenge offers.
//!
//! This middleware issues the same Digest challenge shape as Virtuoso and
//! verifies RFC 2617 MD5 / `qop=auth` responses. Unlike Virtuoso it also
//! accepts preemptive `Authorization: Basic` credentials, so simple clients
//! (python `requests` with `HTTPBasicAuth`, curl without `--digest`) keep
//! working without the challenge round-trip.
//!
//! Nonces are stateless: `hex(unix_secs).hex(mac)` where the MAC is SHA3-256
//! over a random per-process secret and the timestamp. A structurally valid
//! Digest response whose nonce is expired or unknown (e.g. minted before a
//! server restart) gets a `stale="true"` challenge, telling the client to
//! retry with a fresh nonce rather than re-prompt for credentials. Deployments
//! that load-balance across several instances need sticky routing for the
//! challenge round-trip, since each process mints its own nonce secret.
//!
//! Before answering `401` the middleware drains the request body (bounded by
//! [`DRAIN_CAP_BYTES`]). Challenge-driven clients stream the full upload on
//! the first, unauthenticated attempt; responding without reading it makes
//! hyper reset the connection mid-body, which surfaces client-side as a write
//! EPIPE instead of a retryable challenge.
//!
//! Reads (`/sparql`) stay unauthenticated, matching Virtuoso's public endpoint.

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

use crate::AppState;

/// Realm mirrors Virtuoso's challenge so credential caches keyed by realm
/// transfer between backends.
const REALM: &str = "SPARQL";
/// Opaque value clients echo back; carries no state (hex of "sbol-db").
const OPAQUE: &str = "73626f6c2d6462";
/// How long an issued nonce authenticates requests before clients are asked
/// to re-handshake via `stale="true"`.
const NONCE_TTL_SECS: u64 = 300;
/// Upper bound on how much of an unauthenticated request body gets drained
/// before the `401` is sent. Sized above SynBioHub's chunked n3 uploads; a
/// body past the cap still gets the challenge but loses the keep-alive.
const DRAIN_CAP_BYTES: usize = 256 * 1024 * 1024;

/// Middleware guarding the authenticated SPARQL endpoints. Lets the request
/// through when `sparql_auth_disabled` is set, the `Authorization` header
/// carries the configured credentials as preemptive Basic, or it answers a
/// Digest challenge correctly; otherwise drains the body and returns a `401`
/// with a Virtuoso-shaped Digest challenge.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let cfg = &state.config;
    if cfg.sparql_auth_disabled {
        return next.run(req).await;
    }
    let outcome = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|h| {
            check_header(
                h,
                req.method(),
                &cfg.sparql_auth_user,
                &cfg.sparql_auth_password,
                unix_now(),
            )
        })
        .unwrap_or(AuthOutcome::Rejected);

    match outcome {
        AuthOutcome::Authorized => next.run(req).await,
        AuthOutcome::Stale => {
            drain_body(req).await;
            challenge(true)
        }
        AuthOutcome::Rejected => {
            drain_body(req).await;
            challenge(false)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AuthOutcome {
    Authorized,
    /// Credentials verified but against an expired or unknown nonce; the
    /// client should retry with a fresh nonce (`stale="true"`).
    Stale,
    Rejected,
}

fn check_header(
    header_value: &str,
    method: &Method,
    user: &str,
    password: &str,
    now_secs: u64,
) -> AuthOutcome {
    if scheme_rest(header_value, "basic").is_some() {
        if check_basic(header_value, user, password) {
            AuthOutcome::Authorized
        } else {
            AuthOutcome::Rejected
        }
    } else if let Some(rest) = scheme_rest(header_value, "digest") {
        check_digest(rest, method.as_str(), user, password, now_secs)
    } else {
        AuthOutcome::Rejected
    }
}

/// If `header_value` starts with `scheme` (case-insensitive) followed by
/// whitespace, return the remainder.
fn scheme_rest<'a>(header_value: &'a str, scheme: &str) -> Option<&'a str> {
    let (head, rest) = header_value.split_at_checked(scheme.len())?;
    if head.eq_ignore_ascii_case(scheme) && rest.starts_with(' ') {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Validate an `Authorization: Basic <base64(user:pass)>` header against the
/// expected credentials.
fn check_basic(header_value: &str, user: &str, password: &str) -> bool {
    let Some(encoded) = scheme_rest(header_value, "basic") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
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

/// Verify an RFC 2617 Digest response (`MD5`, plain or `qop=auth`) against the
/// configured credentials. `params` is the header value after the `Digest `
/// scheme token.
fn check_digest(
    params: &str,
    method: &str,
    user: &str,
    password: &str,
    now_secs: u64,
) -> AuthOutcome {
    let params = parse_digest_params(params);
    let (Some(username), Some(realm), Some(nonce), Some(uri), Some(response)) = (
        params.get("username"),
        params.get("realm"),
        params.get("nonce"),
        params.get("uri"),
        params.get("response"),
    ) else {
        return AuthOutcome::Rejected;
    };
    if let Some(alg) = params.get("algorithm") {
        if !alg.eq_ignore_ascii_case("MD5") {
            return AuthOutcome::Rejected;
        }
    }

    // The client computed HA1/HA2 from the realm and URI it sends, so the
    // verification uses those same values; identity is enforced through the
    // username and the shared password.
    let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let expected = match params.get("qop") {
        None => md5_hex(&format!("{ha1}:{nonce}:{ha2}")),
        Some(qop) if qop.eq_ignore_ascii_case("auth") => {
            let (Some(nc), Some(cnonce)) = (params.get("nc"), params.get("cnonce")) else {
                return AuthOutcome::Rejected;
            };
            md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"))
        }
        // `auth-int` is unsupported (as in Virtuoso's challenge, which offers
        // only `qop="auth"`).
        Some(_) => return AuthOutcome::Rejected,
    };

    let credentials_ok = constant_eq(username.as_bytes(), user.as_bytes())
        & constant_eq(
            expected.as_bytes(),
            response.to_ascii_lowercase().as_bytes(),
        );
    if !credentials_ok {
        return AuthOutcome::Rejected;
    }
    if nonce_is_current(nonce, now_secs) {
        AuthOutcome::Authorized
    } else {
        AuthOutcome::Stale
    }
}

/// Parse the comma-separated `key=value` list of a Digest header, honoring
/// quoted values (which may contain commas, e.g. in the `uri` query string).
/// Keys are lowercased.
fn parse_digest_params(s: &str) -> HashMap<String, String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quotes => {
                escaped = true;
                current.push(c);
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            ',' if !in_quotes => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);

    let mut params = HashMap::new();
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            params.insert(key.trim().to_ascii_lowercase(), unquote(value));
        }
    }
    params
}

/// Strip surrounding quotes and resolve `\"`-style escapes.
fn unquote(value: &str) -> String {
    let value = value.trim();
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .filter(|_| value.len() >= 2)
    else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for c in inner.chars() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    out
}

fn md5_hex(input: &str) -> String {
    use md5::{Digest as _, Md5};
    hex::encode(Md5::digest(input.as_bytes()))
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Per-process nonce MAC secret.
fn nonce_secret() -> &'static [u8; 32] {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    SECRET.get_or_init(|| {
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        secret[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        secret
    })
}

/// MAC over the nonce timestamp. SHA3-256 is not length-extendable, so
/// hashing `secret || timestamp` directly is a sound MAC.
fn nonce_mac(ts_secs: u64) -> String {
    use sha3::{Digest as _, Sha3_256};
    let mut hasher = Sha3_256::new();
    hasher.update(nonce_secret());
    hasher.update(ts_secs.to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn issue_nonce(now_secs: u64) -> String {
    format!("{now_secs:x}.{}", nonce_mac(now_secs))
}

/// Whether `nonce` was minted by this process within [`NONCE_TTL_SECS`].
fn nonce_is_current(nonce: &str, now_secs: u64) -> bool {
    let Some((ts_hex, mac)) = nonce.split_once('.') else {
        return false;
    };
    let Ok(ts) = u64::from_str_radix(ts_hex, 16) else {
        return false;
    };
    if !constant_eq(mac.as_bytes(), nonce_mac(ts).as_bytes()) {
        return false;
    }
    now_secs
        .checked_sub(ts)
        .is_some_and(|age| age <= NONCE_TTL_SECS)
}

/// Consume the request body (up to [`DRAIN_CAP_BYTES`]) so streaming clients
/// finish their upload and read the `401` instead of hitting a mid-body reset.
async fn drain_body(req: Request) {
    use futures::StreamExt as _;
    let mut stream = req.into_body().into_data_stream();
    let mut budget = DRAIN_CAP_BYTES;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                budget = budget.saturating_sub(bytes.len());
                if budget == 0 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn challenge(stale: bool) -> Response {
    let nonce = issue_nonce(unix_now());
    let stale = if stale { "true" } else { "false" };
    let value = format!(
        "Digest realm=\"{REALM}\", domain=\"/sparql-auth /sparql-graph-crud-auth\", \
         nonce=\"{nonce}\", opaque=\"{OPAQUE}\", stale=\"{stale}\", qop=\"auth\", algorithm=MD5"
    );
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, value)],
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

    /// Build the client side of a `qop=auth` Digest exchange.
    fn digest_params(user: &str, pass: &str, method: &str, uri: &str, nonce: &str) -> String {
        let ha1 = md5_hex(&format!("{user}:{REALM}:{pass}"));
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        let nc = "00000001";
        let cnonce = "0a4f113b";
        let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
        format!(
            "username=\"{user}\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", \
             qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\", \
             opaque=\"{OPAQUE}\", algorithm=MD5"
        )
    }

    #[test]
    fn accepts_correct_basic_credentials() {
        assert!(check_basic(&basic("dba", "dba"), "dba", "dba"));
    }

    #[test]
    fn rejects_wrong_basic_password_and_user() {
        assert!(!check_basic(&basic("dba", "nope"), "dba", "dba"));
        assert!(!check_basic(&basic("nope", "dba"), "dba", "dba"));
    }

    #[test]
    fn rejects_malformed_basic_header() {
        assert!(!check_basic("Bearer abc", "dba", "dba"));
        assert!(!check_basic("Basic !!!notbase64", "dba", "dba"));
        assert!(!check_basic("Basic", "dba", "dba"));
    }

    #[test]
    fn digest_qop_auth_round_trip_authorizes() {
        let now = 1_700_000_000;
        let nonce = issue_nonce(now);
        let params = digest_params("dba", "dba", "POST", "/sparql-auth?query=x", &nonce);
        assert_eq!(
            check_digest(&params, "POST", "dba", "dba", now),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn digest_without_qop_authorizes() {
        let now = 1_700_000_000;
        let nonce = issue_nonce(now);
        let uri = "/sparql-graph-crud-auth/";
        let ha1 = md5_hex(&format!("dba:{REALM}:dba"));
        let ha2 = md5_hex(&format!("PUT:{uri}"));
        let response = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
        let params = format!(
            "username=\"dba\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", \
             response=\"{response}\""
        );
        assert_eq!(
            check_digest(&params, "PUT", "dba", "dba", now),
            AuthOutcome::Authorized
        );
    }

    #[test]
    fn digest_rejects_wrong_password_and_method_mismatch() {
        let now = 1_700_000_000;
        let nonce = issue_nonce(now);
        let bad_pass = digest_params("dba", "wrong", "POST", "/sparql-auth", &nonce);
        assert_eq!(
            check_digest(&bad_pass, "POST", "dba", "dba", now),
            AuthOutcome::Rejected
        );
        // The method is bound into HA2, so a response minted for POST does not
        // authorize a DELETE.
        let post_only = digest_params("dba", "dba", "POST", "/sparql-auth", &nonce);
        assert_eq!(
            check_digest(&post_only, "DELETE", "dba", "dba", now),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn digest_expired_or_foreign_nonce_is_stale() {
        let minted = 1_700_000_000;
        let nonce = issue_nonce(minted);
        let params = digest_params("dba", "dba", "POST", "/sparql-auth", &nonce);
        let late = minted + NONCE_TTL_SECS + 1;
        assert_eq!(
            check_digest(&params, "POST", "dba", "dba", late),
            AuthOutcome::Stale
        );
        // A nonce this process never minted (e.g. issued before a restart)
        // also asks for a re-handshake rather than rejecting the credentials.
        let foreign = format!("{minted:x}.{}", "0".repeat(32));
        let params = digest_params("dba", "dba", "POST", "/sparql-auth", &foreign);
        assert_eq!(
            check_digest(&params, "POST", "dba", "dba", minted),
            AuthOutcome::Stale
        );
    }

    #[test]
    fn digest_params_parse_quoted_commas_and_escapes() {
        let params = parse_digest_params(
            "username=\"d\\\"ba\", uri=\"/sparql-auth?query=SELECT,%20x\", qop=auth, NC=00000001",
        );
        assert_eq!(params["username"], "d\"ba");
        assert_eq!(params["uri"], "/sparql-auth?query=SELECT,%20x");
        assert_eq!(params["qop"], "auth");
        assert_eq!(params["nc"], "00000001");
    }

    #[test]
    fn header_scheme_dispatch() {
        let now = 1_700_000_000;
        assert_eq!(
            check_header(&basic("dba", "dba"), &Method::POST, "dba", "dba", now),
            AuthOutcome::Authorized
        );
        let nonce = issue_nonce(now);
        let digest = format!(
            "digest {}",
            digest_params("dba", "dba", "POST", "/sparql-auth", &nonce)
        );
        assert_eq!(
            check_header(&digest, &Method::POST, "dba", "dba", now),
            AuthOutcome::Authorized
        );
        assert_eq!(
            check_header("Bearer abc", &Method::POST, "dba", "dba", now),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn challenge_is_virtuoso_shaped() {
        let res = challenge(false);
        let value = res
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .expect("challenge header")
            .to_owned();
        assert!(value.starts_with("Digest realm=\"SPARQL\""), "{value}");
        assert!(value.contains("qop=\"auth\""), "{value}");
        assert!(value.contains("algorithm=MD5"), "{value}");
        assert!(value.contains("stale=\"false\""), "{value}");
    }

    #[test]
    fn nonce_lifecycle() {
        let now = 1_700_000_000;
        let nonce = issue_nonce(now);
        assert!(nonce_is_current(&nonce, now));
        assert!(nonce_is_current(&nonce, now + NONCE_TTL_SECS));
        assert!(!nonce_is_current(&nonce, now + NONCE_TTL_SECS + 1));
        // A nonce timestamped in the future (clock stepped back) is not
        // current; the resulting stale challenge re-mints from the new clock.
        assert!(!nonce_is_current(&nonce, now - 1));
        assert!(!nonce_is_current("garbage", now));
        assert!(!nonce_is_current(&format!("{now:x}.deadbeef"), now));
    }
}
