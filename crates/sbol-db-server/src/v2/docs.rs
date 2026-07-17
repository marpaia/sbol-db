//! The V2 OpenAPI schema and its docs page.
//!
//! `GET /api/v2/openapi.json` serves a hand-written OpenAPI 3.1 spec of the V2
//! surface, embedded at compile time so the docs page is self-contained.
//! `GET /api/v2/docs` renders it with [Scalar](https://github.com/scalar/scalar)
//! from a CDN, mirroring the native `/docs` page. Both are unauthenticated.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const OPENAPI_JSON: &str = include_str!("openapi.json");

const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <title>sbol-db V2 API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="icon" href="data:," />
    <style>
      body { margin: 0; }
    </style>
  </head>
  <body>
    <script id="api-reference" data-url="/api/v2/openapi.json"></script>
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

/// `GET /api/v2/openapi.json` — the embedded V2 OpenAPI 3.1 schema.
pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

/// `GET /api/v2/docs` — the interactive V2 API reference.
pub async fn docs_html() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], DOCS_HTML)
}
