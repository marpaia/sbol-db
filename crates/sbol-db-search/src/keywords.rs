//! The synthetic `keywords` field fed to the ranked text index.
//!
//! SBOLExplorer widens each part's searchable text beyond its literal metadata
//! by synthesizing a keyword string from three sources: the display id split on
//! underscores, the label and synonyms of its Sequence Ontology role, and the
//! biopax type suffix. [`build_keywords`] reproduces SBOLExplorer's
//! `add_keywords` / `add_roles` / `add_sbol_type` so the native index tokenizes
//! the same terms the tool it replaces did.

use serde::{Deserialize, Serialize};

/// A Sequence Ontology term the caller resolves from the ontology store and
/// passes in, so this crate stays free of any storage or ontology-file
/// dependency. `id` is the term's identifier (containing the `SO_xxxxxxx`
/// fragment), `label` its human-readable name, and `synonyms` its alternative
/// names.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoTerm {
    pub id: String,
    pub label: String,
    pub synonyms: Vec<String>,
}

/// The biopax level-3 namespace whose suffix SBOLExplorer folds into keywords.
/// The suffix begins at byte 48, immediately after the trailing `#`.
const BIOPAX_PREFIX: &str = "http://www.biopax.org/release/biopax-level3.owl#";

/// The `identifiers.org` header stripped from Sequence Ontology synonyms.
const INSDC_HEADER: &str = "INSDC_qualifier:";

/// Build the keyword string for one part.
///
/// The result is, in order: the display id with underscores replaced by spaces;
/// the label and synonyms of the Sequence Ontology term whose id contains the
/// role's trailing `SO:xxxxxxx` fragment (each synonym stripped of its
/// `INSDC_qualifier:` header); and the biopax type suffix. This mirrors
/// SBOLExplorer exactly, including its case-sensitive `Region` handling: the
/// strip only fires for a suffix containing lowercase `region`, so a
/// CamelCase `DnaRegion` is kept whole.
pub fn build_keywords(
    display_id: Option<&str>,
    role: Option<&str>,
    sbol_type: Option<&str>,
    so_terms: &[SoTerm],
) -> String {
    let mut keywords = match display_id {
        Some(id) => id.split('_').collect::<Vec<_>>().join(" "),
        None => String::new(),
    };

    if let Some(role) = role {
        if role.contains("identifiers.org") {
            let fragment = role[role.len().saturating_sub(10)..].replace(':', "_");
            let mut role_words: Vec<String> = Vec::new();
            for term in so_terms {
                if term.id.contains(&fragment) {
                    role_words.push(term.label.clone());
                    for synonym in &term.synonyms {
                        let synonym = if synonym.contains("INSDC") {
                            synonym.replace(INSDC_HEADER, "")
                        } else {
                            synonym.clone()
                        };
                        if !role_words.contains(&synonym) {
                            role_words.push(synonym);
                        }
                    }
                }
            }
            if !role_words.is_empty() {
                keywords.push(' ');
                keywords.push_str(&role_words.join(" "));
            }
        }
    }

    if let Some(sbol_type) = sbol_type {
        if sbol_type.contains(BIOPAX_PREFIX) {
            let mut suffix = sbol_type[BIOPAX_PREFIX.len()..].to_owned();
            if suffix.contains("region") {
                suffix = suffix.replace("Region", "");
            }
            keywords.push(' ');
            keywords.push_str(&suffix);
        }
    }

    keywords
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promoter_term() -> SoTerm {
        SoTerm {
            id: "http://purl.obolibrary.org/obo/SO_0000167".to_owned(),
            label: "promoter".to_owned(),
            synonyms: vec![
                "INSDC_qualifier:promoter".to_owned(),
                "promoter region".to_owned(),
            ],
        }
    }

    #[test]
    fn display_id_splits_on_underscore() {
        let kw = build_keywords(Some("BBa_K123"), None, None, &[]);
        assert!(kw.contains("BBa"));
        assert!(kw.contains("K123"));
        assert!(!kw.contains('_'));
    }

    #[test]
    fn role_resolves_label_and_synonyms() {
        let kw = build_keywords(
            Some("BBa_K123"),
            Some("http://identifiers.org/so/SO:0000167"),
            None,
            &[promoter_term()],
        );
        assert!(kw.contains("promoter"));
        // The INSDC header is stripped from the synonym.
        assert!(!kw.contains("INSDC_qualifier:"));
        assert!(kw.contains("promoter region"));
    }

    #[test]
    fn biopax_suffix_is_appended() {
        // Faithful to SBOLExplorer: the lowercase `region` guard does not fire
        // for CamelCase `DnaRegion`, so the suffix is appended whole.
        let kw = build_keywords(
            None,
            None,
            Some("http://www.biopax.org/release/biopax-level3.owl#DnaRegion"),
            &[],
        );
        assert_eq!(kw.trim(), "DnaRegion");
    }

    #[test]
    fn no_inputs_yields_empty() {
        assert_eq!(build_keywords(None, None, None, &[]), "");
    }
}
