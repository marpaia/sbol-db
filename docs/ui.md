# Data lab UI

`sbol-db` ships an embedded TypeScript SPA — the **data lab bench** — at
`/lab`, alongside the rest of the HTTP API (`/docs`, `/sparql`,
`/objects`, …). It's a React + Vite app that lives in the `sbol-db-ui`
crate; production builds bake the compiled assets into the binary via
`rust-embed`, so deploying it is exactly the same as deploying any
other route on the server.

The lab is fronted by two knobs:

- `SBOL_DB_LAB_ENABLED` (env, default `true`) — runtime toggle. When
  `false`, `/lab` returns 404 and the rest of the API is unaffected.
- `--no-default-features` on `sbol-db-server` (cargo, default on) —
  compile-time strip. Removes the `sbol-db-ui` dependency entirely;
  the binary ships without the embedded assets.

## Development

Two terminals: the Rust server provides the JSON API on port 8888, and
the Vite dev server provides the UI with hot module reload on port
5173. The Vite dev server's proxy forwards `/lab/api/*` and
`/openapi.json` to the Rust server, so the SPA talks to a real
backend while you iterate on the frontend.

```sh
# Terminal 1 — Rust server (also serves the embedded UI at :8888/lab,
# but during dev you'll point your browser at the Vite server below).
cargo run -p sbol-db -- server

# Terminal 2 — Vite dev server with React Refresh.
cd crates/sbol-db-ui/ui
npm run dev
```

Then open `http://localhost:5173/lab/`. Saves to any `.tsx`, `.ts`, or
`.css` file under `crates/sbol-db-ui/ui/src/` update the browser
instantly.

### First-time setup

The `cargo build` invocation that compiles `sbol-db-ui` will run
`npm ci` automatically on first build (or after a clean), via the
crate's `build.rs`. You don't need to install npm dependencies by
hand — Cargo drives the whole pipeline. The only prerequisite is
Node.js 20 or newer on `PATH`.

If you'd rather run the npm install yourself (e.g. to pick up a fresh
`package-lock.json` before a `cargo build`), the command is the same
one the build script uses:

```sh
cd crates/sbol-db-ui/ui
npm ci
```

### macOS SDK path override

If `cargo build` fails on macOS with `'sys/types.h' file not found`
from `pg_query`, your macOS SDK lives somewhere other than the
default Xcode path that `.cargo/config.toml` assumes (e.g. Command
Line Tools only, or a non-default Xcode install). Override at the
shell level:

```sh
export BINDGEN_EXTRA_CLANG_ARGS="-isysroot $(xcrun --show-sdk-path)"
```

### Production-shape testing

To exercise the binary-embedded path — the same code path that ships
in a container image — skip the Vite dev server and visit the Rust
server directly:

```sh
cargo run -p sbol-db -- server
# open http://localhost:8888/lab
```

This is what users see. The Vite dev server is purely a development
convenience; the binary at `localhost:8888/lab` serves the same
compiled assets that ship to production.

### Useful UI scripts

All run from `crates/sbol-db-ui/ui/`:

| Command            | Purpose                                                |
| ------------------ | ------------------------------------------------------ |
| `npm run dev`      | Vite dev server on `:5173` with HMR.                    |
| `npm run build`    | Production build. Normally driven by Cargo; useful manually for output inspection. |
| `npm run lint`     | ESLint over `src/`.                                     |
| `npm run typecheck`| `tsc -b --noEmit` over the project.                     |
| `npm run format`   | Prettier write.                                         |

### Opt-outs and edge cases

- **No Node installed?** The `cargo build` still succeeds. `build.rs`
  emits a `cargo:warning=` and embeds a stub HTML page explaining
  how to rebuild. `/lab` returns 503 with the stub.
- **Want a Rust-only build (CI, cross-compile, air-gapped)?** Set
  `SBOL_DB_SKIP_UI_BUILD=1` in the build environment; `build.rs`
  becomes a no-op and the stub page is embedded instead.
- **Want to disable the lab at runtime?** Set
  `SBOL_DB_LAB_ENABLED=false` before `sbol-db server`. The `/lab`
  routes aren't mounted; the rest of the API is unaffected.
