//! The SynBioHub query accelerator contract.
//!
//! SynBioHub issues a fixed set of SPARQL templates whose results are, in
//! effect, "enumerate a set of top-level objects and return a metadata bundle
//! per object, with counts and facets." A backend that supports the accelerator
//! answers these from purpose-built indexes (range scans + point lookups) rather
//! than evaluating the graph pattern, with the expensive parts (counts, the
//! membership anti-join) precomputed at write time.
//!
//! The recognizer in `sbol-db-sparql` turns a parsed query into an
//! [`AcceleratedQuery`]; the backend returns [`AccelSolutions`]. Recognition is
//! best-effort: anything not recognized, or any backend without accelerator
//! support, falls back to the generic SPARQL engine, so results never depend on
//! the accelerator being used.

use std::collections::{HashMap, HashSet};

use sbol_db_core::{ObjectTerm, SubjectTerm, Triple};
use serde::{Deserialize, Serialize};

use crate::TermValue;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const TOPLEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
const DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const VERSION: &str = "http://sbols.org/v2#version";
const SBOL_TYPE: &str = "http://sbols.org/v2#type";
const ROLE: &str = "http://sbols.org/v2#role";
const TITLE: &str = "http://purl.org/dc/terms/title";
const DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const CREATOR: &str = "http://purl.org/dc/elements/1.1/creator";
const MEMBER: &str = "http://sbols.org/v2#member";

/// BioPAX prefix `sbol2:type` values are filtered to (for the `?sbolType` column).
pub const BIOPAX_PREFIX: &str = "http://www.biopax.org/release/biopax-level3.owl";
/// Sequence Ontology prefix `sbol2:role` values are filtered to (`?role`).
pub const SO_PREFIX: &str = "http://identifiers.org/so/";

/// A literal value with its datatype/language, preserved so accelerated results
/// are byte-identical to what the generic engine would emit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LitVal {
    pub value: String,
    pub datatype: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Per-object metadata the SynBioHub queries project. Literal fields are
/// multi-valued: SPARQL `OPTIONAL` yields one row per value, so a faithful
/// accelerator preserves every value (most objects have exactly one).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetaRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_id: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub description: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sbol_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creators: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub top_level: bool,
}

impl MetaRecord {
    /// The sort key (first displayId) for displayId-ordered enumeration.
    pub fn sort_key(&self) -> &str {
        self.display_id
            .first()
            .map(|l| l.value.as_str())
            .unwrap_or("")
    }
}

/// One object and its metadata.
pub struct AccelObject {
    pub iri: String,
    pub meta: MetaRecord,
}

/// The derived accelerator index for a graph: per-object metadata, all collection
/// memberships, and the precomputed "root member" set (members not referenced by
/// another member). Backends persist this in their own idiom.
pub struct AccelIndex {
    pub objects: Vec<AccelObject>,
    /// (collection, member) for every membership.
    pub members: Vec<(String, String)>,
    /// (collection, member) for members not referenced by another member.
    pub root_members: Vec<(String, String)>,
}

