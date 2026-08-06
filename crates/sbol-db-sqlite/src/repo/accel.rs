//! The SynBioHub query accelerator for SQLite: per-graph derived indexes that
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
//!
//! SQLite's default `BINARY` text collation is byte order, matching the other
//! backends' enumeration order, so no explicit collation is needed.

use std::collections::{BTreeMap, HashMap, HashSet};

use sbol_db_core::DomainError;
use sbol_db_storage::{
    build_catalog_projection, generate_metadata_rows, generate_rows, integer,
    merge_resource_occurrences, AccelSolutions, AcceleratedQuery, CatalogSequenceRecord,
    CursorPage, FacetKind, Field, MetaRecord, ResourceOccurrence, ResourceQuery, ResourceRecord,
    Scope, SequenceQuery, TermValue,
};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

use crate::pool::db_err;
use crate::repo::triple::TripleRepository;

const FK_TYPES: i64 = 1;
const FK_ROLES: i64 = 2;
const FK_CREATORS: i64 = 3;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Max rows per multi-row `INSERT` so the bound-parameter count stays well under
/// SQLite's limit (the widest row binds five values).
const INSERT_CHUNK: usize = 100;

/// Derives, maintains, and queries the accelerator indexes for the SQLite
/// backend.
#[derive(Clone)]
pub struct AccelRepository {
    pool: SqlitePool,
    triples: TripleRepository,
}

impl AccelRepository {
    pub fn new(pool: SqlitePool, triples: TripleRepository) -> Self {
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
            AcceleratedQuery::FacetCounts {
                graph,
                kind,
                value_var,
                count_var,
            } => self.facet_counts(graph, *kind, value_var, count_var).await,
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

    pub async fn resource_occurrences(
        &self,
        iri: &str,
    ) -> Result<Vec<ResourceOccurrence>, DomainError> {
        let rows = sqlx::query(
            "SELECT graph_iri, meta FROM accel_object WHERE iri = ? ORDER BY graph_iri",
        )
        .bind(iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                let graph_iri: String = row.try_get("graph_iri").map_err(db_err)?;
                let meta: String = row.try_get("meta").map_err(db_err)?;
                Ok(ResourceOccurrence {
                    graph_iri,
                    resource_iri: iri.to_owned(),
                    meta: serde_json::from_str(&meta).map_err(db_err)?,
                })
            })
            .collect()
    }

    pub async fn resource(&self, iri: &str) -> Result<Option<ResourceRecord>, DomainError> {
        let occurrences = self.resource_occurrences(iri).await?;
        Ok(merge_resource_occurrences(iri, &occurrences))
    }

    pub async fn resources(
        &self,
        query: &ResourceQuery,
    ) -> Result<CursorPage<ResourceRecord>, DomainError> {
        let limit = query.limit.clamp(1, 500) as i64;
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT DISTINCT o.iri FROM accel_object o WHERE TRUE");
        if let Some(after) = &query.after {
            qb.push(" AND o.iri > ").push_bind(after.clone());
        }
        if let Some(graph) = &query.graph_iri {
            qb.push(" AND o.graph_iri = ").push_bind(graph.clone());
        }
        if let Some(class) = &query.class {
            qb.push(
                " AND EXISTS (SELECT 1 FROM accel_type ty \
                 WHERE ty.graph_iri = o.graph_iri AND ty.iri = o.iri AND ty.type_iri = ",
            )
            .push_bind(class.clone())
            .push(")");
        }
        if let Some(role) = &query.role {
            qb.push(
                " AND EXISTS (SELECT 1 FROM accel_role r \
                 WHERE r.graph_iri = o.graph_iri AND r.iri = o.iri AND r.role_iri = ",
            )
            .push_bind(role.clone())
            .push(")");
        }
        if let Some(text) = query.text.as_deref().filter(|text| !text.is_empty()) {
            let needle = format!("%{}%", text.to_lowercase());
            qb.push(" AND (lower(o.iri) LIKE ")
                .push_bind(needle.clone())
                .push(" OR lower(o.meta) LIKE ")
                .push_bind(needle)
                .push(")");
        }
        qb.push(" ORDER BY o.iri LIMIT ").push_bind(limit + 1);
        let mut iris: Vec<String> = qb
            .build_query_scalar()
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let has_more = iris.len() > limit as usize;
        iris.truncate(limit as usize);
        let next_cursor = has_more.then(|| iris.last().cloned()).flatten();
        let items = self.resources_for_iris(&iris).await?;
        Ok(CursorPage { items, next_cursor })
    }

