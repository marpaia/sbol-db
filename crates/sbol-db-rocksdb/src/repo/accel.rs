//! The SynBioHub query accelerator: per-graph derived indexes that answer the
//! fixed SynBioHub query templates with range scans and point lookups instead
//! of graph-pattern evaluation.
//!
//! The indexes are derived from a graph's triples (not from an SBOL parse), so
//! they are maintained synchronously on the verbatim Graph Store write path
//! SynBioHub uses: [`AccelRepository::stage_refresh`] derives a graph's indexes
//! from its post-write triples and stages the deletes and puts into the write's
//! own batch, so the indexes commit atomically with the triples. Reads (which
//! never rebuild) always see indexes consistent with the committed triples.

use std::collections::{HashMap, HashSet};

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, Triple};
use sbol_db_storage::{
    build_accel_index, generate_metadata_rows, generate_rows, integer, AccelSolutions,
    AcceleratedQuery, FacetKind, Field, MetaRecord, Scope, TermValue,
};

use crate::db::{compose, Db, SEP};

const FK_TYPES: u8 = 1;
const FK_ROLES: u8 = 2;
const FK_CREATORS: u8 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Derives, maintains, and queries the accelerator indexes for a backend.
#[derive(Clone)]
pub struct AccelRepository {
    db: Db,
}

