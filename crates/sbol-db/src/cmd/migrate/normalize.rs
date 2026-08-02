//! Policy-gated repair of invalid IRI references in a Virtuoso N-Quads export.
//!
//! The raw export remains immutable. This module only rewrites IRI-reference
//! tokens (never literal text), identifies approved source and target IRIs by
//! SHA-256 rather than recording sensitive/user-controlled IRIs in policy or
//! reports, and refuses any count, graph, collision, or strict-parse mismatch.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use oxiri::{Iri, IriRef};
use oxrdfio::{RdfFormat, RdfParser};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const POLICY_SCHEMA: &str = "sbol-db.synbiohub-rdf-normalization-policy.v1";
pub(crate) const REPORT_SCHEMA: &str = "sbol-db.synbiohub-rdf-normalization.v1";

#[derive(Debug)]
pub struct NormalizeInputs {
    pub input: PathBuf,
    pub output: PathBuf,
    pub policy: PathBuf,
    pub report: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationPolicy {
    pub schema: String,
    pub rules: Vec<NormalizationRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationRule {
    pub source_iri_sha256: String,
    pub target_iri_sha256: String,
    pub operation: NormalizationOperation,
    pub expected_occurrences: IriRoleCounts,
    pub expected_distinct_graphs: u64,
    pub expected_replacements: u64,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizationOperation {
    PercentEncodeSpaces,
    MapRelativeIriToUrn,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IriRoleCounts {
    pub subject: u64,
    pub predicate: u64,
    pub object: u64,
    pub datatype: u64,
    pub graph: u64,
}

impl IriRoleCounts {
    fn increment(&mut self, role: IriRole) {
        match role {
            IriRole::Subject => self.subject += 1,
            IriRole::Predicate => self.predicate += 1,
            IriRole::Object => self.object += 1,
            IriRole::Datatype => self.datatype += 1,
            IriRole::Graph => self.graph += 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedArtifact {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationRuleReport {
    pub source_iri_sha256: String,
    pub target_iri_sha256: String,
    pub occurrences: IriRoleCounts,
    pub distinct_graphs: u64,
    pub replacements: u64,
    pub preexisting_target_occurrences: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationReport {
    pub schema: String,
    pub generated_at: String,
    pub raw: NormalizedArtifact,
    pub policy: NormalizedArtifact,
    pub normalized: NormalizedArtifact,
    pub input_quads: u64,
    pub output_quads: u64,
    pub rules: Vec<NormalizationRuleReport>,
}

#[derive(Clone, Copy, Debug)]
enum IriRole {
    Subject,
    Predicate,
    Object,
    Datatype,
    Graph,
}

impl IriRole {
    fn name(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Predicate => "predicate",
            Self::Object => "object",
            Self::Datatype => "datatype",
            Self::Graph => "graph",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct IriSpan {
    inner_start: usize,
    inner_end: usize,
    role: IriRole,
}

#[derive(Debug)]
struct ScannedQuad {
    iris: Vec<IriSpan>,
    graph_identity_sha256: String,
}

#[derive(Default)]
struct RuleState {
    occurrences: IriRoleCounts,
    graphs: BTreeSet<String>,
    replacements: u64,
    target_collisions: u64,
}

#[derive(Default)]
struct UnapprovedState {
    occurrences: IriRoleCounts,
    graphs: BTreeSet<String>,
    first_line: u64,
    space_codepoints: u64,
    kind: String,
}

#[derive(Serialize)]
struct UnapprovedInvalidReport {
    iri_sha256: String,
    kind: String,
    occurrences: IriRoleCounts,
    distinct_graphs: u64,
    first_line: u64,
    space_codepoints: u64,
}

struct Replacement {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

/// Normalize one immutable N-Quads export and write a provenance report.
pub fn run(inputs: NormalizeInputs) -> Result<()> {
    if !inputs.input.is_file() {
        bail!("raw RDF input is not a file: {}", inputs.input.display());
    }
    if !inputs.policy.is_file() {
        bail!(
            "normalization policy is not a file: {}",
            inputs.policy.display()
        );
    }
    if inputs.output.exists() {
        bail!(
            "refusing to overwrite normalized RDF output: {}",
            inputs.output.display()
        );
    }
    if inputs.report.exists() {
        bail!(
            "refusing to overwrite normalization report: {}",
            inputs.report.display()
        );
    }
    if inputs.input == inputs.output {
        bail!("raw and normalized RDF paths must be different");
    }

    let policy_bytes = std::fs::read(&inputs.policy)
        .with_context(|| format!("reading normalization policy {}", inputs.policy.display()))?;
    let policy: NormalizationPolicy = serde_json::from_slice(&policy_bytes)
        .with_context(|| format!("parsing normalization policy {}", inputs.policy.display()))?;
    validate_policy(&policy)?;

    let source_rules = policy
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.source_iri_sha256.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let target_rules = policy
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.target_iri_sha256.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut states = (0..policy.rules.len())
        .map(|_| RuleState::default())
        .collect::<Vec<_>>();
    let mut unapproved = BTreeMap::<String, UnapprovedState>::new();

    let output_parent = inputs.output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(output_parent).with_context(|| {
        format!(
            "creating normalized RDF directory {}",
            output_parent.display()
        )
    })?;
    let mut output_temp = tempfile::NamedTempFile::new_in(output_parent)
        .with_context(|| format!("creating temporary output in {}", output_parent.display()))?;

    let raw_file = File::open(&inputs.input)
        .with_context(|| format!("opening raw RDF input {}", inputs.input.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, raw_file);
    let mut writer = BufWriter::with_capacity(1024 * 1024, output_temp.as_file_mut());
    let mut raw_hasher = Sha256::new();
    let mut normalized_hasher = Sha256::new();
    let mut raw_bytes = 0_u64;
    let mut normalized_bytes = 0_u64;
    let mut input_quads = 0_u64;
    let mut line_number = 0_u64;
    let mut line = Vec::new();

    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("reading raw RDF line {}", line_number + 1))?;
        if read == 0 {
            break;
        }
        line_number += 1;
        raw_hasher.update(&line);
        raw_bytes += read as u64;

        let Some(scanned) = scan_quad_line(&line, line_number)? else {
            writer.write_all(&line)?;
            normalized_hasher.update(&line);
            normalized_bytes += line.len() as u64;
            continue;
        };
        input_quads += 1;
        let mut replacements = Vec::new();

        for span in scanned.iris {
            let encoded = &line[span.inner_start..span.inner_end];
            let decoded = decode_iri(encoded).with_context(|| {
                format!(
                    "decoding {} IRI token on raw RDF line {line_number}",
                    span.role.name()
                )
            })?;
            let digest = sha256_text(&decoded);

            if let Some(index) = target_rules.get(digest.as_str()) {
                states[*index].target_collisions += 1;
            }

            let iri_is_valid = Iri::parse(decoded.as_str()).is_ok();
            if let Some(index) = source_rules.get(digest.as_str()).copied() {
                if iri_is_valid {
                    bail!(
                        "normalization policy source {} matched an already-valid IRI on line {line_number}",
                        digest
                    );
                }
                let rule = &policy.rules[index];
                let (replacement, count) = match rule.operation {
                    NormalizationOperation::PercentEncodeSpaces => percent_encode_spaces(encoded),
                    NormalizationOperation::MapRelativeIriToUrn => {
                        (map_relative_iri_to_urn(encoded)?, 1)
                    }
                };
                if count == 0 {
                    bail!(
                        "normalization policy source {} matched line {line_number} but its operation made no replacement",
                        digest
                    );
                }
                let target = decode_iri(&replacement).with_context(|| {
                    format!("decoding normalized IRI token on raw RDF line {line_number}")
                })?;
                if Iri::parse(target.as_str()).is_err() {
                    bail!(
                        "normalization rule for source {} produced an invalid target on line {line_number}",
                        digest
                    );
                }
                let target_digest = sha256_text(&target);
                if target_digest != rule.target_iri_sha256 {
                    bail!(
                        "normalization rule for source {} produced target digest {}, expected {}",
                        digest,
                        target_digest,
                        rule.target_iri_sha256
                    );
                }

                let state = &mut states[index];
                state.occurrences.increment(span.role);
                state.graphs.insert(scanned.graph_identity_sha256.clone());
                state.replacements += count;
                replacements.push(Replacement {
                    start: span.inner_start,
                    end: span.inner_end,
                    bytes: replacement,
                });
            } else if !iri_is_valid {
                let state = unapproved.entry(digest).or_insert_with(|| UnapprovedState {
                    first_line: line_number,
                    kind: if IriRef::parse(decoded.as_str()).is_ok() {
                        "relative_iri".to_owned()
                    } else if decoded.contains(' ') {
                        "forbidden_space".to_owned()
                    } else {
                        "other_invalid_iri".to_owned()
                    },
                    ..UnapprovedState::default()
                });
                state.occurrences.increment(span.role);
                state.graphs.insert(scanned.graph_identity_sha256.clone());
                state.space_codepoints += decoded
                    .chars()
                    .filter(|character| *character == ' ')
                    .count() as u64;
            }
        }

        let normalized_line = apply_replacements(&line, &replacements);
        writer.write_all(&normalized_line)?;
        normalized_hasher.update(&normalized_line);
        normalized_bytes += normalized_line.len() as u64;
    }
    writer.flush()?;
    drop(writer);
    output_temp.as_file().sync_all()?;

    let rule_validation = verify_rule_counts(&policy, states);
    if !unapproved.is_empty() {
        let inventory = unapproved
            .into_iter()
            .map(|(iri_sha256, state)| UnapprovedInvalidReport {
                iri_sha256,
                kind: state.kind,
                occurrences: state.occurrences,
                distinct_graphs: state.graphs.len() as u64,
                first_line: state.first_line,
                space_codepoints: state.space_codepoints,
            })
            .collect::<Vec<_>>();
        bail!(
            "raw RDF contains unapproved invalid IRIs; no output was published:\n{}\napproved-rule validation: {}",
            serde_json::to_string_pretty(&inventory)?,
            match &rule_validation {
                Ok(_) => "ok".to_owned(),
                Err(error) => error.to_string(),
            }
        );
    }
    let rule_reports = rule_validation?;
    let output_quads = strict_nquads_count(output_temp.path())?;
    if output_quads != input_quads {
        bail!(
            "normalized N-Quads strict parse produced {output_quads} quads, but the token scanner saw {input_quads}"
        );
    }

    let report = NormalizationReport {
        schema: REPORT_SCHEMA.to_owned(),
        generated_at: Utc::now().to_rfc3339(),
        raw: NormalizedArtifact {
            path: inputs.input.clone(),
            bytes: raw_bytes,
            sha256: hex::encode(raw_hasher.finalize()),
        },
        policy: NormalizedArtifact {
            path: inputs.policy.clone(),
            bytes: policy_bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&policy_bytes)),
        },
        normalized: NormalizedArtifact {
            path: inputs.output.clone(),
            bytes: normalized_bytes,
            sha256: hex::encode(normalized_hasher.finalize()),
        },
        input_quads,
        output_quads,
        rules: rule_reports,
    };

    output_temp.persist(&inputs.output).map_err(|error| {
        anyhow!(
            "persisting normalized RDF {}: {}",
            inputs.output.display(),
            error.error
        )
    })?;
    write_json_atomic_new(&inputs.report, &report)?;
    crate::output::print_json(&report)?;
    Ok(())
}

/// Re-hash the complete normalization chain and strictly parse its output.
pub(crate) fn verify_report(path: &Path) -> Result<NormalizationReport> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading RDF normalization report {}", path.display()))?;
    let report: NormalizationReport = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing RDF normalization report {}", path.display()))?;
    if report.schema != REPORT_SCHEMA {
        bail!(
            "unsupported RDF normalization report schema `{}` (expected `{REPORT_SCHEMA}`)",
            report.schema
        );
    }
    verify_artifact(&report.raw, "raw RDF")?;
    verify_artifact(&report.policy, "normalization policy")?;
    verify_artifact(&report.normalized, "normalized RDF")?;

    let policy_bytes = std::fs::read(&report.policy.path)?;
    let policy: NormalizationPolicy = serde_json::from_slice(&policy_bytes)?;
    validate_policy(&policy)?;
    if report.rules.len() != policy.rules.len() {
        bail!("normalization report rule count does not match its verified policy");
    }
    for (expected, actual) in policy.rules.iter().zip(&report.rules) {
        if actual.source_iri_sha256 != expected.source_iri_sha256
            || actual.target_iri_sha256 != expected.target_iri_sha256
            || actual.occurrences != expected.expected_occurrences
            || actual.distinct_graphs != expected.expected_distinct_graphs
            || actual.replacements != expected.expected_replacements
            || actual.preexisting_target_occurrences != 0
        {
            bail!(
                "normalization report does not satisfy verified policy rule {}",
                expected.source_iri_sha256
            );
        }
    }
    if report.input_quads != report.output_quads {
        bail!("normalization report input/output quad counts differ");
    }
    let strict = strict_nquads_count(&report.normalized.path)?;
    if strict != report.output_quads {
        bail!(
            "normalized RDF now parses as {strict} quads, expected {}",
            report.output_quads
        );
    }
    Ok(report)
}

fn validate_policy(policy: &NormalizationPolicy) -> Result<()> {
    if policy.schema != POLICY_SCHEMA {
        bail!(
            "unsupported RDF normalization policy schema `{}` (expected `{POLICY_SCHEMA}`)",
            policy.schema
        );
    }
    if policy.rules.is_empty() {
        bail!("RDF normalization policy must contain at least one rule");
    }
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for rule in &policy.rules {
        validate_digest(&rule.source_iri_sha256, "source_iri_sha256")?;
        validate_digest(&rule.target_iri_sha256, "target_iri_sha256")?;
        if rule.source_iri_sha256 == rule.target_iri_sha256 {
            bail!("normalization source and target digests must differ");
        }
        if !sources.insert(rule.source_iri_sha256.as_str()) {
            bail!("duplicate normalization source digest");
        }
        if !targets.insert(rule.target_iri_sha256.as_str()) {
            bail!("duplicate normalization target digest");
        }
        if rule.reason.trim().is_empty() {
            bail!("normalization rules require a non-empty reason");
        }
        if rule.expected_distinct_graphs == 0 || rule.expected_replacements == 0 {
            bail!("normalization rules require non-zero graph and replacement expectations");
        }
    }
    if sources.iter().any(|source| targets.contains(source)) {
        bail!("a normalization source digest may not also be a target digest");
    }
    Ok(())
}

fn validate_digest(value: &str, field: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{field} must be a lowercase SHA-256 digest");
    }
    Ok(())
}

fn verify_rule_counts(
    policy: &NormalizationPolicy,
    states: Vec<RuleState>,
) -> Result<Vec<NormalizationRuleReport>> {
    policy
        .rules
        .iter()
        .zip(states)
        .map(|(rule, state)| {
            if state.target_collisions != 0 {
                bail!(
                    "normalization target {} already occurred {} time(s) in the raw export",
                    rule.target_iri_sha256,
                    state.target_collisions
                );
            }
            if state.occurrences != rule.expected_occurrences {
                bail!(
                    "normalization source {} occurrence counts differ: expected {:?}, got {:?}",
                    rule.source_iri_sha256,
                    rule.expected_occurrences,
                    state.occurrences
                );
            }
            if state.graphs.len() as u64 != rule.expected_distinct_graphs {
                bail!(
                    "normalization source {} graph count differs: expected {}, got {}",
                    rule.source_iri_sha256,
                    rule.expected_distinct_graphs,
                    state.graphs.len()
                );
            }
            if state.replacements != rule.expected_replacements {
                bail!(
                    "normalization source {} replacement count differs: expected {}, got {}",
                    rule.source_iri_sha256,
                    rule.expected_replacements,
                    state.replacements
                );
            }
            Ok(NormalizationRuleReport {
                source_iri_sha256: rule.source_iri_sha256.clone(),
                target_iri_sha256: rule.target_iri_sha256.clone(),
                occurrences: state.occurrences,
                distinct_graphs: state.graphs.len() as u64,
                replacements: state.replacements,
                preexisting_target_occurrences: state.target_collisions,
            })
        })
        .collect()
}

fn scan_quad_line(line: &[u8], line_number: u64) -> Result<Option<ScannedQuad>> {
    let mut cursor = Cursor::new(line, line_number);
    cursor.skip_ws();
    if cursor.done() || cursor.peek() == Some(b'#') {
        return Ok(None);
    }

    let mut iris = Vec::with_capacity(5);
    cursor.subject(&mut iris)?;
    cursor.require_ws("after subject")?;
    cursor.iri(IriRole::Predicate, &mut iris)?;
    cursor.require_ws("after predicate")?;
    cursor.object(&mut iris)?;
    cursor.require_ws("after object")?;

    let graph_identity_sha256 = if cursor.peek() == Some(b'.') {
        sha256_text("[default graph]")
    } else if cursor.peek() == Some(b'<') {
        let span = cursor.iri(IriRole::Graph, &mut iris)?;
        let decoded = decode_iri(&line[span.inner_start..span.inner_end])
            .with_context(|| format!("decoding graph IRI on raw RDF line {line_number}"))?;
        sha256_text(&decoded)
    } else if cursor.starts_with(b"_:") {
        let token = cursor.blank_node()?;
        hex::encode(Sha256::digest(token))
    } else {
        return cursor.error("expected graph label or terminating dot");
    };

    if cursor.peek() != Some(b'.') {
        cursor.require_ws("before terminating dot")?;
    }
    cursor.expect(b'.', "terminating dot")?;
    cursor.skip_ws();
    if !cursor.done() && cursor.peek() != Some(b'#') {
        return cursor.error("unexpected content after terminating dot");
    }
    Ok(Some(ScannedQuad {
        iris,
        graph_identity_sha256,
    }))
}

struct Cursor<'a> {
    line: &'a [u8],
    pos: usize,
    line_number: u64,
}

impl<'a> Cursor<'a> {
    fn new(line: &'a [u8], line_number: u64) -> Self {
        Self {
            line,
            pos: 0,
            line_number,
        }
    }

    fn done(&self) -> bool {
        self.pos >= self.line.len()
    }

    fn peek(&self) -> Option<u8> {
        self.line.get(self.pos).copied()
    }

    fn starts_with(&self, value: &[u8]) -> bool {
        self.line[self.pos..].starts_with(value)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn require_ws(&mut self, context: &str) -> Result<()> {
        if !self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            return self.error(&format!("expected whitespace {context}"));
        }
        self.skip_ws();
        Ok(())
    }

    fn expect(&mut self, byte: u8, label: &str) -> Result<()> {
        if self.peek() != Some(byte) {
            return self.error(&format!("expected {label}"));
        }
        self.pos += 1;
        Ok(())
    }

    fn subject(&mut self, iris: &mut Vec<IriSpan>) -> Result<()> {
        if self.peek() == Some(b'<') {
            self.iri(IriRole::Subject, iris)?;
            Ok(())
        } else if self.starts_with(b"_:") {
            self.blank_node()?;
            Ok(())
        } else {
            self.error("expected IRI or blank-node subject")
        }
    }

    fn object(&mut self, iris: &mut Vec<IriSpan>) -> Result<()> {
        if self.peek() == Some(b'<') {
            self.iri(IriRole::Object, iris)?;
        } else if self.starts_with(b"_:") {
            self.blank_node()?;
        } else if self.peek() == Some(b'"') {
            self.literal(iris)?;
        } else {
            return self.error("expected IRI, blank-node, or literal object");
        }
        Ok(())
    }

    fn iri(&mut self, role: IriRole, iris: &mut Vec<IriSpan>) -> Result<IriSpan> {
        self.expect(b'<', "IRI start")?;
        let inner_start = self.pos;
        while let Some(byte) = self.peek() {
            if byte == b'>' {
                let span = IriSpan {
                    inner_start,
                    inner_end: self.pos,
                    role,
                };
                self.pos += 1;
                iris.push(span);
                return Ok(span);
            }
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            self.pos += 1;
        }
        self.error("unterminated IRI token")
    }

    fn blank_node(&mut self) -> Result<&'a [u8]> {
        if !self.starts_with(b"_:") {
            return self.error("expected blank node");
        }
        let start = self.pos;
        self.pos += 2;
        while self.peek().is_some_and(|byte| !byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
        if self.pos == start + 2 {
            return self.error("empty blank-node label");
        }
        Ok(&self.line[start..self.pos])
    }

    fn literal(&mut self, iris: &mut Vec<IriSpan>) -> Result<()> {
        self.expect(b'"', "literal start")?;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    if self.done() {
                        return self.error("unterminated literal escape");
                    }
                    self.pos += 1;
                }
                Some(b'\n' | b'\r') | None => return self.error("unterminated literal"),
                Some(_) => self.pos += 1,
            }
        }
        if self.peek() == Some(b'@') {
            self.pos += 1;
            while self.peek().is_some_and(|byte| !byte.is_ascii_whitespace()) {
                self.pos += 1;
            }
        } else if self.starts_with(b"^^") {
            self.pos += 2;
            self.iri(IriRole::Datatype, iris)?;
        }
        Ok(())
    }

    fn error<T>(&self, message: &str) -> Result<T> {
        bail!(
            "malformed N-Quads on raw RDF line {}: {message}",
            self.line_number
        )
    }
}

fn decode_iri(encoded: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(encoded).context("IRI token is not UTF-8")?;
    let mut decoded = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() && bytes[index + 1] == b'u' {
            decoded.push(decode_uchar(&bytes[index + 2..], 4)?);
            index += 6;
        } else if bytes[index] == b'\\' && index + 1 < bytes.len() && bytes[index + 1] == b'U' {
            decoded.push(decode_uchar(&bytes[index + 2..], 8)?);
            index += 10;
        } else {
            let rest = std::str::from_utf8(&bytes[index..])?;
            let character = rest.chars().next().expect("non-empty UTF-8 suffix");
            decoded.push(character);
            index += character.len_utf8();
        }
    }
    Ok(decoded)
}

fn decode_uchar(hex_bytes: &[u8], digits: usize) -> Result<char> {
    if hex_bytes.len() < digits {
        bail!("truncated Unicode escape in IRI token");
    }
    let text = std::str::from_utf8(&hex_bytes[..digits])?;
    let value = u32::from_str_radix(text, 16).context("invalid Unicode escape in IRI token")?;
    char::from_u32(value).context("invalid Unicode scalar value in IRI token")
}

fn percent_encode_spaces(encoded: &[u8]) -> (Vec<u8>, u64) {
    let mut output = Vec::with_capacity(encoded.len());
    let mut index = 0;
    let mut replacements = 0_u64;
    while index < encoded.len() {
        if encoded[index] == b' ' {
            output.extend_from_slice(b"%20");
            replacements += 1;
            index += 1;
        } else if encoded[index..].starts_with(b"\\u0020") {
            output.extend_from_slice(b"%20");
            replacements += 1;
            index += 6;
        } else if encoded[index..].starts_with(b"\\U00000020") {
            output.extend_from_slice(b"%20");
            replacements += 1;
            index += 10;
        } else {
            output.push(encoded[index]);
            index += 1;
        }
    }
    (output, replacements)
}

fn map_relative_iri_to_urn(encoded: &[u8]) -> Result<Vec<u8>> {
    let decoded = decode_iri(encoded)?;
    if IriRef::parse(decoded.as_str()).is_err() || Iri::parse(decoded.as_str()).is_ok() {
        bail!("map_relative_iri_to_urn requires a valid relative IRI reference");
    }
    let mut output = b"urn:synbiohub:legacy-relative-iri:".to_vec();
    for byte in decoded.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
            output.push(*byte);
        } else {
            write!(output, "%{byte:02X}")?;
        }
    }
    Ok(output)
}