/// Compute the accelerator index for a graph from its triples. Pure and
/// backend-neutral: every backend derives the same index, then persists/serves
/// it however it likes. The reference adjacency spans blank nodes (SBOL2 links
/// objects through blank Components), and the root-member set is the SynBioHub
/// `FILTER NOT EXISTS` anti-join (a member is root unless another member
/// references it directly or via a child).
pub fn build_accel_index(triples: &[Triple]) -> AccelIndex {
    let mut metas: HashMap<String, MetaRecord> = HashMap::new();
    let mut members_of: HashMap<String, Vec<String>> = HashMap::new();
    let mut out_edges: HashMap<String, Vec<String>> = HashMap::new();

    for t in triples {
        if let Some(object) = obj_node(&t.object) {
            let subject_node = subj_node(&t.subject);
            if t.predicate.as_str() == MEMBER {
                if let (SubjectTerm::Iri(_), ObjectTerm::Iri(_)) = (&t.subject, &t.object) {
                    members_of
                        .entry(subject_node.clone())
                        .or_default()
                        .push(object.clone());
                }
            }
            out_edges.entry(subject_node).or_default().push(object);
        }
        let subject = match &t.subject {
            SubjectTerm::Iri(iri) => iri.as_str(),
            SubjectTerm::BlankNode(_) => continue,
        };
        let m = metas.entry(subject.to_owned()).or_default();
        match t.predicate.as_str() {
            RDF_TYPE => {
                if let ObjectTerm::Iri(o) = &t.object {
                    m.types.push(o.as_str().to_owned());
                }
            }
            TOPLEVEL => {
                if let ObjectTerm::Iri(o) = &t.object {
                    if o.as_str() == subject {
                        m.top_level = true;
                    }
                }
            }
            DISPLAY_ID => push_literal(&mut m.display_id, &t.object),
            TITLE => push_literal(&mut m.name, &t.object),
            DESCRIPTION => push_literal(&mut m.description, &t.object),
            VERSION => push_literal(&mut m.version, &t.object),
            SBOL_TYPE => {
                if let ObjectTerm::Iri(o) = &t.object {
                    m.sbol_types.push(o.as_str().to_owned());
                }
            }
            ROLE => {
                if let ObjectTerm::Iri(o) = &t.object {
                    m.roles.push(o.as_str().to_owned());
                }
            }
            CREATOR => {
                if let Some(v) = scalar(&t.object) {
                    m.creators.push(v);
                }
            }
            _ => {}
        }
    }

    let mut members = Vec::new();
    let mut root_members = Vec::new();
    for (collection, mem) in &members_of {
        let member_set: HashSet<&str> = mem.iter().map(String::as_str).collect();
        let mut referenced: HashSet<&str> = HashSet::new();
        for om in mem {
            let Some(targets) = out_edges.get(om) else {
                continue;
            };
            for tgt in targets {
                if tgt != om && member_set.contains(tgt.as_str()) {
                    referenced.insert(tgt.as_str());
                }
                if let Some(grand) = out_edges.get(tgt) {
                    for y in grand {
                        if y != om && member_set.contains(y.as_str()) {
                            referenced.insert(y.as_str());
                        }
                    }
                }
            }
        }
        for m in mem {
            members.push((collection.clone(), m.clone()));
            if !referenced.contains(m.as_str()) {
                root_members.push((collection.clone(), m.clone()));
            }
        }
    }

    let objects = metas
        .into_iter()
        .map(|(iri, meta)| AccelObject { iri, meta })
        .collect();
    AccelIndex {
        objects,
        members,
        root_members,
    }
}

/// A node id for reference adjacency; blank nodes are kept (prefixed `_:`) as
/// traversable intermediates but never match the IRI member set.
fn subj_node(subject: &SubjectTerm) -> String {
    match subject {
        SubjectTerm::Iri(iri) => iri.as_str().to_owned(),
        SubjectTerm::BlankNode(b) => format!("_:{b}"),
    }
}

fn obj_node(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
        ObjectTerm::BlankNode(b) => Some(format!("_:{b}")),
        ObjectTerm::Literal { .. } => None,
    }
}

fn push_literal(values: &mut Vec<LitVal>, object: &ObjectTerm) {
    if let ObjectTerm::Literal {
        value,
        datatype,
        language,
    } = object
    {
        values.push(LitVal {
            value: value.clone(),
            datatype: datatype.as_str().to_owned(),
            language: language.clone(),
        });
    }
}

fn scalar(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
        ObjectTerm::Literal { value, .. } => Some(value.clone()),
        ObjectTerm::BlankNode(_) => None,
    }
}

/// A metadata field an accelerated projection can return for an object. Maps a
/// SELECT variable to the value the accelerator fills it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The object IRI (`?subject`/`?uri`).
    Subject,
    DisplayId,
    Version,
    Name,
    Description,
    /// An `rdf:type` of the object (multi-valued ⇒ one row per type).
    Type,
    /// A `sbol2:type` restricted to BioPAX (multi-valued, optional).
    SbolType,
    /// A `sbol2:role` restricted to the Sequence Ontology (multi-valued, optional).
    Role,
}

