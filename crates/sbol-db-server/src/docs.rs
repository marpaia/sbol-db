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
//! Both reference pages use the same small product shell and map Scalar's
//! supported theme variables onto the SBOL DB light and dark palettes.

use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

const OPENAPI_JSON: &str = include_str!("openapi.json");
const SYNBIOHUB_OPENAPI_JSON: &str = include_str!("synbiohub_openapi.json");
const SCALAR_THEME_CSS: &str = include_str!("scalar-theme.css");
const SCALAR_CDN_URL: &str = "https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.62.9";

const INITIAL_THEME_SCRIPT: &str = r#"
      (() => {
        const storageKey = "sbol-lab:theme";
        const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");

        const readPreference = () => {
          try {
            const stored = localStorage.getItem(storageKey);
            if (stored === "light" || stored === "dark" || stored === "system") {
              return stored;
            }
          } catch (_) {
            // A blocked localStorage should not prevent the reference from loading.
          }
          return "system";
        };

        const applyRegistryTheme = () => {
          const preference = readPreference();
          const darkMode =
            preference === "dark" ||
            (preference === "system" && systemTheme.matches);

          window.sbolDocsDarkMode = darkMode;
          document.documentElement.classList.toggle("dark", darkMode);
          document.documentElement.style.colorScheme = darkMode ? "dark" : "light";
          document.body.classList.toggle("dark-mode", darkMode);
          document.body.classList.toggle("light-mode", !darkMode);
        };

        applyRegistryTheme();
        systemTheme.addEventListener("change", () => {
          if (readPreference() === "system") applyRegistryTheme();
        });
        window.addEventListener("storage", (event) => {
          if (event.key === storageKey || event.key === null) applyRegistryTheme();
        });
      })();
"#;

fn docs_header(subtitle: &str) -> String {
    format!(
        r#"<header class="sbol-docs-header">
      <a class="sbol-docs-brand" href="/" aria-label="Back to SBOL DB">
        <span class="sbol-docs-mark" aria-hidden="true">
          <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 9c5 0 6 14 11 14s6-14 11-14" />
            <path d="M5 23c5 0 6-14 11-14s6 14 11 14" />
            <path d="M8 11v10" />
            <path d="M24 11v10" />
            <path d="M16 13v6" />
          </svg>
        </span>
        <span class="sbol-docs-title-group">
          <span class="sbol-docs-title">SBOL DB</span>
          <span class="sbol-docs-subtitle">{subtitle}</span>
        </span>
      </a>
      <a class="sbol-docs-back" href="/">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        <span class="sbol-docs-back-label">Back to registry</span>
      </a>
    </header>"#
    )
}

pub(crate) fn render_scalar_docs_page(
    title: &str,
    subtitle: &str,
    reference_markup: &str,
    mount_script: &str,
) -> String {
    let header = docs_header(subtitle);
    format!(
        r##"<!doctype html>
<html lang="en">
  <head>
    <title>{title}</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="theme-color" content="#218380" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <style>
{SCALAR_THEME_CSS}
    </style>
  </head>
  <body>
    <script>{INITIAL_THEME_SCRIPT}
    </script>
    {header}
    {reference_markup}
    <script src="{SCALAR_CDN_URL}"></script>
    <script>{mount_script}
    </script>
  </body>
</html>
"##
    )
}

const REFERENCE_MARKUP: &str = r#"<div id="app"></div>"#;

const MOUNT_SCRIPT: &str = r#"
      Scalar.createApiReference(document.getElementById("app"), {
        theme: "none",
        layout: "modern",
        darkMode: window.sbolDocsDarkMode,
        forceDarkModeState: window.sbolDocsDarkMode ? "dark" : "light",
        hideDarkModeToggle: true,
        hideClientButton: false,
        mcp: {
          disabled: true
        },
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
"#;

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
        render_scalar_docs_page(
            "SBOL DB API Reference",
            "API reference",
            REFERENCE_MARKUP,
            MOUNT_SCRIPT,
        ),
    )
}