    pub async fn resources_for_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<ResourceRecord>, DomainError> {
        if iris.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT iri, graph_iri, meta FROM accel_object WHERE iri IN (");
        {
            let mut values = qb.separated(", ");
            for iri in iris {
                values.push_bind(iri);
            }
        }
        qb.push(") ORDER BY iri, graph_iri");
        let rows = qb.build().fetch_all(&self.pool).await.map_err(db_err)?;
        let mut by_iri: BTreeMap<String, Vec<ResourceOccurrence>> = BTreeMap::new();
        for row in rows {
            let iri: String = row.try_get("iri").map_err(db_err)?;
            let graph_iri: String = row.try_get("graph_iri").map_err(db_err)?;
            let meta: String = row.try_get("meta").map_err(db_err)?;
            by_iri
                .entry(iri.clone())
                .or_default()
                .push(ResourceOccurrence {
                    graph_iri,
                    resource_iri: iri,
                    meta: serde_json::from_str(&meta).map_err(db_err)?,
                });
        }
        Ok(iris
            .iter()
            .filter_map(|iri| {
                by_iri
                    .get(iri)
                    .and_then(|occurrences| merge_resource_occurrences(iri, occurrences))
            })
            .collect())
    }

    pub async fn sequence(&self, iri: &str) -> Result<Option<CatalogSequenceRecord>, DomainError> {
        let graph_count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT graph_iri) FROM accel_type WHERE iri = ? \
             AND type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')",
        )
        .bind(iri)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        if graph_count == 0 {
            return Ok(None);
        }
        let rows = sqlx::query(
            r#"
            SELECT predicate_iri, object_iri, object_literal
            FROM sbol_triples
            WHERE subject_iri = ?
              AND predicate_iri IN (
                'http://sbols.org/v2#elements', 'http://sbols.org/v3#elements',
                'http://sbols.org/v2#encoding', 'http://sbols.org/v3#encoding'
              )
            ORDER BY graph_iri, id
            "#,
        )
        .bind(iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut elements = None;
        let mut encoding_iri = None;
        for row in rows {
            let predicate: String = row.try_get("predicate_iri").map_err(db_err)?;
            if predicate.ends_with("#elements") && elements.is_none() {
                elements = row.try_get("object_literal").map_err(db_err)?;
            } else if predicate.ends_with("#encoding") && encoding_iri.is_none() {
                encoding_iri = row.try_get("object_iri").map_err(db_err)?;
            }
        }
        Ok(Some(CatalogSequenceRecord {
            iri: iri.to_owned(),
            graph_count: graph_count as u64,
            alphabet: catalog_alphabet(encoding_iri.as_deref()),
            encoding_iri,
            elements,
        }))
    }