impl AccelRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub(crate) fn stage_import_object(
        &self,
        batch: &mut WriteBatch,
        graph: &str,
        iri: &str,
        meta: &MetaRecord,
    ) -> Result<(), DomainError> {
        let sort = meta.sort_key();
        let encoded = serde_json::to_vec(meta).map_err(ser_err)?;
        batch.put_cf(
            &self.db.cf("acc_meta"),
            key(&[graph.as_bytes(), iri.as_bytes()]),
            &encoded,
        );
        for ty in &meta.types {
            batch.put_cf(
                &self.db.cf("acc_bytype"),
                key(&[
                    graph.as_bytes(),
                    ty.as_bytes(),
                    sort.as_bytes(),
                    iri.as_bytes(),
                ]),
                &encoded,
            );
        }
        if meta.top_level {
            batch.put_cf(
                &self.db.cf("acc_toplevel"),
                key(&[graph.as_bytes(), sort.as_bytes(), iri.as_bytes()]),
                &encoded,
            );
            for role in &meta.roles {
                batch.put_cf(
                    &self.db.cf("acc_byrole"),
                    key(&[
                        graph.as_bytes(),
                        role.as_bytes(),
                        sort.as_bytes(),
                        iri.as_bytes(),
                    ]),
                    &encoded,
                );
            }
        }
        Ok(())
    }

    pub(crate) fn stage_import_member(
        &self,
        batch: &mut WriteBatch,
        graph: &str,
        collection: &str,
        member: &str,
        sort: &str,
        is_root: bool,
    ) {
        batch.put_cf(
            &self.db.cf("acc_member"),
            key(&[
                graph.as_bytes(),
                collection.as_bytes(),
                sort.as_bytes(),
                member.as_bytes(),
            ]),
            [],
        );
        batch.put_cf(
            &self.db.cf("acc_member_of"),
            key(&[graph.as_bytes(), member.as_bytes()]),
            [],
        );
        if is_root {
            batch.put_cf(
                &self.db.cf("acc_rootmember"),
                key(&[
                    graph.as_bytes(),
                    collection.as_bytes(),
                    sort.as_bytes(),
                    member.as_bytes(),
                ]),
                [],
            );
        }
    }

    pub(crate) fn stage_import_facet(
        &self,
        batch: &mut WriteBatch,
        graph: &str,
        kind: FacetKind,
        value: &str,
        count: u64,
    ) {
        batch.put_cf(
            &self.db.cf("acc_facet"),
            facet_key(graph, facet_tag(kind), value),
            count.to_le_bytes(),
        );
    }

    pub(crate) fn stage_import_count(&self, batch: &mut WriteBatch, key: Vec<u8>, count: u64) {
        batch.put_cf(&self.db.cf("acc_count"), key, count.to_le_bytes());
    }

    /// Answer a recognized query from the graph's accelerator indexes, which the
    /// write path keeps in sync with the committed triples (see
    /// [`Self::stage_refresh`]).
    pub fn run(&self, query: &AcceleratedQuery) -> Result<AccelSolutions, DomainError> {
        match query {
            AcceleratedQuery::ObjectList {
                graph,
                scope,
                projection,
                offset,
                limit,
                subject_prefix,
            } => self.object_list(
                graph,
                scope,
                projection,
                *offset,
                *limit,
                subject_prefix.as_deref(),
            ),
            AcceleratedQuery::Count {
                graph,
                scope,
                var,
                subject_prefix,
            } => self.count(graph, scope, var, subject_prefix.as_deref()),
            AcceleratedQuery::Facet { graph, kind, var } => self.facet(graph, *kind, var),
            AcceleratedQuery::FacetCounts {
                graph,
                kind,
                value_var,
                count_var,
            } => self.facet_counts(graph, *kind, value_var, count_var),
            AcceleratedQuery::ObjectMetadata {
                graph,
                subject,
                projection,
                required,
            } => self.object_metadata(graph, subject, projection, required),
        }
    }

    /// Rebuild a graph's accelerator indexes from `triples` (the graph's
    /// post-write triple set), staging the work into the caller's write batch so
    /// it commits atomically with the triple write. Old per-graph index keys are
    /// deleted and the freshly derived keys put. Callers invoke this after every
    /// write that changes a graph's triples; reads never rebuild.
    pub fn stage_refresh(
        &self,
        batch: &mut WriteBatch,
        graph: &str,
        triples: &[Triple],
    ) -> Result<(), DomainError> {
        let gp = prefix(&[graph.as_bytes()]);
        for cf in [
            "acc_meta",
            "acc_toplevel",
            "acc_bytype",
            "acc_byrole",
            "acc_member",
            "acc_rootmember",
            "acc_member_of",
            "acc_facet",
            "acc_count",
        ] {
            self.stage_clear_prefix(batch, cf, &gp)?;
        }

        // Callers reconstruct the post-write triple set by concatenating the
        // committed scan with the batch's inserts (the batch's writes are not yet
        // visible to a scan). A triple that is both already committed and
        // re-posted therefore appears twice, which would inflate the derived
        // metadata (e.g. duplicate `dcterms:title` values yield duplicate
        // metadata rows). Dedup to the true triple set before deriving, so the
        // index matches a clean rescan (what the SQL backends feed).
        let mut seen: HashSet<&Triple> = HashSet::with_capacity(triples.len());
        let deduped: Vec<Triple> = triples
            .iter()
            .filter(|t| seen.insert(*t))
            .cloned()
            .collect();
        let index = build_accel_index(&deduped);

        let meta_cf = self.db.cf("acc_meta");
        let tl_cf = self.db.cf("acc_toplevel");
        let bt_cf = self.db.cf("acc_bytype");
        let br_cf = self.db.cf("acc_byrole");
        let fc_cf = self.db.cf("acc_facet");
        let count_cf = self.db.cf("acc_count");

        let mut toplevel_count: u64 = 0;
        let mut type_counts: HashMap<&str, u64> = HashMap::new();
        let mut toplevel_type_counts: HashMap<&str, u64> = HashMap::new();
        let mut role_counts: HashMap<&str, u64> = HashMap::new();
        let mut type_role_counts: HashMap<(&str, &str), u64> = HashMap::new();
        let mut creator_counts: HashMap<&str, u64> = HashMap::new();
        let members: HashSet<&str> = index
            .members
            .iter()
            .map(|(_, member)| member.as_str())
            .collect();
        let mut root_type_counts: HashMap<&str, u64> = HashMap::new();
        let mut root_toplevel_type_counts: HashMap<&str, u64> = HashMap::new();
        // Member displayId sort keys, for ordering the membership indexes.
        let mut sort_of: HashMap<&str, &str> = HashMap::new();
        for obj in &index.objects {
            let iri = obj.iri.as_str();
            let m = &obj.meta;
            let sort = m.sort_key();
            let encoded = serde_json::to_vec(m).map_err(ser_err)?;
            sort_of.insert(iri, sort);
            batch.put_cf(&meta_cf, key(&[graph.as_bytes(), iri.as_bytes()]), &encoded);
            for ty in &m.types {
                batch.put_cf(
                    &bt_cf,
                    key(&[
                        graph.as_bytes(),
                        ty.as_bytes(),
                        sort.as_bytes(),
                        iri.as_bytes(),
                    ]),
                    &encoded,
                );
                *type_counts.entry(ty.as_str()).or_default() += 1;
                if !members.contains(iri) {
                    *root_type_counts.entry(ty.as_str()).or_default() += 1;
                }
            }
            if m.top_level {
                toplevel_count += 1;
                batch.put_cf(
                    &tl_cf,
                    key(&[graph.as_bytes(), sort.as_bytes(), iri.as_bytes()]),
                    &encoded,
                );
                for ty in &m.types {
                    *toplevel_type_counts.entry(ty.as_str()).or_default() += 1;
                    if !members.contains(iri) {
                        *root_toplevel_type_counts.entry(ty.as_str()).or_default() += 1;
                    }
                }
                for r in &m.roles {
                    batch.put_cf(
                        &br_cf,
                        key(&[
                            graph.as_bytes(),
                            r.as_bytes(),
                            sort.as_bytes(),
                            iri.as_bytes(),
                        ]),
                        &encoded,
                    );
                    *role_counts.entry(r.as_str()).or_default() += 1;
                    for ty in &m.types {
                        *type_role_counts
                            .entry((ty.as_str(), r.as_str()))
                            .or_default() += 1;
                    }
                }
                for c in &m.creators {
                    *creator_counts.entry(c.as_str()).or_default() += 1;
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
        for (ty, n) in &toplevel_type_counts {
            batch.put_cf(
                &count_cf,
                count_key_toplevel_type(graph, ty),
                n.to_le_bytes(),
            );
            batch.put_cf(&fc_cf, facet_key(graph, FK_TYPES, ty), n.to_le_bytes());
        }
        for (ty, n) in &root_type_counts {
            batch.put_cf(&count_cf, count_key_root_type(graph, ty), n.to_le_bytes());
        }
        for (ty, n) in &root_toplevel_type_counts {
            batch.put_cf(
                &count_cf,
                count_key_root_toplevel_type(graph, ty),
                n.to_le_bytes(),
            );
        }
        for (role, n) in &role_counts {
            batch.put_cf(&count_cf, count_key_role(graph, role), n.to_le_bytes());
            batch.put_cf(&fc_cf, facet_key(graph, FK_ROLES, role), n.to_le_bytes());
        }
        for ((ty, role), n) in &type_role_counts {
            batch.put_cf(
                &count_cf,
                count_key_toplevel_type_role(graph, ty, role),
                n.to_le_bytes(),
            );
        }
        for (creator, n) in &creator_counts {
            batch.put_cf(
                &fc_cf,
                facet_key(graph, FK_CREATORS, creator),
                n.to_le_bytes(),
            );
        }

        // Membership indexes, including the precomputed "root member" anti-join
        // (members not referenced by another member directly or via a child),
        // computed in `build_accel_index`. Every collection with members gets both
        // counters (the root counter may be 0).
        let mem_cf = self.db.cf("acc_member");
        let root_cf = self.db.cf("acc_rootmember");
        let member_of_cf = self.db.cf("acc_member_of");
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
            batch.put_cf(
                &member_of_cf,
                key(&[graph.as_bytes(), member.as_bytes()]),
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
        Ok(())
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
        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        // Generate rows in displayId order, dedup as we go, and stop once we have
        // enough for the requested page (objects are visited in order, so a row's
        // position is fixed by its object).
        let target = limit.map(|l| offset + l);
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<Vec<Option<TermValue>>> = Vec::new();
        let mut object_rows = Vec::new();
        let (cf, scan_prefix) = scope_scan(graph, scope);
        self.db
            .for_each_prefix(cf, &scan_prefix, |index_key, index_value| {
                let iri = String::from_utf8_lossy(last_component(index_key)).into_owned();
                if !subject_prefix.is_none_or(|prefix| iri.starts_with(prefix)) {
                    return Ok(true);
                }
                // New secondary-index records carry primary metadata so an
                // ordered page is a sequential read. Empty values remain
                // compatible with databases created before that optimization.
                let meta = if index_value.is_empty() {
                    self.load_meta(graph, &iri)?.unwrap_or_default()
                } else {
                    serde_json::from_slice(index_value).map_err(ser_err)?
                };
                let is_member =
                    matches!(scope, Scope::RootByType(_) | Scope::RootTopLevelByType(_))
                        && self.db.exists_cf(
                            "acc_member_of",
                            &key(&[graph.as_bytes(), iri.as_bytes()]),
                        )?;
                if !scope_includes_loaded(scope, &meta, is_member) {
                    return Ok(true);
                }
                // A member with no metadata (e.g. an external reference) still
                // yields one row with the subject bound and optional columns
                // unbound.
                object_rows.clear();
                generate_rows(&iri, &meta, projection, &mut object_rows);
                for row in object_rows.drain(..) {
                    if seen.insert(format!("{row:?}")) {
                        rows.push(row);
                    }
                }
                Ok(!target.is_some_and(|target| rows.len() >= target))
            })?;
        let rows = rows
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        Ok(AccelSolutions { vars, rows })
    }

    /// Fetch one object's metadata projection by primary key. A missing or
    /// metadata-less object yields no rows (the required title cannot bind).
    fn object_metadata(
        &self,
        graph: &str,
        subject: &str,
        projection: &[(String, Field)],
        required: &[bool],
    ) -> Result<AccelSolutions, DomainError> {
        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        let mut rows = Vec::new();
        if let Some(meta) = self.load_meta(graph, subject)? {
            generate_metadata_rows(subject, &meta, projection, required, &mut rows);
        }
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
                let iri = String::from_utf8_lossy(last_component(key));
                if iri.starts_with(prefix) && self.scope_includes(graph, scope, &iri)? {
                    n += 1;
                }
                Ok(true)
            })?;
            n
        } else {
            let count_key = match scope {
                Scope::TopLevel => count_key_toplevel(graph),
                Scope::ByType(t) => count_key_type(graph, t),
                Scope::RootByType(t) => count_key_root_type(graph, t),
                Scope::TopLevelByType(t) => count_key_toplevel_type(graph, t),
                Scope::RootTopLevelByType(t) => count_key_root_toplevel_type(graph, t),
                Scope::TopLevelByRole(role) => count_key_role(graph, role),
                Scope::TopLevelByTypeAndRole { object_type, role } => {
                    count_key_toplevel_type_role(graph, object_type, role)
                }
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

    fn facet_counts(
        &self,
        graph: &str,
        kind: FacetKind,
        value_var: &str,
        count_var: &str,
    ) -> Result<AccelSolutions, DomainError> {
        let tag = facet_tag(kind);
        let scan_prefix = compose(&[graph.as_bytes(), &[SEP], &[tag], &[SEP]]);
        let mut rows = Vec::new();
        self.db
            .for_each_prefix("acc_facet", &scan_prefix, |key, value| {
                let facet = String::from_utf8_lossy(last_component(key)).into_owned();
                let subject_count = decode_count(value);
                let term = match kind {
                    FacetKind::Creators => TermValue::Literal {
                        value: facet,
                        datatype: XSD_STRING.to_owned(),
                        language: None,
                    },
                    _ => TermValue::Iri(facet),
                };
                rows.push(vec![Some(term), Some(integer(subject_count))]);
                Ok(true)
            })?;
        Ok(AccelSolutions {
            vars: vec![value_var.to_owned(), count_var.to_owned()],
            rows,
        })
    }

    fn scope_includes(&self, graph: &str, scope: &Scope, iri: &str) -> Result<bool, DomainError> {
        let metadata = || self.load_meta(graph, iri);
        Ok(match scope {
            Scope::TopLevel
            | Scope::ByType(_)
            | Scope::TopLevelByRole(_)
            | Scope::Collection { .. } => true,
            Scope::TopLevelByType(_) => metadata()?.is_some_and(|meta| meta.top_level),
            Scope::RootByType(_) => !self
                .db
                .exists_cf("acc_member_of", &key(&[graph.as_bytes(), iri.as_bytes()]))?,
            Scope::RootTopLevelByType(_) => {
                metadata()?.is_some_and(|meta| meta.top_level)
                    && !self
                        .db
                        .exists_cf("acc_member_of", &key(&[graph.as_bytes(), iri.as_bytes()]))?
            }
            Scope::TopLevelByTypeAndRole { object_type, .. } => metadata()?.is_some_and(|meta| {
                meta.top_level && meta.types.iter().any(|ty| ty == object_type)
            }),
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

    /// Stage deletes for every key under `scan_prefix` in `cf` into `batch`, so
    /// the old per-graph index entries are removed atomically with the rebuild.
    fn stage_clear_prefix(
        &self,
        batch: &mut WriteBatch,
        cf: &str,
        scan_prefix: &[u8],
    ) -> Result<(), DomainError> {
        let mut keys = Vec::new();
        self.db.for_each_prefix(cf, scan_prefix, |key, _| {
            keys.push(key.to_owned());
            Ok(true)
        })?;
        let handle = self.db.cf(cf);
        for key in keys {
            batch.delete_cf(&handle, key);
        }
        Ok(())
    }
}

pub(crate) fn count_key_toplevel(graph: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"tl"])
}

pub(crate) fn count_key_type(graph: &str, type_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"ty", type_iri.as_bytes()])
}

pub(crate) fn count_key_role(graph: &str, role_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"role", role_iri.as_bytes()])
}

