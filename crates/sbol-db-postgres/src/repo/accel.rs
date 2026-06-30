//! The SynBioHub query accelerator for Postgres: per-graph derived indexes that
//! answer the fixed SynBioHub query templates with point lookups and range scans
//! instead of graph-pattern evaluation.
//!
//! The indexes are derived from a graph's triples (via the backend-neutral
//! [`build_accel_index`]) and maintained synchronously on the write path:
//! [`AccelRepository::refresh_graph`] rebuilds a graph's indexes inside the
//! write's own transaction, scanning the triples through that transaction's
//! connection so the indexes it writes reflect the triples the same transaction
//! wrote. Indexes and triples therefore commit together, and reads (which never
//! rebuild) always see indexes consistent with the committed triples.

use std::collections::{HashMap, HashSet};

use sbol_db_core::DomainError;
use sbol_db_storage::{
    build_accel_index, generate_metadata_rows, generate_rows, integer, AccelSolutions,
    AcceleratedQuery, FacetKind, Field, MetaRecord, Scope, TermValue,
};
use sqlx::{QueryBuilder, Row};

use crate::repo::db_err;
use crate::repo::triple::TripleRepository;
use crate::PgPool;

const FK_TYPES: i16 = 1;
const FK_ROLES: i16 = 2;
const FK_CREATORS: i16 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Derives, maintains, and queries the accelerator indexes for the Postgres
/// backend.
#[derive(Clone)]
pub struct AccelRepository {
    pool: PgPool,
    triples: TripleRepository,
}

impl AccelRepository {
    pub fn new(pool: PgPool, triples: TripleRepository) -> Self {
        Self { pool, triples }
    }

    /// Answer a recognized query from the graph's accelerator indexes, which the
    /// write path keeps in sync with the committed triples (see
    /// [`Self::refresh_graph`]).
    pub async fn run(&self, query: &AcceleratedQuery) -> Result<AccelSolutions, DomainError> {
        match query {
            AcceleratedQuery::ObjectList {
                graph,
                scope,
                projection,
                offset,
                limit,
                subject_prefix,
            } => {
                self.object_list(
                    graph,
                    scope,
                    projection,
                    *offset,
                    *limit,
                    subject_prefix.as_deref(),
                )
                .await
            }
            AcceleratedQuery::Count {
                graph,
                scope,
                var,
                subject_prefix,
            } => {
                self.count(graph, scope, var, subject_prefix.as_deref())
                    .await
            }
            AcceleratedQuery::Facet { graph, kind, var } => self.facet(graph, *kind, var).await,
            AcceleratedQuery::ObjectMetadata {
                graph,
                subject,
                projection,
                required,
            } => {
                self.object_metadata(graph, subject, projection, required)
                    .await
            }
        }
    }

