//! Shared browser-session wire helpers.
//!
//! SynBioHub v1 and v2 resolve the same opaque API tokens through the
//! application facade. A browser carries that token in one `HttpOnly` cookie,
//! while API clients continue to use their generation-specific authorization headers.
//! Keeping cookie construction and parsing here prevents the two adapters from
//! drifting on security attributes or cookie names.

use axum::http::header::COOKIE;
use axum::http::HeaderMap;

pub(crate) const SESSION_COOKIE: &str = "sbol-db-token";

/// Read the shared session token from a `Cookie` header. Empty values and
/// similarly prefixed cookie names do not match.
pub(crate) fn token_from_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(name, value)| {
            let value = value.trim();
            (name.trim() == SESSION_COOKIE && !value.is_empty()).then_some(value)
        })
}

/// The value for `Set-Cookie` after a successful browser login.
pub(crate) fn login_cookie(token: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure}")
}

/// The value for `Set-Cookie` when logging a browser out.
pub(crate) fn logout_cookie(secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_exact_nonempty_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            "theme=dark; sbol-db-token=abc-123; another=x"
                .parse()
                .unwrap(),
        );
        assert_eq!(token_from_cookie(&headers), Some("abc-123"));

        headers.insert(COOKIE, "sbol-db-token-extra=nope".parse().unwrap());
        assert_eq!(token_from_cookie(&headers), None);

        headers.insert(COOKIE, "sbol-db-token=".parse().unwrap());
        assert_eq!(token_from_cookie(&headers), None);
    }

    #[test]
    fn secure_attribute_is_explicit() {
        assert!(!login_cookie("token", false).contains("; Secure"));
        assert!(login_cookie("token", true).ends_with("; Secure"));
        assert!(logout_cookie(true).ends_with("; Secure"));
    }
}
