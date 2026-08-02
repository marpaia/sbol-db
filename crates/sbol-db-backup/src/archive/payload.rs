use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use sbol_db_core::ObjectTerm;
use sbol_db_rocksdb::{Db, RocksdbStore};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::filesystem::{
    create_private_file, file_mode, prepare_private_directory, set_file_mode, sync_directory,
    validate_portable_path, validate_source_directory,
};
use crate::manifest::{
    BackupComponent, BackupFileManifest, BackupManifest, LEGACY_ATTACHMENT_HASH, MAX_FILE_COUNT,
    SBOL2_HASH,
};

pub(super) fn collect_payload_files(payload_root: &Path) -> Result<Vec<BackupFileManifest>> {
    let mut paths = Vec::new();
    collect_regular_files(payload_root, &mut paths)?;
    paths.sort();
    if paths.len() > MAX_FILE_COUNT {
        bail!("backup payload exceeds the maximum file count {MAX_FILE_COUNT}");
    }
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(payload_root)
                .expect("collected below root");
            let portable = path_to_portable(relative)?;
            let component = BackupComponent::from_path(&portable)
                .with_context(|| format!("backup file has unknown component `{portable}`"))?;
            let metadata = fs::metadata(&path)?;
            Ok(BackupFileManifest {
                path: portable,
                component,
                size: metadata.len(),
                sha256: sha256_file(&path)?,
                mode: file_mode(&metadata),
            })
        })
        .collect()
}

fn collect_regular_files(current: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(current)
        .with_context(|| format!("read backup staging directory {}", current.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!("backup source contains a symbolic link: {}", path.display());
        }
        if metadata.is_dir() {
            collect_regular_files(&path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        } else {
            bail!("backup source contains a special file: {}", path.display());
        }
    }
    Ok(())
}

pub(super) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    validate_source_directory(source, "backup component")?;
    copy_tree_inner(source, destination)
}

fn copy_tree_inner(source: &Path, destination: &Path) -> Result<()> {
    prepare_private_directory(destination)?;
    let mut entries: Vec<_> = fs::read_dir(source)
        .with_context(|| format!("read backup source {}", source.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            bail!(
                "backup source contains a symbolic link: {}",
                source_path.display()
            );
        }
        if metadata.is_dir() {
            copy_tree_inner(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            let mut input = File::open(&source_path)?;
            let mut output = create_private_file(&destination_path)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            set_file_mode(&destination_path, file_mode(&metadata))?;
        } else {
            bail!(
                "backup source contains a special file: {}",
                source_path.display()
            );
        }
    }
    sync_directory(destination)?;
    Ok(())
}

pub(super) fn validate_blob_tree(
    manifest: &BackupManifest,
    payload_root: &Path,
) -> Result<BTreeSet<String>> {
    let mut hashes = BTreeSet::new();
    for file in manifest
        .files
        .iter()
        .filter(|file| file.component == BackupComponent::Blobs)
    {
        let parts: Vec<_> = file.path.split('/').collect();
        if parts.len() != 4 || parts[0] != "blobs" || parts[1] != "uploads" {
            bail!(
                "unexpected file in content-addressed blob tree: `{}`",
                file.path
            );
        }
        let shard = parts[2];
        let tail = parts[3]
            .strip_suffix(".gz")
            .with_context(|| format!("blob file lacks .gz suffix: `{}`", file.path))?;
        let hash = format!("{shard}{tail}");
        if shard.len() != 2
            || tail.len() != 38
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("invalid content-addressed blob path: `{}`", file.path);
        }
        let hash = hash.to_ascii_lowercase();
        if !hashes.insert(hash.clone()) {
            bail!("duplicate content-addressed blob `{hash}`");
        }
        let path = payload_root.join(portable_to_path(&file.path)?);
        let actual = sha1_gzip_payload(&path)?;
        if actual != hash {
            bail!("blob content hash does not match path for `{}`", file.path);
        }
    }
    Ok(hashes)
}

pub(super) fn validate_referenced_blobs(db: &Db, available: &BTreeSet<String>) -> Result<usize> {
    let store = RocksdbStore::new(db.clone());
    let triples = store.triple_source();
    let mut referenced = BTreeSet::new();
    for predicate in [SBOL2_HASH, LEGACY_ATTACHMENT_HASH] {
        for triple in triples
            .scan_pattern(None, Some(predicate), None, None, i64::MAX)
            .with_context(|| format!("scan attachment hashes for predicate {predicate}"))?
        {
            let ObjectTerm::Literal { value, .. } = triple.object else {
                bail!("attachment hash predicate has a non-literal object");
            };
            let hash = value.to_ascii_lowercase();
            if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("RocksDB references an invalid attachment hash `{value}`");
            }
            if !available.contains(&hash) {
                bail!("RocksDB references missing attachment blob `{hash}`");
            }
            referenced.insert(hash);
        }
    }
    Ok(referenced.len())
}

fn sha1_gzip_payload(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut decoder = GzDecoder::new(BufReader::new(file));
    let mut digest = Sha1::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = decoder
            .read(&mut buffer)
            .with_context(|| format!("decompress blob {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub(super) fn normalized_tar_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String> {
    let path = entry.path().context("decode backup tar path")?;
    path_to_portable(&path)
}

fn path_to_portable(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                parts.push(value.to_str().context("backup paths must be valid UTF-8")?)
            }
            _ => bail!(
                "backup path is not a safe relative path: {}",
                path.display()
            ),
        }
    }
    if parts.is_empty() {
        bail!("backup path cannot be empty");
    }
    Ok(parts.join("/"))
}

pub(super) fn portable_to_path(path: &str) -> Result<PathBuf> {
    validate_portable_path(path)?;
    Ok(path.split('/').collect())
}
