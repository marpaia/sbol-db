//! The SynBioHub query accelerator: per-graph derived indexes that answer the
//! fixed SynBioHub query templates with range scans and point lookups instead
//! of graph-pattern evaluation.
//!
//! The indexes are derived from a graph's triples (not from an SBOL parse), so
//! they are maintained on the verbatim Graph Store write path SynBioHub uses.
//! Derivation is deferred: a write marks the graph dirty, and the next read that
//! needs the indexes rebuilds them in one pass. The rebuild clears the dirty
//! flag *before* scanning, so a write that lands during a rebuild re-marks the
//! graph and the next read rebuilds again (never serving stale data).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, ObjectTerm, SubjectTerm};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, FacetKind, Field, GraphFilter, Scope, TermValue,
};
use serde::{Deserialize, Serialize};

use crate::db::{compose, Db, SEP};
use crate::repo::triple::TripleRepository;

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
const BIOPAX_PREFIX: &str = "http://www.biopax.org/release/biopax-level3.owl";
const SO_PREFIX: &str = "http://identifiers.org/so/";

const FK_TYPES: u8 = 1;
const FK_ROLES: u8 = 2;
const FK_CREATORS: u8 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// A literal value with its datatype/language, preserved so accelerated results
/// are byte-identical to what the generic engine would emit.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LitVal {
    value: String,
    datatype: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

impl LitVal {
    fn into_term(self) -> TermValue {
        TermValue::Literal {
            value: self.value,
            datatype: self.datatype,
            language: self.language,
        }
    }
}

/// Per-object metadata the SynBioHub queries project. Stored as JSON in
/// `acc_meta`. Literal fields are multi-valued: SPARQL `OPTIONAL` yields one row
/// per value, so a faithful accelerator must preserve every value (most objects
/// have exactly one).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MetaRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    display_id: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    name: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    description: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    version: Vec<LitVal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sbol_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    creators: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    top_level: bool,
}

/// Derives, maintains, and queries the accelerator indexes for a backend.
#[derive(Clone)]
pub struct AccelRepository {
    db: Db,
    triples: TripleRepository,
    /// Serializes rebuilds so two readers don't derive the same graph at once.
    rebuild: Arc<Mutex<()>>,
}

impl AccelRepository {
    pub fn new(db: Db, triples: TripleRepository) -> Self {
        Self {
            db,
            triples,
            rebuild: Arc::new(Mutex::new(())),
        }
    }

    /// Mark a graph's indexes stale within an existing write batch (atomic with
    /// the triple write).
    pub fn stage_mark_dirty(&self, batch: &mut WriteBatch, graph: &str) {
        let cf = self.db.cf("acc_dirty");
        batch.put_cf(&cf, graph.as_bytes(), []);
    }

    /// Answer a recognized query, rebuilding the graph's indexes first if stale.
    pub fn run(&self, query: &AcceleratedQuery) -> Result<AccelSolutions, DomainError> {
        match query {
            AcceleratedQuery::ObjectList {
                graph,
                scope,
                projection,
                offset,
                limit,
                subject_prefix,
            } => {
                self.ensure_fresh(graph)?;
                self.object_list(
                    graph,
                    scope,
                    projection,
                    *offset,
                    *limit,
                    subject_prefix.as_deref(),
                )
            }
            AcceleratedQuery::Count {
                graph,
                scope,
                var,
                subject_prefix,
            } => {
                self.ensure_fresh(graph)?;
                self.count(graph, scope, var, subject_prefix.as_deref())
            }
            AcceleratedQuery::Facet { graph, kind, var } => {
                self.ensure_fresh(graph)?;
                self.facet(graph, *kind, var)
            }
        }
    }

    fn ensure_fresh(&self, graph: &str) -> Result<(), DomainError> {
        let _guard = self.rebuild.lock().unwrap();
        if !self.db.exists_cf("acc_dirty", graph.as_bytes())? {
            return Ok(());
        }
        // Clear before scanning: a write during the rebuild re-marks the graph.
        self.db.delete_cf("acc_dirty", graph.as_bytes())?;
        self.derive(graph)
    }

