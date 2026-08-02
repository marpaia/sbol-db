//! Compatibility-aware browser dispatch for the root-mounted SBOL DB web app.
//!
//! SynBioHub's V1 API owns browser-looking paths such as `/search`,
//! `/profile`, `/admin/*`, and canonical `/public/*` object identities. A
//! normal SPA fallback cannot coexist with those routes: Axum resolves the V1
//! handler first. Application mode therefore wraps the completed router and
//! short-circuits only explicit browser navigation to a known page path.
//! Machine requests, mutation methods, and legacy subresources continue into
//! the existing router unchanged.

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{ACCEPT, CACHE_CONTROL, VARY};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Intercept root application navigation ahead of Axum's method/path dispatch.
pub(crate) async fn dispatch(req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return next.run(req).await;
    }

    let is_head = req.method() == Method::HEAD;
    let path = req.uri().path();

    if is_asset_path(path) {
        return asset(path, is_head);
    }

    if is_page_path(path) && prefers_html(req.headers()) {
        return index(is_head);
    }

    next.run(req).await
}

/// Keep the administrator application usable when the public application is
/// disabled. The root-built bundle is also used by the transitional `/lab`
/// mount, so its assets, sign-in/setup entry points, and `/admin/*` deep links
/// still need root dispatch.
pub(crate) async fn dispatch_admin(req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if is_asset_path(path) {
        return asset(path, req.method() == Method::HEAD);
    }
    if is_admin_page_path(path) && prefers_html(req.headers()) {
        return index(req.method() == Method::HEAD);
    }
    next.run(req).await
}

fn index(is_head: bool) -> Response {
    let mut response = sbol_db_ui::index_response();
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("Accept"));
    if is_head {
        *response.body_mut() = Body::empty();
    }
    response
}

fn asset(path: &str, is_head: bool) -> Response {
    let mut response = match sbol_db_ui::asset_response(path) {
        Some(response) => response,
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(CACHE_CONTROL, "no-store")
            .body(Body::empty())
            .expect("asset 404 response is well-formed"),
    };
    if is_head {
        *response.body_mut() = Body::empty();
    }
    response
}

fn is_asset_path(path: &str) -> bool {
    path.starts_with("/assets/")
        || matches!(
            path,
            "/favicon.svg"
                | "/favicon.ico"
                | "/robots.txt"
                | "/site.webmanifest"
                | "/apple-touch-icon.png"
        )
}

fn is_admin_page_path(path: &str) -> bool {
    matches!(path, "/login" | "/setup") || is_path_family(path, "/admin")
}

/// The deliberately small browser-page allowlist.
///
/// Do not turn this into a catch-all: unknown root paths can be valid V1
/// extension or plugin endpoints. New frontend route families must be added
/// here together with dispatch tests.
fn is_page_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }

    if [
        "/login",
        "/register",
        "/profile",
        "/setup",
        "/submit",
        "/contribute",
        "/advanced-search",
        "/root-collections",
        "/sequence-search",
        "/sparql",
        "/change-password",
        "/connect",
        "/about",
        "/reset-password",
    ]
    .contains(&path)
    {
        return true;
    }

    if [
        "/search",
        "/collections",
        "/workspace",
        "/submissions",
        "/account",
        "/admin",
        "/tools",
        "/objects/view",
    ]
    .iter()
    .any(|root| is_path_family(path, root))
    {
        return true;
    }

    is_canonical_object_path(path)
}