    pub async fn sequences(
        &self,
        query: &SequenceQuery,
    ) -> Result<CursorPage<CatalogSequenceRecord>, DomainError> {
        let limit = query.limit.clamp(1, 500) as i64;
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT DISTINCT ty.iri FROM accel_type ty WHERE ty.type_iri IN \
             ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')",
        );
        if let Some(after) = &query.after {
            qb.push(" AND ty.iri > ").push_bind(after.clone());
        }
        if let Some(text) = query.text.as_deref().filter(|value| !value.is_empty()) {
            let needle = format!("%{}%", text.to_lowercase());
            qb.push(
                " AND EXISTS (SELECT 1 FROM accel_object o WHERE o.iri = ty.iri \
                     AND (lower(o.iri) LIKE ",
            )
            .push_bind(needle.clone())
            .push(" OR lower(o.meta) LIKE ")
            .push_bind(needle)
            .push("))");
        }
        qb.push(" ORDER BY ty.iri LIMIT ").push_bind(limit + 1);
        let mut iris: Vec<String> = qb
            .build_query_scalar()
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let has_more = iris.len() > limit as usize;
        iris.truncate(limit as usize);
        let next_cursor = has_more.then(|| iris.last().cloned()).flatten();
        let items = self.sequences_for_iris(&iris).await?;
        Ok(CursorPage { items, next_cursor })
    }

    async fn sequences_for_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<CatalogSequenceRecord>, DomainError> {
        if iris.is_empty() {
            return Ok(Vec::new());
        }
        let mut count_qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT iri, count(DISTINCT graph_iri) AS graph_count FROM accel_type \
             WHERE type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence') \
             AND iri IN (",
        );
        {
            let mut values = count_qb.separated(", ");
            for iri in iris {
                values.push_bind(iri);
            }
        }
        count_qb.push(") GROUP BY iri");
        let count_rows = count_qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut records: HashMap<String, (u64, Option<String>, Option<String>)> = HashMap::new();
        for row in count_rows {
            let iri: String = row.try_get("iri").map_err(db_err)?;
            let graph_count: i64 = row.try_get("graph_count").map_err(db_err)?;
            records.insert(iri, (graph_count as u64, None, None));
        }

        let mut value_qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT subject_iri, predicate_iri, object_iri, object_literal \
             FROM sbol_triples WHERE predicate_iri IN (\
               'http://sbols.org/v2#elements', 'http://sbols.org/v3#elements', \
               'http://sbols.org/v2#encoding', 'http://sbols.org/v3#encoding'\
             ) AND subject_iri IN (",
        );
        {
            let mut values = value_qb.separated(", ");
            for iri in iris {
                values.push_bind(iri);
            }
        }
        value_qb.push(") ORDER BY subject_iri, graph_iri, id");
        let value_rows = value_qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        for row in value_rows {
            let iri: String = row.try_get("subject_iri").map_err(db_err)?;
            let predicate: String = row.try_get("predicate_iri").map_err(db_err)?;
            let Some((_, elements, encoding)) = records.get_mut(&iri) else {
                continue;
            };
            if predicate.ends_with("#elements") && elements.is_none() {
                *elements = row.try_get("object_literal").map_err(db_err)?;
            } else if predicate.ends_with("#encoding") && encoding.is_none() {
                *encoding = row.try_get("object_iri").map_err(db_err)?;
            }
        }

        Ok(iris
            .iter()
            .filter_map(|iri| {
                records
                    .remove(iri)
                    .map(
                        |(graph_count, elements, encoding_iri)| CatalogSequenceRecord {
                            iri: iri.clone(),
                            graph_count,
                            alphabet: catalog_alphabet(encoding_iri.as_deref()),
                            encoding_iri,
                            elements,
                        },
                    )
            })
            .collect())
    }

    /// Rebuild a graph's accelerator indexes from its triples, inside the
    /// caller's write transaction (atomic with the triple write). The triple
    /// scan runs through `conn`, so it sees the triples the same transaction
    /// just wrote; the rebuilt indexes are deleted and reinserted on `conn` and
    /// commit together with the triples. Callers invoke this after every write
    /// that changes a graph's triples.
    pub async fn refresh_graph(
        &self,
        conn: &mut SqliteConnection,
        graph: &str,
    ) -> Result<(), DomainError> {
        let triples = self.triples.scan_graph_in_conn(conn, graph).await?;
        let index = build_catalog_projection(&triples);

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

        // (iri, sort, top_level, meta)
        let mut objects: Vec<(String, String, bool, String)> = Vec::new();
        // (type_iri, iri, sort)
        let mut types: Vec<(String, String, String)> = Vec::new();
        // (role_iri, iri, sort)
        let mut roles: Vec<(String, String, String)> = Vec::new();
        let mut facet_counts: HashMap<(i64, String), i64> = HashMap::new();
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
            for role in &m.roles {
                roles.push((role.clone(), iri.to_owned(), sort.to_owned()));
            }
            if m.top_level {
                let mut object_facets: HashSet<(i64, &str)> = HashSet::new();
                for t in &m.types {
                    object_facets.insert((FK_TYPES, t.as_str()));
                }
                for r in &m.roles {
                    object_facets.insert((FK_ROLES, r.as_str()));
                }
                for c in &m.creators {
                    object_facets.insert((FK_CREATORS, c.as_str()));
                }
                for (kind, value) in object_facets {
                    *facet_counts.entry((kind, value.to_owned())).or_default() += 1;
                }
            }
        }
        // (kind, value, exact distinct-subject count)
        let mut facets: Vec<(i64, String, i64)> = facet_counts
            .into_iter()
            .map(|((kind, value), count)| (kind, value, count))
            .collect();
        facets.sort();

        // (collection, member, sort, is_root)
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

        for table in [
            "accel_object",
            "accel_type",
            "accel_role",
            "accel_member",
            "accel_facet",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE graph_iri = ?"))
                .bind(graph)
                .execute(&mut *conn)
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
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
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
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
        }
        for chunk in roles.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT OR IGNORE INTO accel_role (graph_iri, role_iri, iri, sort_key) ",
            );
            qb.push_values(chunk, |mut b, (role_iri, iri, sort)| {
                b.push_bind(graph)
                    .push_bind(role_iri)
                    .push_bind(iri)
                    .push_bind(sort);
            });
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
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
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
        }
        for chunk in facets.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT OR IGNORE INTO accel_facet \
                 (graph_iri, kind, value, subject_count) ",
            );
            qb.push_values(chunk, |mut b, (kind, value, count)| {
                b.push_bind(graph)
                    .push_bind(*kind)
                    .push_bind(value)
                    .push_bind(*count);
            });
            qb.build().execute(&mut *conn).await.map_err(db_err)?;
        }
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
            Scope::TopLevelByType(t) => {
                qb.push(
                    "SELECT ty.iri AS iri, o.meta AS meta FROM accel_type ty \
                     JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
                qb.push(" ORDER BY ty.sort_key, ty.iri");
            }
            Scope::TopLevelByRole(role) => {
                qb.push(
                    "SELECT r.iri AS iri, o.meta AS meta FROM accel_role r \
                     JOIN accel_object o \
                       ON o.graph_iri = r.graph_iri AND o.iri = r.iri \
                     WHERE r.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND r.role_iri = ").push_bind(role.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "r.iri", subject_prefix);
                qb.push(" ORDER BY r.sort_key, r.iri");
            }
            Scope::TopLevelByTypeAndRole { object_type, role } => {
                qb.push(
                    "SELECT r.iri AS iri, o.meta AS meta FROM accel_role r \
                     JOIN accel_type ty \
                       ON ty.graph_iri = r.graph_iri AND ty.iri = r.iri \
                     JOIN accel_object o \
                       ON o.graph_iri = r.graph_iri AND o.iri = r.iri \
                     WHERE r.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND r.role_iri = ").push_bind(role.clone());
                qb.push(" AND ty.type_iri = ")
                    .push_bind(object_type.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "r.iri", subject_prefix);
                qb.push(" ORDER BY r.sort_key, r.iri");
            }
            Scope::RootByType(t) => {
                qb.push(
                    "SELECT ty.iri AS iri, o.meta AS meta FROM accel_type ty \
                     LEFT JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(
                    " AND NOT EXISTS (SELECT 1 FROM accel_member m \
                       WHERE m.graph_iri = ty.graph_iri AND m.member_iri = ty.iri)",
                );
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
                qb.push(" ORDER BY ty.sort_key, ty.iri");
            }
            Scope::RootTopLevelByType(t) => {
                qb.push(
                    "SELECT ty.iri AS iri, o.meta AS meta FROM accel_type ty \
                     JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(" AND o.top_level");
                qb.push(
                    " AND NOT EXISTS (SELECT 1 FROM accel_member m \
                       WHERE m.graph_iri = ty.graph_iri AND m.member_iri = ty.iri)",
                );
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
            Scope::TopLevelByType(t) => {
                qb.push(
                    "SELECT COUNT(*) FROM accel_type ty \
                     JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
            }
            Scope::TopLevelByRole(role) => {
                qb.push(
                    "SELECT COUNT(*) FROM accel_role r \
                     JOIN accel_object o \
                       ON o.graph_iri = r.graph_iri AND o.iri = r.iri \
                     WHERE r.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND r.role_iri = ").push_bind(role.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "r.iri", subject_prefix);
            }
            Scope::TopLevelByTypeAndRole { object_type, role } => {
                qb.push(
                    "SELECT COUNT(*) FROM accel_role r \
                     JOIN accel_type ty \
                       ON ty.graph_iri = r.graph_iri AND ty.iri = r.iri \
                     JOIN accel_object o \
                       ON o.graph_iri = r.graph_iri AND o.iri = r.iri \
                     WHERE r.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND r.role_iri = ").push_bind(role.clone());
                qb.push(" AND ty.type_iri = ")
                    .push_bind(object_type.clone());
                qb.push(" AND o.top_level");
                push_prefix_filter(&mut qb, "r.iri", subject_prefix);
            }
            Scope::RootByType(t) => {
                qb.push("SELECT COUNT(*) FROM accel_type ty WHERE ty.graph_iri = ");
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(
                    " AND NOT EXISTS (SELECT 1 FROM accel_member m \
                       WHERE m.graph_iri = ty.graph_iri AND m.member_iri = ty.iri)",
                );
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
            }
            Scope::RootTopLevelByType(t) => {
                qb.push(
                    "SELECT COUNT(*) FROM accel_type ty \
                     JOIN accel_object o \
                       ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri \
                     WHERE ty.graph_iri = ",
                );
                qb.push_bind(graph);
                qb.push(" AND ty.type_iri = ").push_bind(t.clone());
                qb.push(" AND o.top_level");
                qb.push(
                    " AND NOT EXISTS (SELECT 1 FROM accel_member m \
                       WHERE m.graph_iri = ty.graph_iri AND m.member_iri = ty.iri)",
                );
                push_prefix_filter(&mut qb, "ty.iri", subject_prefix);
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

    async fn facet_counts(
        &self,
        graph: &str,
        kind: FacetKind,
        value_var: &str,
        count_var: &str,
    ) -> Result<AccelSolutions, DomainError> {
        let (tag, iri_value) = match kind {
            FacetKind::Types => (FK_TYPES, true),
            FacetKind::Roles => (FK_ROLES, true),
            FacetKind::Creators => (FK_CREATORS, false),
        };
        let rows = sqlx::query(
            "SELECT value, subject_count FROM accel_facet \
             WHERE graph_iri = ? AND kind = ? ORDER BY value",
        )
        .bind(graph)
        .bind(tag)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let rows = rows
            .into_iter()
            .map(|row| {
                let value: String = row.try_get("value").map_err(db_err)?;
                let count: i64 = row.try_get("subject_count").map_err(db_err)?;
                let value = if iri_value {
                    TermValue::Iri(value)
                } else {
                    TermValue::Literal {
                        value,
                        datatype: XSD_STRING.to_owned(),
                        language: None,
                    }
                };
                Ok(vec![Some(value), Some(integer(count as u64))])
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        Ok(AccelSolutions {
            vars: vec![value_var.to_owned(), count_var.to_owned()],
            rows,
        })
    }
}

/// Append a case-sensitive `column` starts-with `prefix` filter. `substr`
/// compares the leading characters under `BINARY` collation, so it matches
/// byte-for-byte regardless of `LIKE`/`GLOB` metacharacters in the prefix.
fn push_prefix_filter(qb: &mut QueryBuilder<Sqlite>, column: &str, prefix: Option<&str>) {
    if let Some(p) = prefix {
        qb.push(format!(" AND substr({column}, 1, length("));
        qb.push_bind(p.to_owned());
        qb.push(")) = ");
        qb.push_bind(p.to_owned());
    }
}

fn catalog_alphabet(encoding: Option<&str>) -> Option<String> {
    let encoding = encoding?.to_ascii_lowercase();
    Some(
        if encoding.contains("protein") || encoding.contains("amino") {
            "PROTEIN"
        } else if encoding.contains("rna") {
            "RNA"
        } else if encoding.contains("dna")
            || encoding.contains("naseq")
            || encoding.contains("1207")
        {
            "DNA"
        } else {
            "OTHER"
        }
        .to_owned(),
    )
}
