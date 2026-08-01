use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use sbol_db_core::SerializationFormat;
use sbol_db_derive::build_import_plan;
use sbol_db_storage::{ImportInput, ImportOverwrite};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize)]
pub struct CorpusManifest {
    pub id: String,
    pub revision: String,
    pub source: CorpusSource,
    pub import_groups: Vec<ImportGroup>,
    pub expected_imported_documents: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CorpusSource {
    pub repository: String,
    pub commit: String,
    pub selection: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ImportGroup {
    pub path: String,
    pub expected_imported_documents: usize,
    pub expected_parse_failures: usize,
}

#[derive(Clone, Debug)]
pub struct CorpusDocument {
    pub ordinal: usize,
    pub relative_path: String,
    pub format: SerializationFormat,
    pub sha256: String,
    pub body: String,
    pub object_count: usize,
    pub triple_count: usize,
}

#[derive(Clone, Debug)]
pub struct Corpus {
    pub manifest: CorpusManifest,
    pub root: PathBuf,
    pub fingerprint: String,
    pub documents: Vec<CorpusDocument>,
    pub groups: Vec<GroupInventory>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GroupInventory {
    pub path: String,
    pub imported_documents: usize,
    pub parse_failures: Vec<ParseFailure>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ParseFailure {
    pub relative_path: String,
    pub error: String,
}

pub fn load_corpus(manifest_path: &Path, root: &Path) -> Result<Corpus> {
    let manifest: CorpusManifest = serde_json::from_slice(
        &fs::read(manifest_path)
            .with_context(|| format!("reading corpus manifest {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parsing corpus manifest {}", manifest_path.display()))?;
    verify_checkout(root, &manifest.source.commit)?;

    let mut documents = Vec::new();
    let mut groups = Vec::new();
    for group in &manifest.import_groups {
        let group_root = root.join(&group.path);
        if !group_root.is_dir() {
            bail!("corpus group is missing: {}", group_root.display());
        }

        let mut imported_documents = 0;
        let mut parse_failures = Vec::new();
        for path in collect_importable_files(&group_root)? {
            let relative_path = path
                .strip_prefix(root)
                .expect("group file must be below corpus root")
                .to_string_lossy()
                .replace('\\', "/");
            let format = SerializationFormat::from_extension(
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .expect("an importable file has an extension"),
            )
            .expect("collector only returns importable extensions");
            let body = fs::read_to_string(&path)
                .with_context(|| format!("reading corpus document {}", path.display()))?;
            let input = ImportInput {
                body: body.clone(),
                format,
                namespace: default_namespace(&path, format),
                source_uri: Some(relative_path.clone()),
                document_iri: None,
                created_by: None,
                name: None,
                description: None,
                overwrite: ImportOverwrite::Fail,
            };
            match build_import_plan(&input) {
                Ok(plan) => {
                    let sha256 = hex::encode(Sha256::digest(body.as_bytes()));
                    documents.push(CorpusDocument {
                        ordinal: 0,
                        relative_path,
                        format,
                        sha256,
                        body,
                        object_count: plan.summaries.len(),
                        triple_count: plan.triples.len(),
                    });
                    imported_documents += 1;
                }
                Err(error) => parse_failures.push(ParseFailure {
                    relative_path,
                    error: error.to_string(),
                }),
            }
        }

        if imported_documents != group.expected_imported_documents
            || parse_failures.len() != group.expected_parse_failures
        {
            bail!(
                "corpus group {} produced {} imports / {} parse failures; expected {} / {}",
                group.path,
                imported_documents,
                parse_failures.len(),
                group.expected_imported_documents,
                group.expected_parse_failures
            );
        }
        groups.push(GroupInventory {
            path: group.path.clone(),
            imported_documents,
            parse_failures,
        });
    }

    documents.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    for (ordinal, document) in documents.iter_mut().enumerate() {
        document.ordinal = ordinal;
    }
    if documents.len() != manifest.expected_imported_documents {
        bail!(
            "corpus produced {} importable documents; manifest expects {}",
            documents.len(),
            manifest.expected_imported_documents
        );
    }

    let mut fingerprint = Sha256::new();
    for document in &documents {
        fingerprint.update(document.relative_path.as_bytes());
        fingerprint.update([0]);
        fingerprint.update(document.sha256.as_bytes());
        fingerprint.update(*b"\n");
    }

    Ok(Corpus {
        manifest,
        root: root.to_path_buf(),
        fingerprint: hex::encode(fingerprint.finalize()),
        documents,
        groups,
    })
}

fn verify_checkout(root: &Path, expected_commit: &str) -> Result<()> {
    if !root.join(".git").exists() {
        bail!(
            "SBOLTestSuite root is not a Git checkout: {}",
            root.display()
        );
    }
    let head = git_output(root, &["rev-parse", "HEAD"])?;
    if head != expected_commit {
        bail!(
            "SBOLTestSuite must be pinned at {expected_commit}, found {head} in {}",
            root.display()
        );
    }
    git_clean(root, &["diff", "--quiet"])?;
    git_clean(root, &["diff", "--cached", "--quiet"])?;
    Ok(())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .with_context(|| format!("running git in {}", root.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git {} failed in {}: {}",
            arguments.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8(output.stdout)
        .context("git output was not UTF-8")?
        .trim()
        .to_owned())
}

fn git_clean(root: &Path, arguments: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .status()
        .with_context(|| format!("running git in {}", root.display()))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(1) => bail!(
            "SBOLTestSuite has tracked modifications; refusing a non-reproducible run: {}",
            root.display()
        ),
        _ => bail!(
            "git {} failed in {} with {status}",
            arguments.join(" "),
            root.display()
        ),
    }
}

fn collect_importable_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("reading corpus directory {}", directory.display()))?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(SerializationFormat::from_extension)
                    .is_some()
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn default_namespace(path: &Path, format: SerializationFormat) -> Option<String> {
    match format {
        SerializationFormat::Turtle
        | SerializationFormat::JsonLd
        | SerializationFormat::RdfXml
        | SerializationFormat::NTriples
        | SerializationFormat::GenBank
        | SerializationFormat::Fasta => {}
        SerializationFormat::Json | SerializationFormat::TriG | SerializationFormat::NQuads => {
            return None;
        }
    }
    let stem = path.file_stem()?.to_str()?;
    let mut segment = String::with_capacity(stem.len());
    let mut previous_separator = false;
    for character in stem.chars() {
        let mapped = if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            Some(character)
        } else if character.is_ascii_whitespace() || matches!(character, '.' | '/' | '\\' | ':') {
            Some('_')
        } else {
            None
        };
        if let Some(character) = mapped {
            if character == '_' {
                if previous_separator {
                    continue;
                }
                previous_separator = true;
            } else {
                previous_separator = false;
            }
            segment.push(character);
        }
    }
    (!segment.is_empty()).then(|| format!("https://sbol-db.local/imports/{segment}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_sanitization_matches_cli_contract() {
        assert_eq!(
            default_namespace(Path::new("some document.xml"), SerializationFormat::RdfXml),
            Some("https://sbol-db.local/imports/some_document".to_owned())
        );
        assert_eq!(
            default_namespace(Path::new("graph.json"), SerializationFormat::Json),
            None
        );
    }
}
