use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use rust_embed::{EmbeddedFile, RustEmbed};

#[derive(RustEmbed)]
#[folder = "$OUT_DIR/ui-dist/"]
struct Assets;

/// A single embedded file resolved from a request path. Exposed for
/// callers that want to serve the same assets through a non-axum path
/// (custom handler, integration tests, etc.).
pub struct EmbeddedAsset {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Resolve a path inside the embedded asset tree. Returns `None` when
/// the path doesn't exist; the caller decides what to do (SPA fallback
/// to `index.html`, 404, …).
pub fn get_asset(path: &str) -> Option<EmbeddedAsset> {
    let normalized = path.trim_start_matches('/');
    let normalized = if normalized.is_empty() {
        "index.html"
    } else {
        normalized
    };
    Assets::get(normalized).map(|f| EmbeddedAsset {
        path: normalized.to_string(),
        bytes: f.data.into_owned(),
        mime: mime_guess::from_path(normalized)
            .first_or_octet_stream()
            .to_string(),
    })
}

/// True when an `index.html` exists in the embed, i.e. the UI was built
/// successfully.
pub fn is_built() -> bool {
    Assets::get("index.html").is_some()
}

/// axum router that serves the SPA. Mount under whatever prefix the host
/// server wants — the SPA is built with `base: "/lab/"` in vite config,
/// so the natural mount point is `/lab`.
pub fn router() -> Router {
    Router::new()
        .route("/", get(handle))
        .route("/*path", get(handle))
}

async fn handle(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    match Assets::get(path) {
        Some(file) => respond(path, file),
        // SPA fallback: unknown paths (client-side routes) return
        // index.html so React Router can take over.
        None => match Assets::get("index.html") {
            Some(index) => respond("index.html", index),
            None => stub_response(),
        },
    }
}

fn respond(path: &str, file: EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let cache = if path.starts_with("assets/") {
        // Vite emits hashed asset filenames in assets/; safe to cache
        // aggressively. index.html is fingerprinted by reference, not
        // by URL, so it must always revalidate.
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mime_header = HeaderValue::from_str(&mime)
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_header)
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(file.data.into_owned()))
        .expect("static response is well-formed")
}

fn stub_response() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(include_str!("stub.html")))
        .expect("stub response is well-formed")
}
