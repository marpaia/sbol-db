//! Interactive API documentation served at `/docs`, plus the OpenAPI 3.1
//! schema served at `/openapi.json`. The spec is hand-written and embedded
//! at compile time so the docs page is fully self-contained (no client-side
//! generator, no compile-time annotations spread across crates).
//!
//! The UI is rendered by [Scalar](https://github.com/scalar/scalar), pinned to
//! a fixed CDN version and pointed at `/openapi.json` (the V1 SynBioHub-compat
//! surface plus the native/lab endpoints). The idiomatic V2 surface has its own
//! reference at `/api/v2/docs`; each page carries a link to the other. The
//! look-and-feel is closest to FastAPI's auto-generated `/docs` of the modern
//! OpenAPI UIs.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const OPENAPI_JSON: &str = include_str!("openapi.json");

const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <title>sbol-db API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="icon" href="data:," />
    <style>
      body { margin: 0; }
    </style>
  </head>
  <body>
    <a href="/api/v2/docs"
       style="position:fixed;top:10px;right:12px;z-index:2147483647;
              font:600 13px system-ui,sans-serif;padding:6px 12px;
              background:#7b5cff;color:#fff;border-radius:6px;
              text-decoration:none;">V2 API &rarr;</a>
    <script id="api-reference" data-url="/openapi.json"></script>
    <script>
      var configuration = {
        theme: "purple",
        layout: "modern",
        hideClientButton: false
      };
      document.getElementById("api-reference").dataset.configuration =
        JSON.stringify(configuration);
    </script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.62.9"></script>
  </body>
</html>
"#;

pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

pub async fn docs_html() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], DOCS_HTML)
}