/// Which objects an accelerated query ranges over.
#[derive(Clone, Debug)]
pub enum Scope {
    /// Every top-level object (`?s sbh:topLevel ?s`).
    TopLevel,
    /// Every object with a given `rdf:type` (not restricted to top-level), e.g.
    /// `Count` over `ComponentDefinition`, or `getCollections` over `Collection`.
    ByType(String),
    /// Members of a collection. With `root_only`, only members not referenced by
    /// another member (directly or via a child) — SynBioHub's "top-level members"
    /// view, whose `FILTER NOT EXISTS` anti-join is precomputed at derive time.
    Collection { collection: String, root_only: bool },
}

/// A distinct-value facet over top-level objects.
#[derive(Clone, Copy, Debug)]
pub enum FacetKind {
    /// Distinct `rdf:type` values (`getTypes`).
    Types,
    /// Distinct `sbol2:role` values (`getRoles`).
    Roles,
    /// Distinct `dc:creator` values (`getCreators`).
    Creators,
}

/// A recognized SynBioHub query resolved to accelerator parameters.
#[derive(Clone, Debug)]
pub enum AcceleratedQuery {
    /// List objects in `scope` with a per-object metadata projection, ordered by
    /// displayId, paginated. Reproduces the template's `SELECT DISTINCT` over the
    /// multi-valued `type`/`sbolType`/`role` columns (one row per combination).
    ObjectList {
        graph: String,
        scope: Scope,
        projection: Vec<(String, Field)>,
        offset: usize,
        limit: Option<usize>,
        /// A `STRSTARTS(str(?subject), prefix)` filter from the template (the
        /// member-namespace filter on collection-member queries), if present.
        subject_prefix: Option<String>,
    },
    /// Count distinct objects in `scope` (`Count`, `searchCount`). `var` is the
    /// count's result variable.
    Count {
        graph: String,
        scope: Scope,
        var: String,
        subject_prefix: Option<String>,
    },
    /// Distinct facet values over top-level objects (`getTypes`/`getRoles`/
    /// `getCreators`). `var` is the projected variable.
    Facet {
        graph: String,
        kind: FacetKind,
        var: String,
    },
    /// One specific object's metadata projection (`getMetadata`): a constant
    /// `subject` with a per-field projection, answered by a primary-key lookup
    /// on the object's metadata record. `required[i]` marks a column whose
    /// triple pattern is outside any `OPTIONAL`, so the object yields no rows
    /// when that field has no value (an inner join); an optional column instead
    /// contributes an unbound cell.
    ObjectMetadata {
        graph: String,
        subject: String,
        projection: Vec<(String, Field)>,
        required: Vec<bool>,
    },
}

/// A backend's answer to an [`AcceleratedQuery`]: a SPARQL solution sequence.
/// `vars` are the projected variable names (without `?`); each row has one cell
/// per variable, `None` for an unbound (optional) value.
#[derive(Clone, Debug, Default)]
pub struct AccelSolutions {
    pub vars: Vec<String>,
    pub rows: Vec<Vec<Option<TermValue>>>,
}

/// Build one object's rows as the cartesian product of each projected column's
/// possible values, matching the SynBioHub templates' `SELECT DISTINCT` over the
/// multi-valued columns. Every column is optional and contributes an unbound
/// cell when it has no value, so each object yields at least one row (the
/// `?subject` column is always bound). Backends call this with an object's IRI
/// and its [`MetaRecord`]; the rows are appended to `out`.
pub fn generate_rows(
    iri: &str,
    meta: &MetaRecord,
    projection: &[(String, Field)],
    out: &mut Vec<Vec<Option<TermValue>>>,
) {
    let columns: Vec<Vec<Option<TermValue>>> = projection
        .iter()
        .map(|(_, field)| field_values(field, iri, meta))
        .collect();
    cartesian(&columns, out);
}

