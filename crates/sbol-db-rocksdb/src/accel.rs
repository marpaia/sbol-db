//! The SynBioHub query accelerator for the Oxigraph backend: per-graph derived
//! indexes that answer the fixed SynBioHub query templates with point lookups
//! and range scans instead of native SPARQL evaluation.
//!
//! The physical index lives in the SQLite companion (the same `accel_*` tables
//! the SQLite backend uses), but the indexes are derived from the Oxigraph
//! triples, not from SQLite. A write marks the graph dirty (in `accel_dirty`),
//! and the next read that needs the indexes rebuilds them in one pass: it clears
//! the dirty flag before scanning Oxigraph, so a write that lands during a
//! rebuild re-marks the graph and the next read rebuilds again (never serving
//! stale data). Rebuilds replace the graph's rows wholesale, so a concurrent
//! rebuild is at worst redundant.

use std::collections::{HashMap, HashSet};

use oxigraph::store::Store;
use sbol_db_core::{DomainError, Triple};
use sbol_db_sqlite::SqlitePool;
use sbol_db_storage::{
    build_accel_index, generate_metadata_rows, generate_rows, integer, AccelSolutions,
    AcceleratedQuery, FacetKind, Field, MetaRecord, Scope, TermValue,
};
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::convert::quad_to_triple;
use crate::db_err;

const FK_TYPES: i64 = 1;
const FK_ROLES: i64 = 2;
const FK_CREATORS: i64 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Max rows per multi-row `INSERT` so the bound-parameter count stays under
/// SQLite's limit (the widest row binds five values).
const INSERT_CHUNK: usize = 100;

/// Derives, maintains, and queries the accelerator indexes. Storage is the
/// SQLite companion; derivation scans the Oxigraph triples.
#[derive(Clone)]
pub struct AccelRepository {
    pool: SqlitePool,
    store: Store,
}

impl AccelRepository {
    pub fn new(pool: SqlitePool, store: Store) -> Self {
        Self { pool, store }
    }

