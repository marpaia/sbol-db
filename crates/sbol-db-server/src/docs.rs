//! Interactive API documentation served at `/docs`, plus the OpenAPI 3.1
//! schema served at `/openapi.json`. The spec is hand-written and embedded
//! at compile time so the docs page is fully self-contained (no client-side
//! generator, no compile-time annotations spread across crates).
//!
//! The UI is rendered by [Scalar](https://github.com/scalar/scalar), pinned to
//! a fixed CDN version. `Scalar.createApiReference` mounts a single reference
//! carrying three same-origin documents, each a switcher entry: `/openapi.json`
//! (the original SBOL DB API), `/synbiohub/openapi.json` (the SynBioHub
//! v1-compatibility API), and `/api/v2/openapi.json` (the native V2 API). The
//! multi-document `sources` configuration is driven through the explicit
//! `createApiReference` call; the attribute-based auto-mount does not honor it.
//! The look-and-feel is closest to FastAPI's auto-generated `/docs` of the
//! modern OpenAPI UIs.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const OPENAPI_JSON: &str = include_str!("openapi.json");
const SYNBIOHUB_OPENAPI_JSON: &str = include_str!("synbiohub_openapi.json");

const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <title>SBOL DB API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="icon" href="data:," />
    <style>
      body { margin: 0; }
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.62.9"></script>
    <script>
      Scalar.createApiReference(document.getElementById("app"), {
        theme: "purple",
        layout: "modern",
        hideClientButton: false,
        sources: [
          {
            title: "SBOL DB native API",
            slug: "native",
            url: "/openapi.json",
            default: true
          },
          {
            title: "SynBioHub v1 compatibility API",
            slug: "synbiohub-v1",
            url: "/synbiohub/openapi.json"
          },
          {
            title: "SBOL DB V2 API",
            slug: "sbol-db-v2",
            url: "/api/v2/openapi.json"
          }
        ]
      });
    </script>
  </body>
</html>
"#;

pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

/// `GET /synbiohub/openapi.json` — the SynBioHub v1-compatible surface.
pub async fn synbiohub_openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], SYNBIOHUB_OPENAPI_JSON)
}

pub async fn docs_html() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], DOCS_HTML)
}
