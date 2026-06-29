//! Ontology storage + transitive-closure builder, plus a small HTTP loader
//! that fetches an OBO file from a canonical URL (OBO Foundry / EBI) and
//! materialises it into `sbol_ontologies` / `sbol_ontology_terms` /
//! `sbol_ontology_term_aliases` / `sbol_ontology_closure`.
//!
//! The closure is computed in Rust at load time -- one BFS per term up
//! through `is_a` parents -- so role-expansion queries reduce to one
//! indexed lookup against `sbol_ontology_closure.ancestor_iri`.
//!
//! IRI canonicalisation: every term gets its OBO Foundry PURL
//! (`http://purl.obolibrary.org/obo/{PREFIX}_{NUMBER}`) as its canonical
//! IRI plus an identifiers.org alias (`http://identifiers.org/{prefix}/{CURIE}`)
//! since SBOL documents commonly use the latter.

use sbol_db_core::DomainError;
use sbol_db_derive::build_ontology_plan;
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

use sbol_db_storage::{OntologyLoadReport, OntologyRecord, OntologyTermRecord};

#[derive(Clone)]
pub struct OntologyRepository {
    pool: PgPool,
}

impl OntologyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch an OBO file from `source_url`, parse it, and persist the
    /// ontology + terms + closure in a single transaction. `prefix` (e.g.
    /// "SO", "SBO") drives IRI generation and is the row key in
    /// `sbol_ontologies`.
    pub async fn load_from_url(
        &self,
        prefix: &str,
        name: &str,
        source_url: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let client = reqwest::Client::builder()
            .user_agent("sbol-db/0.1 (+https://github.com/marpaia/sbol-db)")
            .build()
            .map_err(|e| DomainError::InvalidInput(format!("reqwest client: {e}")))?;
        let body = client
            .get(source_url)
            .send()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("fetch {source_url}: {e}")))?
            .error_for_status()
            .map_err(|e| DomainError::InvalidInput(format!("HTTP {source_url}: {e}")))?
            .text()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("decode {source_url}: {e}")))?;
        self.load_from_text(prefix, name, Some(source_url), &body)
            .await
    }

    /// Same as [`load_from_url`] but takes already-fetched OBO text. The OBO
    /// parse, canonical-IRI/alias generation, and closure are derived by the
    /// shared [`build_ontology_plan`]; this method only persists the plan.
    pub async fn load_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let plan = build_ontology_plan(prefix, name, source_url, text);

        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // Replace any previous ontology of this prefix.
        sqlx::query("DELETE FROM sbol_ontologies WHERE prefix = $1")
            .bind(&plan.prefix)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        sqlx::query(
            r#"
            INSERT INTO sbol_ontologies (prefix, name, source_url, version, term_count, imported_at)
            VALUES ($1, $2, $3, $4, $5, now())
            "#,
        )
        .bind(&plan.prefix)
        .bind(&plan.name)
        .bind(plan.source_url.as_deref())
        .bind(&plan.version)
        .bind(plan.terms.len() as i32)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        // Per-term insert. Synonyms are text[]; Postgres rejects ragged
        // multidim arrays, so we cannot UNNEST text[][] safely -- and an
        // OBO file is a few thousand rows, well within row-by-row insert
        // throughput inside one transaction.
        for t in &plan.terms {
            sqlx::query(
                r#"
                INSERT INTO sbol_ontology_terms
                    (iri, prefix, curie, name, definition, is_obsolete, synonyms)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(&t.canonical_iri)
            .bind(&plan.prefix)
            .bind(&t.curie)
            .bind(&t.name)
            .bind(&t.definition)
            .bind(t.is_obsolete)
            .bind(&t.synonyms)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        if !plan.aliases.is_empty() {
            let alias_iri: Vec<String> = plan.aliases.iter().map(|(a, _)| a.clone()).collect();
            let alias_canon: Vec<String> = plan.aliases.iter().map(|(_, c)| c.clone()).collect();
            sqlx::query(
                r#"
                INSERT INTO sbol_ontology_term_aliases (alias_iri, canonical_iri)
                SELECT alias, canonical
                FROM UNNEST($1::text[], $2::text[]) AS u(alias, canonical)
                ON CONFLICT (alias_iri) DO UPDATE
                  SET canonical_iri = EXCLUDED.canonical_iri
                "#,
            )
            .bind(&alias_iri)
            .bind(&alias_canon)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        if !plan.closure.is_empty() {
            let mut anc: Vec<String> = Vec::with_capacity(plan.closure.len());
            let mut des: Vec<String> = Vec::with_capacity(plan.closure.len());
            let mut dep: Vec<i16> = Vec::with_capacity(plan.closure.len());
            for (a, d, depth) in &plan.closure {
                anc.push(a.clone());
                des.push(d.clone());
                dep.push(*depth);
            }
            sqlx::query(
                r#"
                INSERT INTO sbol_ontology_closure (ancestor_iri, descendant_iri, depth)
                SELECT ancestor, descendant, depth
                FROM UNNEST($1::text[], $2::text[], $3::int2[])
                  AS u(ancestor, descendant, depth)
                "#,
            )
            .bind(&anc)
            .bind(&des)
            .bind(&dep)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(plan.report())
    }

    pub async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT prefix, name, source_url, version, term_count, imported_at
            FROM sbol_ontologies
            ORDER BY prefix
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(OntologyRecord {
                prefix: row.try_get("prefix").map_err(db_err)?,
                name: row.try_get("name").map_err(db_err)?,
                source_url: row.try_get("source_url").map_err(db_err)?,
                version: row.try_get("version").map_err(db_err)?,
                term_count: row.try_get("term_count").map_err(db_err)?,
                imported_at: row.try_get("imported_at").map_err(db_err)?,
            });
        }
        Ok(out)
    }

    /// Resolve any input IRI (canonical or alias) to a canonical term IRI.
    pub async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT iri::text AS canonical
            FROM sbol_ontology_terms WHERE iri = $1
            UNION ALL
            SELECT canonical_iri::text AS canonical
            FROM sbol_ontology_term_aliases WHERE alias_iri = $1
            LIMIT 1
            "#,
        )
        .bind(iri)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(match row {
            Some(r) => Some(r.try_get("canonical").map_err(db_err)?),
            None => None,
        })
    }

    /// Return the term plus every descendant IRI under it (transitively).
    /// The root itself is included with depth 0.
    pub async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let rows = sqlx::query(
            r#"
            SELECT descendant_iri::text AS iri, depth
            FROM sbol_ontology_closure
            WHERE ancestor_iri = $1
            ORDER BY depth ASC, descendant_iri ASC
            "#,
        )
        .bind(&canonical)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push((
                row.try_get::<String, _>("iri").map_err(db_err)?,
                row.try_get::<i16, _>("depth").map_err(db_err)?,
            ));
        }
        Ok(out)
    }

    /// Page through every term that belongs to `prefix`, sorted by
    /// curie. `search` (when non-empty) restricts by case-insensitive
    /// substring match on either the curie or the name. Returns the
    /// page rows plus the total matching count so that callers can render
    /// a paginated UI without a second round-trip.
    pub async fn list_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        let prefix_upper = prefix.to_ascii_uppercase();
        let pattern = search.map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| {
            format!(
                "%{}%",
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });

        let total: i64 = if let Some(pat) = pattern.as_deref() {
            sqlx::query_scalar(
                r#"
                SELECT COUNT(*)::bigint
                FROM sbol_ontology_terms
                WHERE prefix = $1
                  AND (curie ILIKE $2 OR name ILIKE $2)
                "#,
            )
            .bind(&prefix_upper)
            .bind(pat)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?
        } else {
            sqlx::query_scalar("SELECT COUNT(*)::bigint FROM sbol_ontology_terms WHERE prefix = $1")
                .bind(&prefix_upper)
                .fetch_one(&self.pool)
                .await
                .map_err(db_err)?
        };

        let rows = if let Some(pat) = pattern.as_deref() {
            sqlx::query(
                r#"
                SELECT iri::text AS iri, prefix, curie, name, definition,
                       is_obsolete, synonyms
                FROM sbol_ontology_terms
                WHERE prefix = $1
                  AND (curie ILIKE $2 OR name ILIKE $2)
                ORDER BY curie ASC
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(&prefix_upper)
            .bind(pat)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
        } else {
            sqlx::query(
                r#"
                SELECT iri::text AS iri, prefix, curie, name, definition,
                       is_obsolete, synonyms
                FROM sbol_ontology_terms
                WHERE prefix = $1
                ORDER BY curie ASC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(&prefix_upper)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
        };

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(OntologyTermRecord {
                iri: row.try_get("iri").map_err(db_err)?,
                prefix: row.try_get("prefix").map_err(db_err)?,
                curie: row.try_get("curie").map_err(db_err)?,
                name: row.try_get("name").map_err(db_err)?,
                definition: row.try_get("definition").map_err(db_err)?,
                is_obsolete: row.try_get("is_obsolete").map_err(db_err)?,
                synonyms: row.try_get("synonyms").map_err(db_err)?,
            });
        }
        Ok((out, total))
    }

    pub async fn get_term(&self, iri: &str) -> Result<Option<OntologyTermRecord>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let row = sqlx::query(
            r#"
            SELECT iri::text AS iri, prefix, curie, name, definition,
                   is_obsolete, synonyms
            FROM sbol_ontology_terms WHERE iri = $1
            "#,
        )
        .bind(&canonical)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(match row {
            Some(r) => Some(OntologyTermRecord {
                iri: r.try_get("iri").map_err(db_err)?,
                prefix: r.try_get("prefix").map_err(db_err)?,
                curie: r.try_get("curie").map_err(db_err)?,
                name: r.try_get("name").map_err(db_err)?,
                definition: r.try_get("definition").map_err(db_err)?,
                is_obsolete: r.try_get("is_obsolete").map_err(db_err)?,
                synonyms: r.try_get("synonyms").map_err(db_err)?,
            }),
            None => None,
        })
    }
}
