//! Interactive API documentation served at `/docs`, plus the OpenAPI 3.1
//! schema served at `/openapi.json`. The spec is hand-written and embedded
//! at compile time so the docs page is fully self-contained (no client-side
//! generator, no compile-time annotations spread across crates).
//!
//! The UI is rendered by [Scalar](https://github.com/scalar/scalar), loaded
//! from a CDN. A single `<script>` tag fetches the renderer and points it at
//! two same-origin sources: `/openapi.json` (the V1 SynBioHub-compat surface
//! plus the native/lab endpoints) and `/api/v2/openapi.json` (the idiomatic
//! V2 surface, served by the V2 router). Scalar renders a source switcher so
//! both surfaces are reachable from one docs page. The look-and-feel is
//! closest to FastAPI's auto-generated `/docs` of the modern OpenAPI UIs.

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
    <script id="api-reference"></script>
    <script>
      var configuration = {
        theme: "purple",
        layout: "modern",
        hideClientButton: false,
        sources: [
          {
            title: "SBOL DB (V1 SynBioHub-compat + native/lab)",
            slug: "v1",
            url: "/openapi.json",
            default: true
          },
          {
            title: "SBOL DB V2 (idiomatic)",
            slug: "v2",
            url: "/api/v2/openapi.json"
          }
        ]
      };
      document.getElementById("api-reference").dataset.configuration =
        JSON.stringify(configuration);
    </script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>
"#;

pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

pub async fn docs_html() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], DOCS_HTML)
}