pub(crate) fn count_key_toplevel_type(graph: &str, type_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"tlty", type_iri.as_bytes()])
}

pub(crate) fn count_key_root_type(graph: &str, type_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"rootty", type_iri.as_bytes()])
}

pub(crate) fn count_key_root_toplevel_type(graph: &str, type_iri: &str) -> Vec<u8> {
    key(&[graph.as_bytes(), b"roottlty", type_iri.as_bytes()])
}

pub(crate) fn count_key_toplevel_type_role(graph: &str, type_iri: &str, role_iri: &str) -> Vec<u8> {
    key(&[
        graph.as_bytes(),
        b"tltyrole",
        type_iri.as_bytes(),
        role_iri.as_bytes(),
    ])
}

/// The enumeration column family and scan prefix for a scope's members, in
/// displayId order.
fn scope_scan(graph: &str, scope: &Scope) -> (&'static str, Vec<u8>) {
    match scope {
        Scope::TopLevel => ("acc_toplevel", prefix(&[graph.as_bytes()])),
        Scope::ByType(t) => ("acc_bytype", prefix(&[graph.as_bytes(), t.as_bytes()])),
        Scope::RootByType(t) | Scope::TopLevelByType(t) | Scope::RootTopLevelByType(t) => {
            ("acc_bytype", prefix(&[graph.as_bytes(), t.as_bytes()]))
        }
        Scope::TopLevelByRole(role) => ("acc_byrole", prefix(&[graph.as_bytes(), role.as_bytes()])),
        Scope::TopLevelByTypeAndRole { role, .. } => {
            ("acc_byrole", prefix(&[graph.as_bytes(), role.as_bytes()]))
        }
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

/// Apply the filters that are not already encoded by the selected enumeration
/// column family. The caller supplies batched primary metadata and reverse
/// membership results so a deep page does not issue point reads one object at
/// a time.
fn scope_includes_loaded(scope: &Scope, metadata: &MetaRecord, is_member: bool) -> bool {
    match scope {
        Scope::TopLevel
        | Scope::ByType(_)
        | Scope::TopLevelByRole(_)
        | Scope::Collection { .. } => true,
        Scope::TopLevelByType(_) => metadata.top_level,
        Scope::RootByType(_) => !is_member,
        Scope::RootTopLevelByType(_) => metadata.top_level && !is_member,
        Scope::TopLevelByTypeAndRole { object_type, .. } => {
            metadata.top_level && metadata.types.iter().any(|ty| ty == object_type)
        }
    }
}

pub(crate) fn count_key_member(graph: &str, collection: &str, root_only: bool) -> Vec<u8> {
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

fn facet_tag(kind: FacetKind) -> u8 {
    match kind {
        FacetKind::Types => FK_TYPES,
        FacetKind::Roles => FK_ROLES,
        FacetKind::Creators => FK_CREATORS,
    }
}

fn decode_count(bytes: &[u8]) -> u64 {
    if bytes.len() == 8 {
        u64::from_le_bytes(bytes.try_into().expect("checked count width"))
    } else {
        0
    }
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
