#[cfg(feature = "native")]
use std::env;
#[cfg(feature = "native")]
use std::fs;
#[cfg(feature = "native")]
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=FAISS_DIR");
    println!("cargo:rerun-if-env-changed=FAISS_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=FAISS_LIB_DIR");
    #[cfg(not(feature = "native"))]
    println!("cargo:rustc-env=SBOL_DB_FAISS_VERSION=disabled");

    #[cfg(feature = "native")]
    configure_native();
}

#[cfg(feature = "native")]
fn configure_native() {
    let paths = FaissPaths::discover();
    let version = read_version(&paths.include_dir).unwrap_or_else(|| {
        panic!(
            "could not read FAISS version from {}",
            paths.include_dir.join("faiss/Index.h").display()
        )
    });
    assert!(
        version.0 == 1 && version.1 == 14,
        "FAISS {}.{}.{} is unsupported; expected 1.14.x",
        version.0,
        version.1,
        version.2
    );
    println!(
        "cargo:rustc-env=SBOL_DB_FAISS_VERSION={}.{}.{}",
        version.0, version.1, version.2
    );

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", paths.include_dir.display()))
        .derive_default(true)
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: true,
        })
        .layout_tests(false)
        .allowlist_function("faiss_.*")
        .allowlist_type("idx_t|Faiss.*")
        .opaque_type("FILE")
        .generate()
        .expect("generating target-native FAISS C bindings");
    bindings
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings.rs"))
        .expect("writing target-native FAISS C bindings");

    println!("cargo:rustc-link-search=native={}", paths.lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=faiss");
    println!("cargo:rustc-link-lib=dylib=faiss_c");
}

#[cfg(feature = "native")]
struct FaissPaths {
    include_dir: PathBuf,
    lib_dir: PathBuf,
}

#[cfg(feature = "native")]
impl FaissPaths {
    fn discover() -> Self {
        let prefix = env::var_os("FAISS_DIR").map(PathBuf::from);
        let include_dir = env::var_os("FAISS_INCLUDE_DIR")
            .map(PathBuf::from)
            .or_else(|| prefix.as_ref().map(|path| path.join("include")))
            .or_else(platform_include)
            .expect("set FAISS_DIR or FAISS_INCLUDE_DIR");
        let lib_dir = env::var_os("FAISS_LIB_DIR")
            .map(PathBuf::from)
            .or_else(|| prefix.as_ref().map(|path| path.join("lib")))
            .or_else(platform_lib)
            .expect("set FAISS_DIR or FAISS_LIB_DIR");
        assert!(
            include_dir.join("faiss/Index.h").is_file(),
            "FAISS headers not found under {}",
            include_dir.display()
        );
        assert!(
            lib_dir.is_dir(),
            "FAISS library directory not found: {}",
            lib_dir.display()
        );
        Self {
            include_dir,
            lib_dir,
        }
    }
}

#[cfg(feature = "native")]
fn platform_include() -> Option<PathBuf> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(PathBuf::from("/opt/homebrew/opt/faiss/include"))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(PathBuf::from("/usr/local/opt/faiss/include"))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/usr/local/include"))
    } else {
        None
    }
}

#[cfg(feature = "native")]
fn platform_lib() -> Option<PathBuf> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(PathBuf::from("/opt/homebrew/opt/faiss/lib"))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(PathBuf::from("/usr/local/opt/faiss/lib"))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/usr/local/lib"))
    } else {
        None
    }
}

#[cfg(feature = "native")]
fn read_version(include_dir: &Path) -> Option<(u32, u32, u32)> {
    let source = fs::read_to_string(include_dir.join("faiss/Index.h")).ok()?;
    Some((
        macro_value(&source, "FAISS_VERSION_MAJOR")?,
        macro_value(&source, "FAISS_VERSION_MINOR")?,
        macro_value(&source, "FAISS_VERSION_PATCH")?,
    ))
}

#[cfg(feature = "native")]
fn macro_value(source: &str, name: &str) -> Option<u32> {
    source.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some("#define") && parts.next() == Some(name))
            .then(|| parts.next()?.parse().ok())
            .flatten()
    })
}
