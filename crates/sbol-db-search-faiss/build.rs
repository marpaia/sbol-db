use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FAISS_DIR");
    println!("cargo:rerun-if-env-changed=FAISS_INCLUDE_DIR");
    if env::var_os("CARGO_FEATURE_NATIVE").is_none() {
        println!("cargo:rustc-env=SBOL_DB_FAISS_VERSION=disabled");
        return;
    }

    let include = env::var_os("FAISS_INCLUDE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("FAISS_DIR").map(|path| PathBuf::from(path).join("include")))
        .or_else(platform_include);
    let version = include
        .as_deref()
        .and_then(read_version)
        .unwrap_or_else(|| "1.14.x-unknown".to_owned());
    println!("cargo:rustc-env=SBOL_DB_FAISS_VERSION={version}");
}

fn platform_include() -> Option<PathBuf> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(PathBuf::from("/opt/homebrew/opt/faiss/include"))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/usr/local/include"))
    } else {
        None
    }
}

fn read_version(include: &Path) -> Option<String> {
    let source = fs::read_to_string(include.join("faiss/Index.h")).ok()?;
    let major = macro_value(&source, "FAISS_VERSION_MAJOR")?;
    let minor = macro_value(&source, "FAISS_VERSION_MINOR")?;
    let patch = macro_value(&source, "FAISS_VERSION_PATCH")?;
    Some(format!("{major}.{minor}.{patch}"))
}

fn macro_value<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    source.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some("#define") && parts.next() == Some(name))
            .then(|| parts.next())
            .flatten()
    })
}
