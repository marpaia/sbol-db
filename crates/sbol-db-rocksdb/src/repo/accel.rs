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
use sbol_db_core::DomainError;
use sbol_db_storage::{
    build_accel_index, generate_rows, integer, AccelSolutions, AcceleratedQuery, FacetKind, Field,
    GraphFilter, MetaRecord, Scope, TermValue,
};

use crate::db::{compose, Db, SEP};
use crate::repo::triple::TripleRepository;

const FK_TYPES: u8 = 1;
const FK_ROLES: u8 = 2;
const FK_CREATORS: u8 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

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
        let index = build_accel_index(&triples);

        let mut batch = WriteBatch::default();
        let meta_cf = self.db.cf("acc_meta");
        let tl_cf = self.db.cf("acc_toplevel");
        let bt_cf = self.db.cf("acc_bytype");
        let fc_cf = self.db.cf("acc_facet");
        let count_cf = self.db.cf("acc_count");

        let mut toplevel_count: u64 = 0;
        let mut type_counts: HashMap<&str, u64> = HashMap::new();
        // Member displayId sort keys, for ordering the membership indexes.
        let mut sort_of: HashMap<&str, &str> = HashMap::new();
        for obj in &index.objects {
            let iri = obj.iri.as_str();
            let m = &obj.meta;
            let sort = m.sort_key();
            sort_of.insert(iri, sort);
            batch.put_cf(
                &meta_cf,
                key(&[graph.as_bytes(), iri.as_bytes()]),
                serde_json::to_vec(m).map_err(ser_err)?,
            );
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

        batch.put_cf(
            &count_cf,
            count_key_toplevel(graph),
            toplevel_count.to_le_bytes(),
        );
        for (ty, n) in &type_counts {
            batch.put_cf(&count_cf, count_key_type(graph, ty), n.to_le_bytes());
        }

        // Membership indexes, including the precomputed "root member" anti-join
        // (members not referenced by another member directly or via a child),
        // computed in `build_accel_index`. Every collection with members gets both
        // counters (the root counter may be 0).
        let mem_cf = self.db.cf("acc_member");
        let root_cf = self.db.cf("acc_rootmember");
        let mut member_counts: HashMap<&str, u64> = HashMap::new();
        let mut root_counts: HashMap<&str, u64> = HashMap::new();
        for (collection, member) in &index.members {
            let sort = sort_of.get(member.as_str()).copied().unwrap_or("");
            batch.put_cf(
                &mem_cf,
                key(&[
                    graph.as_bytes(),
                    collection.as_bytes(),
                    sort.as_bytes(),
                    member.as_bytes(),
                ]),
                [],
            );
            *member_counts.entry(collection.as_str()).or_default() += 1;
        }
        for (collection, member) in &index.root_members {
            let sort = sort_of.get(member.as_str()).copied().unwrap_or("");
            batch.put_cf(
                &root_cf,
                key(&[
                    graph.as_bytes(),
                    collection.as_bytes(),
                    sort.as_bytes(),
                    member.as_bytes(),
                ]),
                [],
            );
            *root_counts.entry(collection.as_str()).or_default() += 1;
        }
        for (collection, n) in &member_counts {
            batch.put_cf(
                &count_cf,
                count_key_member(graph, collection, false),
                n.to_le_bytes(),
            );
            let root = root_counts.get(collection).copied().unwrap_or(0);
            batch.put_cf(
                &count_cf,
                count_key_member(graph, collection, true),
                root.to_le_bytes(),
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
