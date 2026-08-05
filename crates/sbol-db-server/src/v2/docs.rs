//! The SynBioHub v2 OpenAPI schema and its docs page.
//!
//! `GET /api/v2/openapi.json` serves a hand-written OpenAPI 3.1 spec of the
//! idiomatic SynBioHub product surface, embedded at compile time so the docs
//! page is self-contained.
//! `GET /api/v2/docs` renders it with [Scalar](https://github.com/scalar/scalar)
//! from a CDN, mirroring the combined `/docs` page. Both are unauthenticated.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

use crate::docs::render_scalar_docs_page;

const OPENAPI_JSON: &str = include_str!("openapi.json");

const REFERENCE_MARKUP: &str =
    r#"<script id="api-reference" data-url="/api/v2/openapi.json"></script>"#;

const MOUNT_SCRIPT: &str = r#"
      var configuration = {
        theme: "none",
        layout: "modern",
        darkMode: window.sbolDocsDarkMode,
        forceDarkModeState: window.sbolDocsDarkMode ? "dark" : "light",
        hideDarkModeToggle: true,
        hideClientButton: false,
        mcp: {
          disabled: true
        }
      };
      document.getElementById("api-reference").dataset.configuration =
        JSON.stringify(configuration);
"#;

/// `GET /api/v2/openapi.json` — the embedded SynBioHub v2 OpenAPI 3.1 schema.
pub async fn openapi_json() -> impl IntoResponse {
    ([(CONTENT_TYPE, "application/json")], OPENAPI_JSON)
}

/// `GET /api/v2/docs` — the interactive SynBioHub v2 API reference.
pub async fn docs_html() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        render_scalar_docs_page(
            "SynBioHub v2 API Reference",
            "SynBioHub v2 API reference",
            REFERENCE_MARKUP,
            MOUNT_SCRIPT,
        ),
    )
}
