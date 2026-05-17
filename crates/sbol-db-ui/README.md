# sbol-db-ui

The embedded TypeScript SPA for the sbol-db query lab bench.

This crate ships two things:

1. A small axum sub-router (`sbol_db_ui::router()`) that serves the
   compiled Vite app, with SPA fallback to `index.html` for client-side
   routes and aggressive caching for hashed `assets/*` files.
2. A `build.rs` that drives the Vite build automatically as part of
   `cargo build`. Artifacts go into `$OUT_DIR/ui-dist/`, so the source
   tree stays clean and `cargo clean` removes them.

The companion JSON API for SQL / SPARQL execution lives in
`sbol-db-server::lab` — this crate is intentionally unaware of it. The
SPA reaches the API at `/lab/api/*` (set by `vite.config.ts`'s `base`
plus the proxy used during dev).

## Building

The UI build is fully integrated into Cargo. From a fresh clone:

```sh
cargo build -p sbol-db
```

On the first build, `build.rs` runs `npm ci` (~30s) and `npm run build`
(~10s). Subsequent builds with no UI source changes are zero-overhead —
Cargo's `rerun-if-changed` tracking sees no inputs changed and skips
the build script entirely.

### Opt-outs

- `SBOL_DB_SKIP_UI_BUILD=1` — `build.rs` becomes a no-op; the binary
  embeds a stub HTML page instead of the real UI. Useful for
  cross-compile, air-gapped builds, or pre-built CI artifacts.
- `cargo build --no-default-features -p sbol-db-server` (with the `lab`
  feature off) — the server doesn't depend on `sbol-db-ui` at all, and
  the `/lab` routes aren't mounted.

If `npm` isn't on `PATH`, the build still succeeds with a stub page
and a `cargo:warning=` advising you to install Node.

## Development

For an HMR loop, run the Vite dev server alongside the Rust server:

```sh
# terminal 1
cargo run -p sbol-db -- serve
# terminal 2
cd crates/sbol-db-ui/ui && npm run dev
```

The Vite dev server (port 5173) proxies `/lab/api` and `/openapi.json`
to the Rust server (port 8080), so the SPA can talk to a real backend
while still getting React Refresh on every save.

For production-shape testing, build once and visit the embed:

```sh
cargo run -p sbol-db -- serve
# open http://localhost:8080/lab
```