fn apply_replacements(line: &[u8], replacements: &[Replacement]) -> Vec<u8> {
    if replacements.is_empty() {
        return line.to_vec();
    }
    let mut output = Vec::with_capacity(line.len());
    let mut cursor = 0;
    for replacement in replacements {
        output.extend_from_slice(&line[cursor..replacement.start]);
        output.extend_from_slice(&replacement.bytes);
        cursor = replacement.end;
    }
    output.extend_from_slice(&line[cursor..]);
    output
}

fn strict_nquads_count(path: &Path) -> Result<u64> {
    let file = File::open(path).with_context(|| format!("opening N-Quads {}", path.display()))?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut count = 0_u64;
    for quad in RdfParser::from_format(RdfFormat::NQuads).for_reader(reader) {
        quad.with_context(|| format!("strictly parsing normalized N-Quads {}", path.display()))?;
        count += 1;
    }
    Ok(count)
}

fn verify_artifact(expected: &NormalizedArtifact, label: &str) -> Result<()> {
    let actual = digest_file(&expected.path)?;
    if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
        bail!(
            "{label} changed after normalization: expected {} bytes / {}, got {} bytes / {}",
            expected.bytes,
            expected.sha256,
            actual.bytes,
            actual.sha256
        );
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<NormalizedArtifact> {
    let file = File::open(path).with_context(|| format!("opening artifact {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok(NormalizedArtifact {
        path: path.to_path_buf(),
        bytes,
        sha256: hex::encode(hasher.finalize()),
    })
}

fn write_json_atomic_new(path: &Path, value: &impl Serialize) -> Result<()> {
    if path.exists() {
        bail!("refusing to overwrite {}", path.display());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| anyhow!("persisting report {}: {}", path.display(), error.error))?;
    Ok(())
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(
        source: &str,
        target: &str,
        occurrences: IriRoleCounts,
        replacements: u64,
    ) -> NormalizationPolicy {
        NormalizationPolicy {
            schema: POLICY_SCHEMA.to_owned(),
            rules: vec![NormalizationRule {
                source_iri_sha256: sha256_text(source),
                target_iri_sha256: sha256_text(target),
                operation: NormalizationOperation::PercentEncodeSpaces,
                expected_occurrences: occurrences,
                expected_distinct_graphs: 1,
                expected_replacements: replacements,
                reason: "fixture invalid IRI".to_owned(),
            }],
        }
    }

    fn run_fixture(
        raw: &str,
        policy: &NormalizationPolicy,
    ) -> Result<(tempfile::TempDir, NormalizationReport)> {
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("raw.nq");
        let output = temp.path().join("normalized.nq");
        let policy_path = temp.path().join("policy.json");
        let report_path = temp.path().join("report.json");
        std::fs::write(&input, raw)?;
        std::fs::write(&policy_path, serde_json::to_vec_pretty(policy)?)?;
        run(NormalizeInputs {
            input,
            output,
            policy: policy_path,
            report: report_path.clone(),
        })?;
        let report = verify_report(&report_path)?;
        Ok((temp, report))
    }

    #[test]
    fn rewrites_only_approved_iri_tokens_and_preserves_literal_text() -> Result<()> {
        let source = "https://example.test/a b";
        let target = "https://example.test/a%20b";
        let raw = concat!(
            "<https://example.test/a\\u0020b> <https://example.test/p> <https://example.test/a\\u0020b> <https://example.test/g> .\n",
            "<https://example.test/s> <https://example.test/p> \"https://example.test/a\\\\u0020b\" <https://example.test/g> .\n",
        );
        let (temp, report) = run_fixture(
            raw,
            &policy(
                source,
                target,
                IriRoleCounts {
                    subject: 1,
                    object: 1,
                    ..IriRoleCounts::default()
                },
                2,
            ),
        )?;
        let output = std::fs::read_to_string(&report.normalized.path)?;
        assert!(output.contains("<https://example.test/a%20b>"));
        assert!(output.contains("\"https://example.test/a\\\\u0020b\""));
        assert_eq!(report.input_quads, 2);
        assert!(temp.path().exists());
        Ok(())
    }

    #[test]
    fn rejects_unapproved_invalid_iri() -> Result<()> {
        let policy = policy(
            "https://example.test/approved bad",
            "https://example.test/approved%20bad",
            IriRoleCounts {
                subject: 1,
                ..IriRoleCounts::default()
            },
            1,
        );
        let error = run_fixture(
            "<https://example.test/different\\u0020bad> <https://example.test/p> <https://example.test/o> <https://example.test/g> .\n",
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unapproved invalid IRIs"));
        assert!(error
            .to_string()
            .contains(&sha256_text("https://example.test/different bad")));
        Ok(())
    }

    #[test]
    fn rejects_expected_count_mismatch() -> Result<()> {
        let source = "https://example.test/a b";
        let target = "https://example.test/a%20b";
        let error = run_fixture(
            "<https://example.test/a\\u0020b> <https://example.test/p> <https://example.test/o> <https://example.test/g> .\n",
            &policy(
                source,
                target,
                IriRoleCounts {
                    subject: 2,
                    ..IriRoleCounts::default()
                },
                1,
            ),
        )
        .unwrap_err();
        assert!(error.to_string().contains("occurrence counts differ"));
        Ok(())
    }

    #[test]
    fn rejects_preexisting_target_collision() -> Result<()> {
        let source = "https://example.test/a b";
        let target = "https://example.test/a%20b";
        let raw = concat!(
            "<https://example.test/a\\u0020b> <https://example.test/p> <https://example.test/o> <https://example.test/g> .\n",
            "<https://example.test/a%20b> <https://example.test/p> <https://example.test/o> <https://example.test/g> .\n",
        );
        let error = run_fixture(
            raw,
            &policy(
                source,
                target,
                IriRoleCounts {
                    subject: 1,
                    ..IriRoleCounts::default()
                },
                1,
            ),
        )
        .unwrap_err();
        assert!(error.to_string().contains("already occurred"));
        Ok(())
    }

    #[test]
    fn maps_relative_iri_to_injective_legacy_urn() -> Result<()> {
        let source = "legacy-role";
        let target = "urn:synbiohub:legacy-relative-iri:legacy-role";
        let mut policy = policy(
            source,
            target,
            IriRoleCounts {
                object: 1,
                ..IriRoleCounts::default()
            },
            1,
        );
        policy.rules[0].operation = NormalizationOperation::MapRelativeIriToUrn;
        let (_temp, report) = run_fixture(
            "<https://example.test/s> <https://example.test/p> <legacy-role> <https://example.test/g> .\n",
            &policy,
        )?;
        let output = std::fs::read_to_string(&report.normalized.path)?;
        assert!(output.contains("<urn:synbiohub:legacy-relative-iri:legacy-role>"));
        Ok(())
    }
}