    /// Mark a graph stale directly on the pool (no enclosing transaction), for
    /// the SPARQL-update writer whose triple write committed in Oxigraph.
    pub async fn mark_dirty_pool(&self, graph: &str) -> Result<(), DomainError> {
        sqlx::query("INSERT OR IGNORE INTO accel_dirty (graph_iri) VALUES (?)")
            .bind(graph)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Answer a recognized query, rebuilding the graph's indexes first if stale.
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
                self.ensure_fresh(graph).await?;
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
                self.ensure_fresh(graph).await?;
                self.count(graph, scope, var, subject_prefix.as_deref())
                    .await
            }
            AcceleratedQuery::Facet { graph, kind, var } => {
                self.ensure_fresh(graph).await?;
                self.facet(graph, *kind, var).await
            }
            AcceleratedQuery::ObjectMetadata {
                graph,
                subject,
                projection,
                required,
            } => {
                self.ensure_fresh(graph).await?;
                self.object_metadata(graph, subject, projection, required)
                    .await
            }
        }
    }

    async fn ensure_fresh(&self, graph: &str) -> Result<(), DomainError> {
        if !self.is_dirty(graph).await? {
            return Ok(());
        }
        sqlx::query("DELETE FROM accel_dirty WHERE graph_iri = ?")
            .bind(graph)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        self.rebuild(graph).await
    }

    async fn is_dirty(&self, graph: &str) -> Result<bool, DomainError> {
        let row = sqlx::query_scalar::<_, i64>("SELECT 1 FROM accel_dirty WHERE graph_iri = ?")
            .bind(graph)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row.is_some())
    }

    /// Scan every triple in `graph` from Oxigraph.
    fn graph_triples(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        let g = oxrdf::NamedNode::new_unchecked(graph);
        let mut out = Vec::new();
        for quad in self.store.quads_for_pattern(
            None,
            None,
            None,
            Some(oxrdf::GraphNameRef::NamedNode(g.as_ref())),
        ) {
            let quad =
                quad.map_err(|e| DomainError::Database(format!("oxigraph accel scan: {e}")))?;
            out.push(quad_to_triple(&quad)?);
        }
        Ok(out)
    }

    async fn rebuild(&self, graph: &str) -> Result<(), DomainError> {
        let triples = self.graph_triples(graph)?;
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

        let mut objects: Vec<(String, String, bool, String)> = Vec::new();
        let mut types: Vec<(String, String, String)> = Vec::new();
        let mut facets: Vec<(i64, String)> = Vec::new();
        let mut facet_seen: HashSet<(i64, &str)> = HashSet::new();
        for obj in &index.objects {
            let iri = obj.iri.as_str();
            let m = &obj.meta;
            let sort = m.sort_key();
            objects.push((
                iri.to_owned(),
                sort.to_owned(),
                m.top_level,
                serde_json::to_string(m).map_err(db_err)?,
            ));
            for t in &m.types {
                types.push((t.clone(), iri.to_owned(), sort.to_owned()));
            }
            if m.top_level {
                for t in &m.types {
                    if facet_seen.insert((FK_TYPES, t.as_str())) {
                        facets.push((FK_TYPES, t.clone()));
                    }
                }
                for r in &m.roles {
                    if facet_seen.insert((FK_ROLES, r.as_str())) {
                        facets.push((FK_ROLES, r.clone()));
                    }
                }
                for c in &m.creators {
                    if facet_seen.insert((FK_CREATORS, c.as_str())) {
                        facets.push((FK_CREATORS, c.clone()));
                    }
                }
            }
        }

        let mut members: Vec<(String, String, String, bool)> = Vec::new();
        for (collection, member) in &index.members {
            let sort = sort_of.get(member.as_str()).copied().unwrap_or("");
            members.push((
                collection.clone(),
                member.clone(),
                sort.to_owned(),
                root_set.contains(&(collection.as_str(), member.as_str())),
            ));
        }

        let mut tx = self.pool.begin().await.map_err(db_err)?;
        for table in ["accel_object", "accel_type", "accel_member", "accel_facet"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE graph_iri = ?"))
                .bind(graph)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        for chunk in objects.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT OR IGNORE INTO accel_object (graph_iri, iri, sort_key, top_level, meta) ",
            );
            qb.push_values(chunk, |mut b, (iri, sort, top, meta)| {
                b.push_bind(graph)
                    .push_bind(iri)
                    .push_bind(sort)
                    .push_bind(*top)
                    .push_bind(meta);
            });
            qb.build().execute(&mut *tx).await.map_err(db_err)?;
        }
        for chunk in types.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT OR IGNORE INTO accel_type (graph_iri, type_iri, iri, sort_key) ",
            );
            qb.push_values(chunk, |mut b, (type_iri, iri, sort)| {
                b.push_bind(graph)
                    .push_bind(type_iri)
                    .push_bind(iri)
                    .push_bind(sort);
            });
            qb.build().execute(&mut *tx).await.map_err(db_err)?;
        }
        for chunk in members.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT OR IGNORE INTO accel_member \
                 (graph_iri, collection_iri, member_iri, sort_key, is_root) ",
            );
            qb.push_values(chunk, |mut b, (collection, member, sort, root)| {
                b.push_bind(graph)
                    .push_bind(collection)
                    .push_bind(member)
                    .push_bind(sort)
                    .push_bind(*root);
            });
            qb.build().execute(&mut *tx).await.map_err(db_err)?;
        }
        for chunk in facets.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("INSERT OR IGNORE INTO accel_facet (graph_iri, kind, value) ");
            qb.push_values(chunk, |mut b, (kind, value)| {
                b.push_bind(graph).push_bind(*kind).push_bind(value);
            });
            qb.build().execute(&mut *tx).await.map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn object_list(
        &self,
        graph: &str,
        scope: &Scope,
        projection: &[(String, Field)],
        offset: usize,
        limit: Option<usize>,
        subject_prefix: Option<&str>,
    ) -> Result<AccelSolutions, DomainError> {
        let candidates = self
            .scope_candidates(graph, scope, subject_prefix, offset, limit)
            .await?;

        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
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

    async fn scope_candidates(
        &self,
        graph: &str,
        scope: &Scope,
        subject_prefix: Option<&str>,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<Vec<(String, MetaRecord)>, DomainError> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("");
        match scope {
            Scope::TopLevel => {
                qb.push("SELECT iri, meta FROM accel_object WHERE graph_iri = ");
                qb.push_bind(graph);
                qb.push(" AND top_level");
                push_prefix_filter(&mut qb, "iri", subject_prefix);
                qb.push(" ORDER BY sort_key, iri");
            }
            Scope::ByType(t) => {
                qb.push(
                    "SELECT ty.iri AS iri, o.meta AS meta FROM accel_type ty \
                     LEFT JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
                qb.push(" ORDER BY ty.sort_key, ty.iri");
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
                qb.push_bind(graph);
                qb.push(" AND m.collection_iri = ")
                    .push_bind(collection.clone());
                if *root_only {
                    qb.push(" AND m.is_root");
                }
                push_prefix_filter(&mut qb, "m.member_iri", subject_prefix);
                qb.push(" ORDER BY m.sort_key, m.member_iri");
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

    async fn object_metadata(
        &self,
        graph: &str,
        subject: &str,
        projection: &[(String, Field)],
        required: &[bool],
    ) -> Result<AccelSolutions, DomainError> {
        let vars: Vec<String> = projection.iter().map(|(v, _)| v.clone()).collect();
        let meta_json: Option<String> =
            sqlx::query_scalar("SELECT meta FROM accel_object WHERE graph_iri = ? AND iri = ?")
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
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("");
        match scope {
            Scope::TopLevel => {
                qb.push("SELECT COUNT(*) FROM accel_object WHERE graph_iri = ");
                qb.push_bind(graph);
                qb.push(" AND top_level");
                push_prefix_filter(&mut qb, "iri", subject_prefix);
            }
            Scope::ByType(t) => {
                qb.push("SELECT COUNT(*) FROM accel_type WHERE graph_iri = ");
                qb.push_bind(graph);
                qb.push(" AND type_iri = ").push_bind(t.clone());
                push_prefix_filter(&mut qb, "iri", subject_prefix);
            }
            Scope::Collection {
                collection,
                root_only,
            } => {
                qb.push("SELECT COUNT(*) FROM accel_member WHERE graph_iri = ");
                qb.push_bind(graph);
                qb.push(" AND collection_iri = ")
                    .push_bind(collection.clone());
                if *root_only {
                    qb.push(" AND is_root");
                }
                push_prefix_filter(&mut qb, "member_iri", subject_prefix);
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
            "SELECT value FROM accel_facet WHERE graph_iri = ? AND kind = ? ORDER BY value",
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

fn push_prefix_filter(qb: &mut QueryBuilder<Sqlite>, column: &str, prefix: Option<&str>) {
    if let Some(p) = prefix {
        qb.push(format!(" AND substr({column}, 1, length("));
        qb.push_bind(p.to_owned());
        qb.push(")) = ");
        qb.push_bind(p.to_owned());
    }
}