/// Build one object's rows for a constant-subject metadata query (`getMetadata`),
/// honoring `required`: a required column uses its raw values, so when it has
/// none the cartesian product is empty and the object yields no rows (an inner
/// join); an optional column contributes an unbound cell when empty. `required`
/// is parallel to `projection`. Rows are appended to `out`.
pub fn generate_metadata_rows(
    iri: &str,
    meta: &MetaRecord,
    projection: &[(String, Field)],
    required: &[bool],
    out: &mut Vec<Vec<Option<TermValue>>>,
) {
    let mut columns: Vec<Vec<Option<TermValue>>> = Vec::with_capacity(projection.len());
    for (i, (_, field)) in projection.iter().enumerate() {
        let values = field_values_raw(field, iri, meta);
        if required.get(i).copied().unwrap_or(false) {
            if values.is_empty() {
                return;
            }
            columns.push(values);
        } else if values.is_empty() {
            columns.push(vec![None]);
        } else {
            columns.push(values);
        }
    }
    cartesian(&columns, out);
}

/// Append the cartesian product of `columns` (one cell per column) to `out`.
fn cartesian(columns: &[Vec<Option<TermValue>>], out: &mut Vec<Vec<Option<TermValue>>>) {
    let mut rows: Vec<Vec<Option<TermValue>>> = vec![Vec::new()];
    for column in columns {
        let mut next = Vec::with_capacity(rows.len() * column.len());
        for prefix in &rows {
            for value in column {
                let mut row = prefix.clone();
                row.push(value.clone());
                next.push(row);
            }
        }
        rows = next;
    }
    out.extend(rows);
}

/// The possible cell values for one projected column, treated as optional: a
/// column with no value yields a single unbound cell, so an object never drops
/// out for a missing optional.
fn field_values(field: &Field, iri: &str, meta: &MetaRecord) -> Vec<Option<TermValue>> {
    let values = field_values_raw(field, iri, meta);
    if values.is_empty() {
        vec![None]
    } else {
        values
    }
}

/// A column's raw values, with no unbound-cell fallback: the literal columns and
/// the IRI columns (`?type`, `?sbolType` filtered to BioPAX, `?role` filtered to
/// the Sequence Ontology) return an empty vec when the object has no such value.
/// `?subject` is always a single bound cell.
fn field_values_raw(field: &Field, iri: &str, meta: &MetaRecord) -> Vec<Option<TermValue>> {
    match field {
        Field::Subject => vec![Some(TermValue::Iri(iri.to_owned()))],
        Field::DisplayId => lit_terms(&meta.display_id),
        Field::Version => lit_terms(&meta.version),
        Field::Name => lit_terms(&meta.name),
        Field::Description => lit_terms(&meta.description),
        Field::Type => iri_terms(&meta.types, ""),
        Field::SbolType => iri_terms(&meta.sbol_types, BIOPAX_PREFIX),
        Field::Role => iri_terms(&meta.roles, SO_PREFIX),
    }
}

fn lit_terms(values: &[LitVal]) -> Vec<Option<TermValue>> {
    values
        .iter()
        .map(|l| {
            Some(TermValue::Literal {
                value: l.value.clone(),
                datatype: l.datatype.clone(),
                language: l.language.clone(),
            })
        })
        .collect()
}

fn iri_terms(values: &[String], iri_prefix: &str) -> Vec<Option<TermValue>> {
    values
        .iter()
        .filter(|v| v.starts_with(iri_prefix))
        .map(|v| Some(TermValue::Iri(v.clone())))
        .collect()
}

/// An `xsd:integer` term for a count result.
pub fn integer(n: u64) -> TermValue {
    TermValue::Literal {
        value: n.to_string(),
        datatype: "http://www.w3.org/2001/XMLSchema#integer".to_owned(),
        language: None,
    }
}