    /// Rebuild a graph's accelerator indexes from its triples, inside the
    /// caller's write transaction (atomic with the triple write). The triple
    /// scan runs through `conn`, so it sees the triples the same transaction
    /// just wrote; the rebuilt indexes are deleted and reinserted on `conn` and
    /// commit together with the triples. Callers invoke this after every write
    /// that changes a graph's triples.
    pub async fn refresh_graph(
        &self,
        conn: &mut sqlx::PgConnection,
        graph: &str,
    ) -> Result<(), DomainError> {
        let triples = self.triples.scan_graph_in_conn(conn, graph).await?;
        let index = build_accel_index(&triples);

        let sort_of: HashMap<&str, &str> = index
            .objects
            .iter()
            .map(|o| (o.iri.as_str(), o.meta.sort_key()))
            .collect();
        let root_set: HashSet<(&str, &str)> = index
            .root_members
            .iter()
            .map(|(c, m)| (c.as_str(), m.as_str()))
            .collect();

        let mut obj_iri: Vec<String> = Vec::new();
        let mut obj_sort: Vec<String> = Vec::new();
        let mut obj_top: Vec<bool> = Vec::new();
        let mut obj_meta: Vec<String> = Vec::new();
        let mut ty_type: Vec<String> = Vec::new();
        let mut ty_iri: Vec<String> = Vec::new();
        let mut ty_sort: Vec<String> = Vec::new();
        let mut facet_kind: Vec<i16> = Vec::new();
        let mut facet_value: Vec<String> = Vec::new();
        let mut facet_seen: HashSet<(i16, &str)> = HashSet::new();
        for obj in &index.objects {
            let iri = obj.iri.as_str();
            let m = &obj.meta;
            let sort = m.sort_key();
            obj_iri.push(iri.to_owned());
            obj_sort.push(sort.to_owned());
            obj_top.push(m.top_level);
            obj_meta.push(serde_json::to_string(m).map_err(db_err)?);
            for t in &m.types {
                ty_type.push(t.clone());
                ty_iri.push(iri.to_owned());
                ty_sort.push(sort.to_owned());
            }
            if m.top_level {
                for t in &m.types {
                    if facet_seen.insert((FK_TYPES, t.as_str())) {
                        facet_kind.push(FK_TYPES);
                        facet_value.push(t.clone());
                    }
                }
                for r in &m.roles {
                    if facet_seen.insert((FK_ROLES, r.as_str())) {
                        facet_kind.push(FK_ROLES);
                        facet_value.push(r.clone());
                    }
                }
                for c in &m.creators {
                    if facet_seen.insert((FK_CREATORS, c.as_str())) {
                        facet_kind.push(FK_CREATORS);
                        facet_value.push(c.clone());
                    }
                }
            }
        }

        let mut mem_coll: Vec<String> = Vec::new();
        let mut mem_iri: Vec<String> = Vec::new();
        let mut mem_sort: Vec<String> = Vec::new();
        let mut mem_root: Vec<bool> = Vec::new();
        for (collection, member) in &index.members {
            mem_coll.push(collection.clone());
            mem_iri.push(member.clone());
            mem_sort.push(
                sort_of
                    .get(member.as_str())
                    .copied()
                    .unwrap_or("")
                    .to_owned(),
            );
            mem_root.push(root_set.contains(&(collection.as_str(), member.as_str())));
        }

        for table in ["accel_object", "accel_type", "accel_member", "accel_facet"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE graph_iri = $1"))
                .bind(graph)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?;
        }
        if !obj_iri.is_empty() {
            sqlx::query(
                "INSERT INTO accel_object (graph_iri, iri, sort_key, top_level, meta)
                 SELECT $1, iri, sort_key, top_level, meta
                 FROM UNNEST($2::text[], $3::text[], $4::bool[], $5::text[])
                    AS u(iri, sort_key, top_level, meta)",
            )
            .bind(graph)
            .bind(&obj_iri)
            .bind(&obj_sort)
            .bind(&obj_top)
            .bind(&obj_meta)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        if !ty_iri.is_empty() {
            sqlx::query(
                "INSERT INTO accel_type (graph_iri, type_iri, iri, sort_key)
                 SELECT $1, type_iri, iri, sort_key
                 FROM UNNEST($2::text[], $3::text[], $4::text[])
                    AS u(type_iri, iri, sort_key)
                 ON CONFLICT DO NOTHING",
            )
            .bind(graph)
            .bind(&ty_type)
            .bind(&ty_iri)
            .bind(&ty_sort)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        if !mem_iri.is_empty() {
            sqlx::query(
                "INSERT INTO accel_member (graph_iri, collection_iri, member_iri, sort_key, is_root)
                 SELECT $1, collection_iri, member_iri, sort_key, is_root
                 FROM UNNEST($2::text[], $3::text[], $4::text[], $5::bool[])
                    AS u(collection_iri, member_iri, sort_key, is_root)
                 ON CONFLICT DO NOTHING",
            )
            .bind(graph)
            .bind(&mem_coll)
            .bind(&mem_iri)
            .bind(&mem_sort)
            .bind(&mem_root)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        if !facet_value.is_empty() {
            sqlx::query(
                "INSERT INTO accel_facet (graph_iri, kind, value)
                 SELECT $1, kind, value
                 FROM UNNEST($2::smallint[], $3::text[]) AS u(kind, value)
                 ON CONFLICT DO NOTHING",
            )
            .bind(graph)
            .bind(&facet_kind)
            .bind(&facet_value)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        Ok(())
    }

    async fn object_list(
        &self,
        graph: &str,
        scope: &Scope,
        projection: &[(String, sbol_db_storage::Field)],
        offset: usize,
        limit: Option<usize>,
        subject_prefix: Option<&str>,
    ) -> Result<AccelSolutions, DomainError> {
        let candidates = self
            .scope_candidates(graph, scope, subject_prefix, offset, limit)
            .await?;

        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        // Generate rows in displayId order, dedup as we go, and stop once we have
        // enough for the requested page (objects are visited in order, and each
        // yields at least one row, so a row's position is fixed by its object).
        let target = limit.map(|l| offset + l);
        let mut seen = HashSet::new();
        let mut rows: Vec<Vec<Option<TermValue>>> = Vec::new();
        let mut object_rows = Vec::new();
        for (iri, meta) in &candidates {
            object_rows.clear();
            generate_rows(iri, meta, projection, &mut object_rows);
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

    /// The objects in `scope`, in displayId order, each with its metadata. A
    /// member with no metadata (an external reference) is returned with a default
    /// (empty) record, so it still yields a subject-only row. When a row limit is
    /// set, the scan is capped to `offset + limit` objects: every object yields at
    /// least one distinct row, so that many objects suffice to fill the page.
    async fn scope_candidates(
        &self,
        graph: &str,
        scope: &Scope,
        subject_prefix: Option<&str>,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<Vec<(String, MetaRecord)>, DomainError> {
        let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new("");
        match scope {
            Scope::TopLevel => {
                qb.push("SELECT iri, meta FROM accel_object WHERE graph_iri = ");
                qb.push_bind(graph.to_owned());
                qb.push(" AND top_level");
                if let Some(p) = subject_prefix {
                    qb.push(" AND iri LIKE ").push_bind(like_prefix(p));
                }
                qb.push(" ORDER BY sort_key COLLATE \"C\", iri COLLATE \"C\"");
            }
            Scope::ByType(t) => {
                qb.push(
                    "SELECT ty.iri AS iri, o.meta AS meta FROM accel_type ty \
                     LEFT JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph.to_owned());
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                if let Some(p) = subject_prefix {
                    qb.push(" AND ty.iri LIKE ").push_bind(like_prefix(p));
                }
                qb.push(" ORDER BY ty.sort_key COLLATE \"C\", ty.iri COLLATE \"C\"");
            }
            Scope::Collection {
                collection,
                root_only,
            } => {
                qb.push(
                    "SELECT m.member_iri AS iri, o.meta AS meta FROM accel_member m \
                     LEFT JOIN accel_object o \
                       ON o.graph_iri = m.graph_iri AND o.iri = m.member_iri \
                     WHERE m.graph_iri = ",
                );
                qb.push_bind(graph.to_owned());
                qb.push(" AND m.collection_iri = ")
                    .push_bind(collection.clone());
                if *root_only {
                    qb.push(" AND m.is_root");
                }
                if let Some(p) = subject_prefix {
                    qb.push(" AND m.member_iri LIKE ").push_bind(like_prefix(p));
                }
                qb.push(" ORDER BY m.sort_key COLLATE \"C\", m.member_iri COLLATE \"C\"");
            }
        }
        if let Some(l) = limit {
            qb.push(" LIMIT ").push_bind((offset + l) as i64);
        }

        let rows = qb.build().fetch_all(&self.pool).await.map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let iri: String = row.try_get("iri").map_err(db_err)?;
            let meta_json: Option<String> = row.try_get("meta").map_err(db_err)?;
            let meta = match meta_json {
                Some(j) => serde_json::from_str(&j).map_err(db_err)?,
                None => MetaRecord::default(),
            };
            out.push((iri, meta));
        }
        Ok(out)
    }

    /// Fetch one object's metadata projection by primary key. A missing or
    /// metadata-less object yields no rows (the required title cannot bind).
    async fn object_metadata(
        &self,
        graph: &str,
        subject: &str,
        projection: &[(String, Field)],
        required: &[bool],
    ) -> Result<AccelSolutions, DomainError> {
        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        let meta_json: Option<String> =
            sqlx::query_scalar("SELECT meta FROM accel_object WHERE graph_iri = $1 AND iri = $2")
                .bind(graph)
                .bind(subject)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        let mut rows = Vec::new();
        if let Some(json) = meta_json {
            let meta: MetaRecord = serde_json::from_str(&json).map_err(db_err)?;
            generate_metadata_rows(subject, &meta, projection, required, &mut rows);
        }
        Ok(AccelSolutions { vars, rows })
    }

    async fn count(
        &self,
        graph: &str,
        scope: &Scope,
        var: &str,
        subject_prefix: Option<&str>,
    ) -> Result<AccelSolutions, DomainError> {
        let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new("");
        match scope {
            Scope::TopLevel => {
                qb.push("SELECT COUNT(*) FROM accel_object WHERE graph_iri = ");
                qb.push_bind(graph.to_owned());
                qb.push(" AND top_level");
                if let Some(p) = subject_prefix {
                    qb.push(" AND iri LIKE ").push_bind(like_prefix(p));
                }
            }
            Scope::ByType(t) => {
                qb.push("SELECT COUNT(*) FROM accel_type WHERE graph_iri = ");
                qb.push_bind(graph.to_owned());
                qb.push(" AND type_iri = ").push_bind(t.clone());
                if let Some(p) = subject_prefix {
                    qb.push(" AND iri LIKE ").push_bind(like_prefix(p));
                }
            }
            Scope::Collection {
                collection,
                root_only,
            } => {
                qb.push("SELECT COUNT(*) FROM accel_member WHERE graph_iri = ");
                qb.push_bind(graph.to_owned());
                qb.push(" AND collection_iri = ")
                    .push_bind(collection.clone());
                if *root_only {
                    qb.push(" AND is_root");
                }
                if let Some(p) = subject_prefix {
                    qb.push(" AND member_iri LIKE ").push_bind(like_prefix(p));
                }
            }
        }
        let n: i64 = qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(AccelSolutions {
            vars: vec![var.to_owned()],
            rows: vec![vec![Some(integer(n as u64))]],
        })
    }

    async fn facet(
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
        let values = sqlx::query_scalar::<_, String>(
            "SELECT value FROM accel_facet WHERE graph_iri = $1 AND kind = $2 \
             ORDER BY value COLLATE \"C\"",
        )
        .bind(graph)
        .bind(tag)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let rows = values
            .into_iter()
            .map(|value| {
                let term = match kind {
                    FacetKind::Creators => TermValue::Literal {
                        value,
                        datatype: XSD_STRING.to_owned(),
                        language: None,
                    },
                    _ => TermValue::Iri(value),
                };
                vec![Some(term)]
            })
            .collect();
        Ok(AccelSolutions {
            vars: vec![var.to_owned()],
            rows,
        })
    }
}

/// A `LIKE` pattern matching everything that starts with `prefix`, escaping the
/// `LIKE` metacharacters (`%`, `_`, `\`) so an IRI prefix is matched literally.
fn like_prefix(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1);
    for ch in prefix.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}
