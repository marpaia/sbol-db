//! Plugin proxying and the temp-file / async-stream handoffs.
//!
//! These are the public, authenticated (not admin-gated) endpoints classic
//! SynBioHub mounts for external plugins: `POST /callPlugin` proxies a request
//! to a configured plugin URL, `GET /expose/:id` serves a time-limited artifact
//! to a plugin, and `/stream/:id` is the async long-run handoff answering
//! `503 Retry-After` until the work resolves. The plugin registry, proxy client,
//! and handoff registries all live on the
//! [`AppServices`](sbol_db_app::AppServices) facade; these handlers are the thin
//! HTTP shell over them.
//!
//! Plugins re-fetch an object's SBOL through the P3 export routes
//! (`/…/sbol`, `/…/sbolnr`, `/…/gb`); a `run` request for a rendering or
//! download plugin carries those export URLs, built by the app-layer
//! [`PluginService`](sbol_db_app::PluginService).

mod call;
mod expose;
mod stream;

pub use call::call_plugin;
pub use expose::serve_expose;
pub use stream::{clear_stream, serve_stream};
