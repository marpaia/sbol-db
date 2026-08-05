# sbol-db-ui

The embedded TypeScript application for SBOL DB and its administrator
data/operations workspace. SynBioHub behavior is exposed through compatibility
adapters; it is not the native product identity.

This crate ships two things:

1. Asset and index responders used by the server's compatibility-aware root
   dispatcher, plus the transitional `/lab` SPA service.
2. A `build.rs` that drives the Vite build automatically as part of
   `cargo build`. Artifacts go into `$OUT_DIR/ui-dist/`, so the source
   tree stays clean and `cargo clean` removes them.

The public portal consumes `/api/v2/*`. The companion admin JSON API for SQL,
SPARQL, schema, and operations lives in `sbol-db-server::lab` under
`/lab/api/*`; this asset crate remains unaware of either API's semantics.

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
  neither portal nor admin pages are mounted.

If `npm` isn't on `PATH`, the build still succeeds with a stub page
and a `cargo:warning=` advising you to install Node.

## Development

For an HMR loop, run the Vite dev server alongside the Rust server:

```sh
# terminal 1
cargo run -p sbol-db -- server
# terminal 2
cd crates/sbol-db-ui/ui && npm run dev
```

The Vite dev server (port 5173) mirrors the production browser/API dispatch and
forwards `/api`, `/lab/api`, and SBOL DB API paths to the Rust server (port
8888), while still providing React Refresh.

For production-shape testing, build once and visit the embed:

```sh
cargo run -p sbol-db -- server
# open http://localhost:8888/
```
