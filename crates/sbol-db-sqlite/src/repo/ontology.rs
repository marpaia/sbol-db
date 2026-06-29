//! Ontology storage and transitive-closure queries over SQLite. The OBO parse,
//! canonical-IRI/alias generation, and closure are derived by the shared
//! [`build_ontology_plan`]; this repo only persists and queries it.

use sbol_db_core::DomainError;
use sbol_db_derive::build_ontology_plan;
use sbol_db_storage::{OntologyLoadReport, OntologyRecord, OntologyTermRecord};
use sqlx::{Row, SqlitePool};

use crate::pool::db_err;

#[derive(Clone)]
pub struct OntologyRepository {
    pool: SqlitePool,
}

impl OntologyRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let plan = build_ontology_plan(prefix, name, source_url, text);

        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // Replacing the ontology row cascades to its terms, aliases, closure.
        sqlx::query("DELETE FROM sbol_ontologies WHERE prefix = ?")
            .bind(&plan.prefix)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        sqlx::query(
            "INSERT INTO sbol_ontologies (prefix, name, source_url, version, term_count, imported_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&plan.prefix)
        .bind(&plan.name)
        .bind(plan.source_url.as_deref())
        .bind(plan.version.as_deref())
        .bind(plan.terms.len() as i64)
        .bind(chrono::Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        for t in &plan.terms {
            let synonyms = serde_json::to_string(&t.synonyms).map_err(db_err)?;
            sqlx::query(
                "INSERT INTO sbol_ontology_terms \
                 (iri, prefix, curie, name, definition, is_obsolete, synonyms) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&t.canonical_iri)
            .bind(&plan.prefix)
            .bind(&t.curie)
            .bind(&t.name)
            .bind(t.definition.as_deref())
            .bind(t.is_obsolete)
            .bind(synonyms)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        for (alias_iri, canonical_iri) in &plan.aliases {
            sqlx::query(
                "INSERT INTO sbol_ontology_term_aliases (alias_iri, canonical_iri, prefix) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(alias_iri) DO UPDATE SET canonical_iri = excluded.canonical_iri",
            )
            .bind(alias_iri)
            .bind(canonical_iri)
            .bind(&plan.prefix)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        for (ancestor, descendant, depth) in &plan.closure {
            sqlx::query(
                "INSERT INTO sbol_ontology_closure (ancestor_iri, descendant_iri, depth, prefix) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(ancestor)
            .bind(descendant)
            .bind(*depth as i64)
            .bind(&plan.prefix)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;
        Ok(plan.report())
    }

    pub async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        let rows = sqlx::query(
            "SELECT prefix, name, source_url, version, term_count, imported_at \
             FROM sbol_ontologies ORDER BY prefix",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(OntologyRecord {
                    prefix: row.try_get("prefix").map_err(db_err)?,
                    name: row.try_get("name").map_err(db_err)?,
                    source_url: row.try_get("source_url").map_err(db_err)?,
                    version: row.try_get("version").map_err(db_err)?,
                    term_count: row.try_get::<i64, _>("term_count").map_err(db_err)? as i32,
                    imported_at: row.try_get("imported_at").map_err(db_err)?,
                })
            })
            .collect()
    }

    pub async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        let row = sqlx::query(
            "SELECT iri AS canonical FROM sbol_ontology_terms WHERE iri = ?1 \
             UNION ALL \
             SELECT canonical_iri AS canonical FROM sbol_ontology_term_aliases WHERE alias_iri = ?1 \
             LIMIT 1",
        )
        .bind(iri)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(|r| r.try_get::<String, _>("canonical").map_err(db_err))
            .transpose()
    }

    pub async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let rows = sqlx::query(
            "SELECT descendant_iri AS iri, depth FROM sbol_ontology_closure \
             WHERE ancestor_iri = ? ORDER BY depth ASC, descendant_iri ASC",
        )
        .bind(&canonical)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>("iri").map_err(db_err)?,
                    row.try_get::<i64, _>("depth").map_err(db_err)? as i16,
                ))
            })
            .collect()
    }

    pub async fn get_term(&self, iri: &str) -> Result<Option<OntologyTermRecord>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let row = sqlx::query(
            "SELECT iri, prefix, curie, name, definition, is_obsolete, synonyms \
             FROM sbol_ontology_terms WHERE iri = ?",
        )
        .bind(&canonical)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_term).transpose()
    }

    pub async fn list_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        let prefix_upper = prefix.to_ascii_uppercase();
        let pattern = search.map(str::trim).filter(|s| !s.is_empty()).map(|s| {
            format!(
                "%{}%",
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });

        let total: i64 = match &pattern {
            Some(pat) => sqlx::query_scalar(
                "SELECT COUNT(*) FROM sbol_ontology_terms \
                 WHERE prefix = ?1 AND (curie LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')",
            )
            .bind(&prefix_upper)
            .bind(pat)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?,
            None => sqlx::query_scalar("SELECT COUNT(*) FROM sbol_ontology_terms WHERE prefix = ?")
                .bind(&prefix_upper)
                .fetch_one(&self.pool)
                .await
                .map_err(db_err)?,
        };

        let rows = match &pattern {
            Some(pat) => sqlx::query(
                "SELECT iri, prefix, curie, name, definition, is_obsolete, synonyms \
                 FROM sbol_ontology_terms \
                 WHERE prefix = ?1 AND (curie LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\') \
                 ORDER BY curie ASC LIMIT ?3 OFFSET ?4",
            )
            .bind(&prefix_upper)
            .bind(pat)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?,
            None => sqlx::query(
                "SELECT iri, prefix, curie, name, definition, is_obsolete, synonyms \
                 FROM sbol_ontology_terms WHERE prefix = ?1 \
                 ORDER BY curie ASC LIMIT ?2 OFFSET ?3",
            )
            .bind(&prefix_upper)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?,
        };

        let terms = rows
            .into_iter()
            .map(row_to_term)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((terms, total))
    }
}

fn row_to_term(row: sqlx::sqlite::SqliteRow) -> Result<OntologyTermRecord, DomainError> {
    let synonyms: String = row.try_get("synonyms").map_err(db_err)?;
    Ok(OntologyTermRecord {
        iri: row.try_get("iri").map_err(db_err)?,
        prefix: row.try_get("prefix").map_err(db_err)?,
        curie: row.try_get("curie").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        definition: row.try_get("definition").map_err(db_err)?,
        is_obsolete: row.try_get("is_obsolete").map_err(db_err)?,
        synonyms: serde_json::from_str(&synonyms).map_err(db_err)?,
    })
}