    fn derive(&self, graph: &str) -> Result<(), DomainError> {
        let gp = prefix(&[graph.as_bytes()]);
        for cf in [
            "acc_meta",
            "acc_toplevel",
            "acc_bytype",
            "acc_member",
            "acc_rootmember",
            "acc_facet",
            "acc_count",
        ] {
            self.clear_prefix(cf, &gp)?;
        }

        let triples = self.triples.scan_pattern(
            None,
            None,
            None,
            Some(&GraphFilter::Iri(graph.to_owned())),
            i64::MAX,
        )?;

        let mut metas: HashMap<String, MetaRecord> = HashMap::new();
        // Collection -> its members, and every IRI subject -> its IRI objects, for
        // the root-member anti-join.
        let mut members_of: HashMap<String, Vec<String>> = HashMap::new();
        let mut out_edges: HashMap<String, Vec<String>> = HashMap::new();
        for t in &triples {
            // Reference adjacency spans blank nodes (SBOL2 links objects through
            // blank Components/Annotations), so build it over every node, not just
            // IRI subjects/objects.
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

        let mut batch = WriteBatch::default();
        let meta_cf = self.db.cf("acc_meta");
        let tl_cf = self.db.cf("acc_toplevel");
        let bt_cf = self.db.cf("acc_bytype");
        let fc_cf = self.db.cf("acc_facet");
        let mut toplevel_count: u64 = 0;
        let mut type_counts: HashMap<&str, u64> = HashMap::new();
        for (iri, m) in &metas {
            batch.put_cf(
                &meta_cf,
                key(&[graph.as_bytes(), iri.as_bytes()]),
                serde_json::to_vec(m).map_err(ser_err)?,
            );
            let sort = sort_key(m);
            for ty in &m.types {
                batch.put_cf(
                    &bt_cf,
                    key(&[
                        graph.as_bytes(),
                        ty.as_bytes(),
                        sort.as_bytes(),
                        iri.as_bytes(),
                    ]),
                    [],
                );
                *type_counts.entry(ty.as_str()).or_default() += 1;
            }
            if m.top_level {
                toplevel_count += 1;
                batch.put_cf(
                    &tl_cf,
                    key(&[graph.as_bytes(), sort.as_bytes(), iri.as_bytes()]),
                    [],
                );
                for ty in &m.types {
                    batch.put_cf(&fc_cf, facet_key(graph, FK_TYPES, ty), []);
                }
                for r in &m.roles {
                    batch.put_cf(&fc_cf, facet_key(graph, FK_ROLES, r), []);
                }
                for c in &m.creators {
                    batch.put_cf(&fc_cf, facet_key(graph, FK_CREATORS, c), []);
                }
            }
        }

        let count_cf = self.db.cf("acc_count");
        batch.put_cf(
            &count_cf,
            count_key_toplevel(graph),
            toplevel_count.to_le_bytes(),
        );
        for (ty, n) in &type_counts {
            batch.put_cf(&count_cf, count_key_type(graph, ty), n.to_le_bytes());
        }

        // Membership indexes, including the precomputed "root member" anti-join:
        // a member is a root unless another member references it directly or via a
        // child (matching the SynBioHub `FILTER NOT EXISTS` over a member-reference
        // UNION). The work is O(members x out-degree^2) once per derive.
        let mem_cf = self.db.cf("acc_member");
        let root_cf = self.db.cf("acc_rootmember");
        for (collection, members) in &members_of {
            let member_set: std::collections::HashSet<&str> =
                members.iter().map(String::as_str).collect();
            let mut referenced: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for om in members {
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
            let mut member_count: u64 = 0;
            let mut root_count: u64 = 0;
            for member in members {
                let sort = metas.get(member).map(sort_key).unwrap_or_default();
                let row_key = key(&[
                    graph.as_bytes(),
                    collection.as_bytes(),
                    sort.as_bytes(),
                    member.as_bytes(),
                ]);
                batch.put_cf(&mem_cf, &row_key, []);
                member_count += 1;
                if !referenced.contains(member.as_str()) {
                    batch.put_cf(&root_cf, &row_key, []);
                    root_count += 1;
                }
            }
            batch.put_cf(
                &count_cf,
                count_key_member(graph, collection, false),
                member_count.to_le_bytes(),
            );
            batch.put_cf(
                &count_cf,
                count_key_member(graph, collection, true),
                root_count.to_le_bytes(),
            );
        }
        self.db.write(batch)
    }

    fn object_list(
        &self,
        graph: &str,
        scope: &Scope,
        projection: &[(String, Field)],
        offset: usize,
        limit: Option<usize>,
        subject_prefix: Option<&str>,
    ) -> Result<AccelSolutions, DomainError> {
        let (cf, scan_prefix) = scope_scan(graph, scope);
        let mut iris: Vec<String> = Vec::new();
        self.db.for_each_prefix(cf, &scan_prefix, |key, _| {
            let iri = String::from_utf8_lossy(last_component(key)).into_owned();
            if subject_prefix.is_none_or(|p| iri.starts_with(p)) {
                iris.push(iri);
            }
            Ok(true)
        })?;

        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        // Generate rows in displayId order, dedup as we go, and stop once we have
        // enough for the requested page (objects are visited in order, so a row's
        // position is fixed by its object).
        let target = limit.map(|l| offset + l);
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<Vec<Option<TermValue>>> = Vec::new();
        let mut object_rows = Vec::new();
        for iri in &iris {
            // A member with no metadata (e.g. an external reference) still yields
            // one row with the subject bound and the optional columns unbound.
            let meta = self.load_meta(graph, iri)?.unwrap_or_default();
            object_rows.clear();
            generate_rows(iri, &meta, projection, &mut object_rows);
            for row in object_rows.drain(..) {
                if seen.insert(format!("{row:?}")) {
                    rows.push(row);
                }
            }
            if target.is_some_and(|t| rows.len() >= t) {
                break;
            }
        }
        let rows = rows
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        Ok(AccelSolutions { vars, rows })
    }

    fn count(
        &self,
        graph: &str,
        scope: &Scope,
        var: &str,
        subject_prefix: Option<&str>,
    ) -> Result<AccelSolutions, DomainError> {
        let n = if let Some(prefix) = subject_prefix {
            // A subject-prefix filter rules out the precomputed counter; count the
            // matching entries from the enumeration index instead.
            let (cf, scan_prefix) = scope_scan(graph, scope);
            let mut n: u64 = 0;
            self.db.for_each_prefix(cf, &scan_prefix, |key, _| {
                if last_component(key).starts_with(prefix.as_bytes()) {
                    n += 1;
                }
                Ok(true)
            })?;
            n
        } else {
            let count_key = match scope {
                Scope::TopLevel => count_key_toplevel(graph),
                Scope::ByType(t) => count_key_type(graph, t),
                Scope::Collection {
                    collection,
                    root_only,
                } => count_key_member(graph, collection, *root_only),
            };
            match self.db.get_cf("acc_count", &count_key)? {
                Some(bytes) if bytes.len() == 8 => u64::from_le_bytes(bytes.try_into().unwrap()),
                _ => 0,
            }
        };
        Ok(AccelSolutions {
            vars: vec![var.to_owned()],
            rows: vec![vec![Some(integer(n))]],
        })
    }

    fn facet(
        &self,
        graph: &str,
        kind: FacetKind,
        var: &str,
    ) -> Result<AccelSolutions, DomainError> {
        let tag = match kind {
            FacetKind::Types => FK_TYPES,
            FacetKind::Roles => FK_ROLES,
            FacetKind::Creators => FK_CREATORS,
        };
        let scan_prefix = compose(&[graph.as_bytes(), &[SEP], &[tag], &[SEP]]);
        let mut rows = Vec::new();
        self.db
            .for_each_prefix("acc_facet", &scan_prefix, |key, _| {
                let value = String::from_utf8_lossy(last_component(key)).into_owned();
                let term = match kind {
                    FacetKind::Creators => TermValue::Literal {
                        value,
                        datatype: XSD_STRING.to_owned(),
                        language: None,
                    },
                    _ => TermValue::Iri(value),
                };
                rows.push(vec![Some(term)]);
                Ok(true)
            })?;
        Ok(AccelSolutions {
            vars: vec![var.to_owned()],
            rows,
        })
    }

    fn load_meta(&self, graph: &str, iri: &str) -> Result<Option<MetaRecord>, DomainError> {
        match self
            .db
            .get_cf("acc_meta", &key(&[graph.as_bytes(), iri.as_bytes()]))?
        {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(ser_err)?)),
            None => Ok(None),
        }
    }

