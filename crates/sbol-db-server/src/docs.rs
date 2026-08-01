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
//! Both references are wrapped in the product-owned Design Ledger shell while
//! Scalar continues to own source switching and interactive API exploration.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const OPENAPI_JSON: &str = include_str!("openapi.json");
const SYNBIOHUB_OPENAPI_JSON: &str = include_str!("synbiohub_openapi.json");
const DOCS_STYLE: &str = include_str!("docs_style.css");

const DOCS_BODY: &str = r#"
    <div id="app"></div>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.62.9"></script>
    <script>
      Scalar.createApiReference(document.getElementById("app"), {
        theme: "none",
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
"#;

/// Shared API-reference shell for the native and V2 documentation routes.
///
/// Scalar remains responsible for the interactive reference while this shell
/// supplies the same SBOL vocabulary, type, color, and navigation used by the
/// public registry and admin control plane.
pub(crate) fn docs_page(title: &str, surface: &str, body: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en">
  <head>
    <title>{title}</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="theme-color" content="#faf8f2" />
    <link rel="icon" href="data:," />
    <style>{style}</style>
  </head>
  <body data-sbol-docs-shell>
    <header class="api-masthead">
      <a class="api-brand" href="/" aria-label="SBOL DB registry">
        <svg class="api-mark" viewBox="0 0 40 40" role="img" aria-label="SBOL design rail">
          <path d="M5 21h30" stroke="#667085" stroke-width="1.5" />
          <path d="M9 25V12h7" fill="none" stroke="#d9772b" stroke-width="2.2" />
          <path d="m16 12-4-3v6z" fill="#d9772b" />
          <path d="M19 16h10l4 5-4 5H19z" fill="#167866" />
          <path d="M34 13v16M31 13h6" fill="none" stroke="#c94f43" stroke-width="2.2" />
        </svg>
        <span class="api-brand-copy">
          <span class="api-brand-name">SBOL DB</span>
          <span class="api-surface">{surface}</span>
        </span>
      </a>
      <nav class="api-nav" aria-label="Product navigation">
        <a href="/">Registry</a>
        <a href="/docs">All APIs</a>
        <a href="/api/v2/docs">V2 API</a>
      </nav>
    </header>
    {body}
  </body>
</html>
"##,
        style = DOCS_STYLE
    )
}

pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

/// `GET /synbiohub/openapi.json` — the SynBioHub v1-compatible surface.
pub async fn synbiohub_openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], SYNBIOHUB_OPENAPI_JSON)
}

pub async fn docs_html() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        docs_page("SBOL DB / API reference", "API reference", DOCS_BODY),
    )
}
