//! Pure derivation of what a backend must persist for an OBO ontology: the
//! canonical-IRI terms, their alias IRIs, and the transitive `is_a` closure.
//! Backends differ only in how they write the [`OntologyPlan`].

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use sbol_db_core::obo::parse_obo;
use sbol_db_storage::OntologyLoadReport;

/// One ontology term, keyed by its canonical OBO Foundry IRI.
pub struct OntologyTermRow {
    pub canonical_iri: String,
    pub curie: String,
    pub name: String,
    pub definition: Option<String>,
    pub is_obsolete: bool,
    pub synonyms: Vec<String>,
}

/// Everything one ontology load must persist, derived purely from the OBO text.
pub struct OntologyPlan {
    /// Uppercased prefix (`SO`, `SBO`), the ontology's key.
    pub prefix: String,
    pub name: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub terms: Vec<OntologyTermRow>,
    /// `(alias_iri, canonical_iri)` pairs (identifiers.org forms and alt_ids).
    pub aliases: Vec<(String, String)>,
    /// `(ancestor_iri, descendant_iri, depth)`; every term includes itself at
    /// depth 0.
    pub closure: Vec<(String, String, i16)>,
}

impl OntologyPlan {
    pub fn report(&self) -> OntologyLoadReport {
        OntologyLoadReport {
            prefix: self.prefix.clone(),
            source_url: self.source_url.clone(),
            version: self.version.clone(),
            term_count: self.terms.len(),
            closure_count: self.closure.len(),
            alias_count: self.aliases.len(),
        }
    }
}

/// Parse OBO `text` and derive its [`OntologyPlan`]. Only terms whose CURIE
/// carries `prefix` are kept; their canonical IRI is the OBO Foundry PURL
/// (`http://purl.obolibrary.org/obo/{PREFIX}_{NUMBER}`), with an
/// identifiers.org alias since SBOL documents commonly use that form.
pub fn build_ontology_plan(
    prefix: &str,
    name: &str,
    source_url: Option<&str>,
    text: &str,
) -> OntologyPlan {
    let parsed = parse_obo(text);
    let prefix_upper = prefix.to_ascii_uppercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    let version = parsed.data_version.clone();

    let mut terms: Vec<MaterialisedTerm> = Vec::with_capacity(parsed.terms.len());
    let mut curie_to_canonical: HashMap<String, String> = HashMap::new();
    for t in &parsed.terms {
        if !t.curie.starts_with(&format!("{prefix_upper}:")) {
            continue;
        }
        let canonical = curie_to_iri(&prefix_upper, &t.curie);
        curie_to_canonical.insert(t.curie.clone(), canonical.clone());
        terms.push(MaterialisedTerm {
            canonical_iri: canonical,
            curie: t.curie.clone(),
            name: t.name.clone(),
            definition: t.definition.clone(),
            is_obsolete: t.is_obsolete,
            parents: t.parents.clone(),
            alt_ids: t.alt_ids.clone(),
            synonyms: t.synonyms.clone(),
        });
    }
    for t in &terms {
        for alt in &t.alt_ids {
            curie_to_canonical
                .entry(alt.clone())
                .or_insert_with(|| t.canonical_iri.clone());
        }
    }

    // Closure: BFS up through each term's parents.
    let mut closure_pairs: HashSet<(String, String, i16)> = HashSet::new();
    let mut parent_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in &terms {
        let parents: Vec<&str> = t
            .parents
            .iter()
            .filter_map(|p| curie_to_canonical.get(p.as_str()).map(|s| s.as_str()))
            .collect();
        parent_map.insert(t.canonical_iri.as_str(), parents);
    }
    for t in &terms {
        closure_pairs.insert((t.canonical_iri.clone(), t.canonical_iri.clone(), 0));
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(t.canonical_iri.as_str());
        let mut frontier: VecDeque<(&str, i16)> = VecDeque::new();
        frontier.push_back((t.canonical_iri.as_str(), 0));
        while let Some((cur, depth)) = frontier.pop_front() {
            if depth > 1024 {
                break;
            }
            let Some(parents) = parent_map.get(cur) else {
                continue;
            };
            for p in parents {
                if visited.insert(p) {
                    closure_pairs.insert(((*p).to_owned(), t.canonical_iri.clone(), depth + 1));
                    frontier.push_back((p, depth + 1));
                }
            }
        }
    }

    // Aliases: identifiers.org form + alt_ids (both IRI and identifiers.org).
    let mut aliases: Vec<(String, String)> = Vec::new();
    let mut alias_seen: BTreeSet<String> = BTreeSet::new();
    for t in &terms {
        let identif = format!("http://identifiers.org/{prefix_lower}/{}", t.curie);
        if identif != t.canonical_iri && alias_seen.insert(identif.clone()) {
            aliases.push((identif, t.canonical_iri.clone()));
        }
        for alt in &t.alt_ids {
            if alt.starts_with(&format!("{prefix_upper}:")) {
                let alt_iri = curie_to_iri(&prefix_upper, alt);
                if alt_iri != t.canonical_iri && alias_seen.insert(alt_iri.clone()) {
                    aliases.push((alt_iri, t.canonical_iri.clone()));
                }
                let alt_identif = format!("http://identifiers.org/{prefix_lower}/{alt}");
                if alt_identif != t.canonical_iri && alias_seen.insert(alt_identif.clone()) {
                    aliases.push((alt_identif, t.canonical_iri.clone()));
                }
            }
        }
    }

    OntologyPlan {
        prefix: prefix_upper,
        name: name.to_owned(),
        source_url: source_url.map(|s| s.to_owned()),
        version,
        terms: terms
            .into_iter()
            .map(|t| OntologyTermRow {
                canonical_iri: t.canonical_iri,
                curie: t.curie,
                name: t.name,
                definition: t.definition,
                is_obsolete: t.is_obsolete,
                synonyms: t.synonyms,
            })
            .collect(),
        aliases,
        closure: closure_pairs.into_iter().collect(),
    }
}

struct MaterialisedTerm {
    canonical_iri: String,
    curie: String,
    name: String,
    definition: Option<String>,
    is_obsolete: bool,
    parents: Vec<String>,
    alt_ids: Vec<String>,
    synonyms: Vec<String>,
}

fn curie_to_iri(prefix_upper: &str, curie: &str) -> String {
    let suffix = curie
        .strip_prefix(&format!("{prefix_upper}:"))
        .unwrap_or(curie);
    format!("http://purl.obolibrary.org/obo/{prefix_upper}_{suffix}")
}