    fn clear_prefix(&self, cf: &str, scan_prefix: &[u8]) -> Result<(), DomainError> {
        let mut keys = Vec::new();
        self.db.for_each_prefix(cf, scan_prefix, |key, _| {
            keys.push(key.to_owned());
            Ok(true)
        })?;
        let mut batch = WriteBatch::default();
        let handle = self.db.cf(cf);
        for key in keys {
            batch.delete_cf(&handle, key);
        }
        self.db.write(batch)
    }
}

/// Build one object's rows as the cartesian product of each projected column's
/// possible values, matching the SynBioHub templates' `SELECT DISTINCT` over the
/// multi-valued columns. `?type` is a required join (the object yields no rows if
/// untyped and `Type` is projected); the literal columns and `?sbolType`
/// (BioPAX) / `?role` (Sequence Ontology) are optional and contribute an unbound
/// cell when they have no value.
fn generate_rows(
    iri: &str,
    meta: &MetaRecord,
    projection: &[(String, Field)],
    out: &mut Vec<Vec<Option<TermValue>>>,
) {
    let mut columns: Vec<Vec<Option<TermValue>>> = Vec::with_capacity(projection.len());
    for (_, field) in projection {
        match field_values(field, iri, meta) {
            Some(values) => columns.push(values),
            None => return, // a required column with no value: no rows
        }
    }
    let mut rows: Vec<Vec<Option<TermValue>>> = vec![Vec::new()];
    for column in columns {
        let mut next = Vec::with_capacity(rows.len() * column.len());
        for prefix in &rows {
            for value in &column {
                let mut row = prefix.clone();
                row.push(value.clone());
                next.push(row);
            }
        }
        rows = next;
    }
    out.extend(rows);
}

