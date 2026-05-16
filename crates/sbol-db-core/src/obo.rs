//! Minimal OBO 1.4 parser scoped to the subset sbol-db needs for ontology
//! expansion: term IRIs, names, parents, aliases.
//!
//! What we read from `[Term]` stanzas:
//! - `id:` — the canonical CURIE (e.g. `SO:0000167`)
//! - `name:`
//! - `def:` (the quoted body only)
//! - `is_a:` (parent CURIE; the trailing `! comment` is stripped)
//! - `is_obsolete:`
//! - `synonym:` (the quoted body only)
//! - `alt_id:` (treated as aliases of this term)
//!
//! We deliberately ignore `[Typedef]`, `[Instance]`, headers, and
//! cross-stanza references; this parser exists to load SO and SBO well
//! enough to compute the role/type closure, not to round-trip OBO.

use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OboTerm {
    pub curie: String,
    pub name: String,
    pub definition: Option<String>,
    pub is_obsolete: bool,
    pub parents: Vec<String>,
    pub alt_ids: Vec<String>,
    pub synonyms: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OboOntology {
    pub format_version: Option<String>,
    pub data_version: Option<String>,
    pub default_namespace: Option<String>,
    pub terms: Vec<OboTerm>,
}

/// Parse an OBO 1.4 document. Unrecognised lines are skipped, not errored.
pub fn parse_obo(input: &str) -> OboOntology {
    let mut ontology = OboOntology::default();
    let mut current: Option<OboTerm> = None;
    let mut in_term = false;
    let mut in_header = true;
    let mut seen_curies: HashSet<String> = HashSet::new();

    for raw in input.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[") {
            if let Some(end) = rest.find(']') {
                let stanza = &rest[..end];
                in_header = false;
                if let Some(term) = current.take() {
                    if !term.curie.is_empty() && seen_curies.insert(term.curie.clone()) {
                        ontology.terms.push(term);
                    }
                }
                if stanza == "Term" {
                    current = Some(OboTerm {
                        curie: String::new(),
                        name: String::new(),
                        definition: None,
                        is_obsolete: false,
                        parents: Vec::new(),
                        alt_ids: Vec::new(),
                        synonyms: Vec::new(),
                    });
                    in_term = true;
                } else {
                    in_term = false;
                }
                continue;
            }
        }

        let (tag, value) = match split_tag(line) {
            Some(t) => t,
            None => continue,
        };

        if in_header {
            match tag {
                "format-version" => ontology.format_version = Some(value.to_owned()),
                "data-version" => ontology.data_version = Some(value.to_owned()),
                "default-namespace" => ontology.default_namespace = Some(value.to_owned()),
                _ => {}
            }
            continue;
        }

        if !in_term {
            continue;
        }
        let Some(term) = current.as_mut() else {
            continue;
        };
        match tag {
            "id" => term.curie = strip_trailing_comment(value).to_owned(),
            "name" => term.name = strip_trailing_comment(value).to_owned(),
            "def" => {
                if let Some(body) = extract_quoted(value) {
                    term.definition = Some(body);
                }
            }
            "is_a" => {
                let parent = strip_trailing_comment(value).trim().to_owned();
                if !parent.is_empty() {
                    term.parents.push(parent);
                }
            }
            "is_obsolete" => {
                term.is_obsolete = value.trim().eq_ignore_ascii_case("true");
            }
            "alt_id" => {
                let alt = strip_trailing_comment(value).trim().to_owned();
                if !alt.is_empty() {
                    term.alt_ids.push(alt);
                }
            }
            "synonym" => {
                if let Some(body) = extract_quoted(value) {
                    term.synonyms.push(body);
                }
            }
            _ => {}
        }
    }
    if let Some(term) = current.take() {
        if !term.curie.is_empty() && seen_curies.insert(term.curie.clone()) {
            ontology.terms.push(term);
        }
    }
    ontology
}

fn split_tag(line: &str) -> Option<(&str, &str)> {
    let (tag, rest) = line.split_once(':')?;
    Some((tag.trim(), rest.trim_start()))
}

fn strip_trailing_comment(value: &str) -> &str {
    if let Some(idx) = value.find(" ! ") {
        value[..idx].trim_end()
    } else {
        value.trim_end()
    }
}

fn extract_quoted(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let first = bytes.iter().position(|&b| b == b'"')?;
    let mut buf = String::new();
    let mut i = first + 1;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if escaped {
            buf.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(buf);
        } else {
            buf.push(c);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"format-version: 1.4
data-version: 2024-01-15
default-namespace: sequence

[Term]
id: SO:0000001
name: region
def: "A sequence_feature with an extent greater than zero." [SO:ke]
synonym: "sequence_feature" RELATED []

[Term]
id: SO:0000167
name: promoter
def: "A regulatory_region composed of the TSS." [SO:regcreative]
is_a: SO:0001055 ! transcriptional_cis_regulatory_region
alt_id: SO:0000067

[Typedef]
id: part_of
name: part_of
is_transitive: true
"#;

    #[test]
    fn parses_terms_and_skips_typedef() {
        let onto = parse_obo(SAMPLE);
        assert_eq!(onto.terms.len(), 2);
        let promoter = onto.terms.iter().find(|t| t.curie == "SO:0000167").unwrap();
        assert_eq!(promoter.name, "promoter");
        assert_eq!(promoter.parents, vec!["SO:0001055"]);
        assert_eq!(promoter.alt_ids, vec!["SO:0000067"]);
        assert_eq!(
            promoter.definition.as_deref(),
            Some("A regulatory_region composed of the TSS.")
        );
    }

    #[test]
    fn header_fields_captured() {
        let onto = parse_obo(SAMPLE);
        assert_eq!(onto.format_version.as_deref(), Some("1.4"));
        assert_eq!(onto.data_version.as_deref(), Some("2024-01-15"));
        assert_eq!(onto.default_namespace.as_deref(), Some("sequence"));
    }

    #[test]
    fn handles_synonyms() {
        let onto = parse_obo(SAMPLE);
        let region = &onto.terms[0];
        assert_eq!(region.synonyms, vec!["sequence_feature"]);
    }
}
