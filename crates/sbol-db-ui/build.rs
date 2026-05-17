//! Build script for `sbol-db-ui`.
//!
//! When the crate is compiled, this script invokes the Vite build that
//! lives under `ui/`, writing artifacts into `$OUT_DIR/ui-dist/` where
//! `rust-embed` picks them up. Cargo's `rerun-if-changed` machinery keeps
//! the npm build off the critical path when no UI source has changed.
//!
//! Behavior is robust to environments that don't have Node installed
//! (cross-compile, cargo-chef cook, air-gapped builds): a missing `npm`
//! emits a `cargo:warning=` and falls back to the stub HTML page. Setting
//! `SBOL_DB_SKIP_UI_BUILD=1` opts out explicitly.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let ui_dir = manifest_dir.join("ui");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let dist_dir = out_dir.join("ui-dist");

    // Ensure the embed target exists on every code path so rust-embed
    // never errors with "folder not found"; an empty directory is fine.
    std::fs::create_dir_all(&dist_dir).expect("create OUT_DIR/ui-dist");

    println!("cargo:rerun-if-env-changed=SBOL_DB_SKIP_UI_BUILD");
    emit_rerun_directives(&ui_dir);

    // The ui/ source isn't always present (cargo-chef cook stubs only the
    // Cargo manifests, not data directories). Treat its absence as a
    // no-op rather than a failure.
    if !ui_dir.join("package.json").exists() {
        println!(
            "cargo:warning=sbol-db-ui: ui/package.json not found; skipping UI build (stub assets will be served)"
        );
        return;
    }

    if env::var("SBOL_DB_SKIP_UI_BUILD").is_ok() {
        println!(
            "cargo:warning=sbol-db-ui: SBOL_DB_SKIP_UI_BUILD set; skipping UI build (stub assets will be served)"
        );
        return;
    }

    let Ok(npm) = which::which("npm") else {
        println!(
            "cargo:warning=sbol-db-ui: npm not found on PATH; UI will not be embedded. Install Node.js >= 20 or set SBOL_DB_SKIP_UI_BUILD=1 to silence."
        );
        return;
    };

    // `npm ci` is the cold-install step. Once `node_modules/` exists,
    // Cargo's rerun-if-changed tracking on package-lock.json takes care
    // of invalidating us on dep changes.
    if !ui_dir.join("node_modules").exists() {
        run(
            Command::new(&npm)
                .args(["ci", "--no-audit", "--no-fund"])
                .current_dir(&ui_dir),
            "npm ci",
        );
    }

    // `vite build --outDir <abs path>` writes directly into OUT_DIR so
    // build artifacts never pollute the source tree. `--emptyOutDir`
    // is required because the outDir is outside of the project root.
    run(
        Command::new(&npm)
            .args(["run", "build", "--", "--outDir"])
            .arg(&dist_dir)
            .arg("--emptyOutDir")
            .current_dir(&ui_dir),
        "vite build",
    );
}

fn emit_rerun_directives(ui_dir: &Path) {
    const WATCHED_FILES: &[&str] = &[
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
        "tsconfig.node.json",
        "tailwind.config.ts",
        "postcss.config.js",
        "components.json",
        "index.html",
    ];
    for f in WATCHED_FILES {
        let p = ui_dir.join(f);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    for sub in ["src", "public"] {
        let p = ui_dir.join(sub);
        if !p.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&p).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() {
                println!("cargo:rerun-if-changed={}", entry.path().display());
            }
        }
    }
}

fn run(cmd: &mut Command, label: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("sbol-db-ui: failed to spawn {label}: {e}"));
    if !status.success() {
        panic!("sbol-db-ui: {label} failed with status {status}");
    }
}