fn is_path_family(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Browser pages exist only at bare persistent/versioned identities. V1
/// suffixes such as `/full`, `/remove`, `/uses`, `/attach`, and `/download`
/// remain machine endpoints even when a caller happens to send `text/html`.
fn is_canonical_object_path(path: &str) -> bool {
    let segments: Vec<_> = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    match segments.as_slice() {
        ["public", _, _] | ["user", _, _, _] => true,
        ["public", _, _, version] | ["user", _, _, _, version] => {
            !is_reserved_identity_suffix(version)
        }
        _ => false,
    }
}

/// Version-less V1 identities reuse the same path depth as several static
/// representation and relationship suffixes. Axum gives those static routes
/// precedence over `:version`; the portal classifier must make the same choice
/// before it sees the router.
fn is_reserved_identity_suffix(segment: &str) -> bool {
    matches!(
        segment,
        "full"
            | "sbol"
            | "sbolnr"
            | "gb"
            | "fasta"
            | "gff"
            | "omex"
            | "summary"
            | "metadata"
            | "subCollections"
            | "uses"
            | "usesCount"
            | "similar"
            | "similarCount"
            | "twins"
            | "twinsCount"
            | "attach"
            | "attachUrl"
            | "download"
            | "remove"
            | "replace"
            | "removeCollection"
            | "icon"
            | "copyFromRemote"
            | "makePublic"
            | "addOwner"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcceptKind {
    Html,
    Machine,
}

/// Return true only when explicit HTML is the caller's most-preferred
/// recognized representation. Missing headers and wildcards retain legacy API
/// behavior, as do equal-quality headers that list a machine type first.
fn prefers_html(headers: &HeaderMap) -> bool {
    let mut best: Option<(f32, usize, AcceptKind)> = None;
    let mut order = 0usize;

    for value in headers.get_all(ACCEPT).iter() {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for range in value.split(',') {
            let mut parts = range.split(';');
            let media_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let mut quality = 1.0f32;
            for parameter in parts {
                let Some((name, value)) = parameter.trim().split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("q") {
                    quality = value.trim().parse::<f32>().unwrap_or(0.0);
                }
            }

            let kind = match media_type.as_str() {
                "text/html" | "application/xhtml+xml" => Some(AcceptKind::Html),
                "*/*" => None,
                media if media.ends_with("/*") => None,
                "" => None,
                _ => Some(AcceptKind::Machine),
            };

            if quality > 0.0 {
                if let Some(kind) = kind {
                    let replace = best
                        .as_ref()
                        .map(|(best_quality, best_order, _)| {
                            quality > *best_quality
                                || (quality == *best_quality && order < *best_order)
                        })
                        .unwrap_or(true);
                    if replace {
                        best = Some((quality, order, kind));
                    }
                }
            }
            order += 1;
        }
    }

    matches!(best, Some((_, _, AcceptKind::Html)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(accept: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(accept) = accept {
            headers.insert(ACCEPT, HeaderValue::from_str(accept).unwrap());
        }
        headers
    }

    #[test]
    fn browser_navigation_prefers_explicit_html() {
        assert!(prefers_html(&headers(Some(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        ))));
        assert!(prefers_html(&headers(Some(
            "application/json;q=0.5, text/html;q=0.9"
        ))));
    }

    #[test]
    fn api_defaults_remain_machine_requests() {
        assert!(!prefers_html(&headers(None)));
        assert!(!prefers_html(&headers(Some("*/*"))));
        assert!(!prefers_html(&headers(Some("application/*"))));
        assert!(!prefers_html(&headers(Some(
            "application/json, text/html;q=0.1"
        ))));
        assert!(!prefers_html(&headers(Some("application/json, text/html"))));
        assert!(!prefers_html(&headers(Some("text/html;q=0, */*;q=1"))));
    }

    #[test]
    fn page_allowlist_includes_product_routes_and_bare_identities() {
        for path in [
            "/",
            "/login",
            "/search",
            "/search/objectType=ComponentDefinition",
            "/sequence-search",
            "/connect",
            "/about",
            "/advanced-search",
            "/collections/featured",
            "/workspace/shared",
            "/admin/operations/jobs",
            "/public/igem/BBa_F2620/1",
            "/public/igem/BBa_F2620",
            "/user/alice/designs/toggle/1",
            "/user/alice/designs/toggle",
        ] {
            assert!(is_page_path(path), "expected portal page: {path}");
        }
    }

    #[test]
    fn api_and_object_subresources_are_not_pages() {
        for path in [
            "/api/v2/search",
            "/healthz",
            "/browse",
            "/rootCollections",
            "/searchCount",
            "/public/igem/BBa_F2620/1/full",
            "/public/igem/BBa_F2620/1/remove",
            "/public/igem/BBa_F2620/sbol",
            "/public/igem/BBa_F2620/uses",
            "/user/alice/designs/toggle/sbolnr",
            "/user/alice/designs/toggle/1/download",
            "/callPlugin",
        ] {
            assert!(!is_page_path(path), "must remain an API path: {path}");
        }
    }
}
