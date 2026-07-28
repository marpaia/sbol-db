use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use sbol_db_search_sdk::VectorError;
use sha3::{Digest, Sha3_256};
use tempfile::NamedTempFile;

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), VectorError> {
    let parent = path
        .parent()
        .ok_or_else(|| VectorError::Backend(format!("path {:?} has no parent directory", path)))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(io_error)?;
    temporary.write_all(bytes).map_err(io_error)?;
    temporary.flush().map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    temporary
        .persist(path)
        .map_err(|error| io_error(error.error))?;
    sync_directory(parent)
}

pub(crate) fn atomic_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), VectorError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| VectorError::Backend(format!("cannot serialize {:?}: {error}", path)))?;
    atomic_write(path, &bytes)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, VectorError> {
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| VectorError::Backend(format!("cannot parse {:?}: {error}", path)))
}

pub(crate) fn checksum_file(path: &Path) -> Result<String, VectorError> {
    let mut file = File::open(path).map_err(io_error)?;
    let mut digest = Sha3_256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub(crate) fn checksum_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha3_256::digest(bytes))
}

pub(crate) fn sync_directory(path: &Path) -> Result<(), VectorError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

pub(crate) fn copy_file(source: &Path, target: &Path) -> Result<(), VectorError> {
    let bytes = fs::read(source).map_err(io_error)?;
    atomic_write(target, &bytes)
}

pub(crate) fn io_error(error: std::io::Error) -> VectorError {
    VectorError::Backend(error.to_string())
}
