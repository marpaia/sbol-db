use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::filesystem::prepare_directory;
use super::layout::ManagedDataLayout;
use crate::cli::{BackendKind, RuntimeProfile};

pub(crate) const DEFAULT_DATABASE_URL: &str = "postgres://sbol:sbol@localhost:5432/sbol";
const DEFAULT_DEVELOPMENT_DATA_DIR: &str = "sbol-db-data";

/// Fully resolved storage configuration held for the lifetime of the server.
#[derive(Debug)]
pub struct ServerRuntime {
    profile: RuntimeProfile,
    data_root: PathBuf,
    database_url: String,
    blob_root: PathBuf,
    layout: Option<ManagedDataLayout>,
}

impl ServerRuntime {
    /// Resolve the server's database and blob paths. Production is deliberately
    /// closed over one RocksDB topology; development keeps explicit backend
    /// selection available while still using a durable blob directory.
    pub fn resolve(
        profile: RuntimeProfile,
        data_dir: Option<&Path>,
        blob_root: Option<&Path>,
        backend: Option<BackendKind>,
        database_url: Option<&str>,
    ) -> Result<Self> {
        match profile {
            RuntimeProfile::Production => {
                let data_dir = data_dir
                    .context("--data-dir (or SBOL_DB_DATA_DIR) is required in production")?;
                if !data_dir.is_absolute() {
                    bail!("production --data-dir must be an absolute path");
                }
                if let Some(kind) = backend {
                    if kind != BackendKind::Rocksdb {
                        bail!(
                            "production profile manages a RocksDB appliance; \
                             --backend must be rocksdb or omitted"
                        );
                    }
                }
                if database_url.is_some() {
                    bail!(
                        "production profile derives its RocksDB path from --data-dir; \
                         remove --database-url/DATABASE_URL"
                    );
                }
                if blob_root.is_some() {
                    bail!(
                        "production profile derives its blob path from the active generation; \
                         remove --blob-root/SBOL_DB_BLOB_ROOT"
                    );
                }

                let layout = ManagedDataLayout::open(data_dir)?;
                let database_url = format!("rocksdb://{}", layout.database_path().display());
                let blob_root = layout.blob_root().to_path_buf();
                Ok(Self {
                    profile,
                    data_root: layout.root().to_path_buf(),
                    database_url,
                    blob_root,
                    layout: Some(layout),
                })
            }
            RuntimeProfile::Development => {
                let database_url = resolve_connection(backend, database_url)?;
                let data_dir = data_dir
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_DEVELOPMENT_DATA_DIR));
                let blob_root = blob_root
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| data_dir.join("blobs"));
                prepare_directory(&blob_root, "blob root")?;
                Ok(Self {
                    profile,
                    data_root: data_dir,
                    database_url,
                    blob_root,
                    layout: None,
                })
            }
        }
    }

    pub fn profile(&self) -> RuntimeProfile {
        self.profile
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// Root for process-level durable state. Production components that must
    /// participate in atomic backup/restore use the managed generation paths.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn blob_root(&self) -> &Path {
        &self.blob_root
    }

    pub fn layout(&self) -> Option<&ManagedDataLayout> {
        self.layout.as_ref()
    }
}

/// Resolve an ordinary CLI connection string outside the managed production
/// server. With a backend selector, a bare path gains that backend's scheme and
/// an explicitly conflicting scheme fails closed.
pub fn resolve_connection(backend: Option<BackendKind>, url: Option<&str>) -> Result<String> {
    let url = url.unwrap_or(DEFAULT_DATABASE_URL);
    let Some(backend) = backend else {
        return Ok(url.to_owned());
    };
    match url.split_once("://") {
        Some((scheme, _)) if backend.accepts_scheme(scheme) => Ok(url.to_owned()),
        Some((scheme, _)) => bail!(
            "--backend {} conflicts with --database-url scheme `{scheme}://`; \
             pass a {}:// connection string (or a bare path) or drop --backend",
            backend.scheme(),
            backend.scheme(),
        ),
        None => Ok(format!("{}://{url}", backend.scheme())),
    }
}
