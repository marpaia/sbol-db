//! Embedded TypeScript SPA for the sbol-db query lab bench.
//!
//! The crate exposes a single axum sub-router that serves the compiled
//! Vite app. The host server decides whether and where to mount it; this
//! crate is intentionally unaware of authentication, configuration, and
//! the JSON API endpoints that the SPA talks to. Those live in
//! `sbol-db-server::lab`.
//!
//! Assets are baked into the binary at compile time by `rust-embed`,
//! reading from `$OUT_DIR/ui-dist/` (populated by `build.rs`). If the
//! UI was never built — because Node wasn't available or
//! `SBOL_DB_SKIP_UI_BUILD` was set — every route falls back to a stub
//! HTML page explaining how to rebuild.

mod assets;

pub use assets::{get_asset, router, EmbeddedAsset};

/// Whether the embed contains a built UI. Returns `false` when the
/// crate was compiled without a successful `npm run build` (cross-compile,
/// air-gapped CI, etc.).
pub fn is_built() -> bool {
    assets::is_built()
}