/// The possible cell values for one projected column. `None` means the object
/// produces no rows (a required column with no value).
fn field_values(field: &Field, iri: &str, meta: &MetaRecord) -> Option<Vec<Option<TermValue>>> {
    let optional = |values: Vec<Option<TermValue>>| {
        if values.is_empty() {
            vec![None]
        } else {
            values
        }
    };
    Some(match field {
        Field::Subject => vec![Some(TermValue::Iri(iri.to_owned()))],
        Field::DisplayId => optional(lit_terms(&meta.display_id)),
        Field::Version => optional(lit_terms(&meta.version)),
        Field::Name => optional(lit_terms(&meta.name)),
        Field::Description => optional(lit_terms(&meta.description)),
        // `?type` is bound by `OPTIONAL { ?s a ?type }` in the collection-member
        // queries, so it is optional here; SynBioHub's top-level objects always
        // carry a type, so this also matches `search`'s required `?s a ?type`.
        Field::Type => optional(iri_terms(&meta.types, "")),
        Field::SbolType => optional(iri_terms(&meta.sbol_types, BIOPAX_PREFIX)),
        Field::Role => optional(iri_terms(&meta.roles, SO_PREFIX)),
    })
}

fn lit_terms(values: &[LitVal]) -> Vec<Option<TermValue>> {
    values
        .iter()
        .cloned()
        .map(|l| Some(l.into_term()))
        .collect()
}

fn iri_terms(values: &[String], iri_prefix: &str) -> Vec<Option<TermValue>> {
    values
        .iter()
        .filter(|v| v.starts_with(iri_prefix))
        .map(|v| Some(TermValue::Iri(v.clone())))
        .collect()
}

fn count_key_toplevel(graph: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"tl"])
}

fn count_key_type(graph: &str, type_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"ty", type_iri.as_bytes()])
}

/// The enumeration column family and scan prefix for a scope's members, in
/// displayId order.
fn scope_scan(graph: &str, scope: &Scope) -> (&'static str, Vec<u8>) {
    match scope {
        Scope::TopLevel => ("acc_toplevel", prefix(&[graph.as_bytes()])),
        Scope::ByType(t) => ("acc_bytype", prefix(&[graph.as_bytes(), t.as_bytes()])),
        Scope::Collection {
            collection,
            root_only,
        } => (
            if *root_only {
                "acc_rootmember"
            } else {
                "acc_member"
            },
            prefix(&[graph.as_bytes(), collection.as_bytes()]),
        ),
    }
}

fn count_key_member(graph: &str, collection: &str, root_only: bool) -> Vec<u8> {
    let tag: &[u8] = if root_only { b"rmem" } else { b"mem" };
    key(&[graph.as_bytes(), tag, collection.as_bytes()])
}

/// A node identifier for reference-adjacency. Blank nodes are kept (prefixed
/// `_:`) so reference chains through them are traversable; they never match the
/// IRI member set, only serve as intermediates.
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

fn sort_key(meta: &MetaRecord) -> String {
    meta.display_id
        .first()
        .map(|l| l.value.clone())
        .unwrap_or_default()
}

fn integer(n: u64) -> TermValue {
    TermValue::Literal {
        value: n.to_string(),
        datatype: "http://www.w3.org/2001/XMLSchema#integer".to_owned(),
        language: None,
    }
}

/// A full key: parts joined by `SEP` with no trailing separator, so the final
/// part (the IRI) is recoverable by [`last_component`].
fn key(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(SEP);
        }
        out.extend_from_slice(part);
    }
    out
}

/// A scan prefix: the key parts followed by a trailing `SEP`, so iteration is
/// bounded to keys under exactly these parts (none of which contain `SEP`).
fn prefix(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = key(parts);
    out.push(SEP);
    out
}

fn facet_key(graph: &str, tag: u8, value: &str) -> Vec<u8> {
    compose(&[graph.as_bytes(), &[SEP], &[tag], &[SEP], value.as_bytes()])
}

fn last_component(key: &[u8]) -> &[u8] {
    match key.iter().rposition(|&b| b == SEP) {
        Some(pos) => &key[pos + 1..],
        None => key,
    }
}

fn ser_err(e: serde_json::Error) -> DomainError {
    DomainError::Database(format!("accel serde: {e}"))
}
