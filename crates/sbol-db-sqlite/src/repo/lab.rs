//! Dashboard / graph-browser reads for the lab UI over SQLite (the
//! [`LabStore`](sbol_db_storage::LabStore) surface).

use sbol_db_core::{DomainError, GraphId, IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_storage::{ClassCount, CorpusCounts, GraphOverview, GraphTriplesPage};
use sqlx::{Row, SqlitePool};

use crate::pool::db_err;

#[derive(Clone)]
pub struct LabRepository {
    pool: SqlitePool,
}

impl LabRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        // No validation-run table in SQLite yet; reported as 0.
        let row = sqlx::query(
            r#"
            SELECT
              (SELECT count(*) FROM sbol_objects)   AS objects,
              (SELECT count(*) FROM sbol_graphs)    AS graphs,
              (SELECT count(*) FROM sbol_triples)   AS triples,
              (SELECT count(*) FROM sbol_sequences) AS sequences,
              (SELECT count(*) FROM sbol_ontologies) AS ontologies
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(CorpusCounts {
            objects: row.try_get("objects").map_err(db_err)?,
            graphs: row.try_get("graphs").map_err(db_err)?,
            triples: row.try_get("triples").map_err(db_err)?,
            sequences: row.try_get("sequences").map_err(db_err)?,
            validation_runs: 0,
            ontologies: row.try_get("ontologies").map_err(db_err)?,
        })
    }

    pub async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sbol_graphs WHERE (?1 IS NULL OR kind = ?1)",
        )
        .bind(kind)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
              g.id, g.iri, g.kind, g.name, g.serialization_format, g.source_uri, g.created_at,
              coalesce(o.n, 0) AS object_count,
              coalesce(t.n, 0) AS triple_count
            FROM sbol_graphs g
            LEFT JOIN (
              SELECT graph_id, count(*) AS n FROM sbol_objects
              WHERE graph_id IS NOT NULL GROUP BY graph_id
            ) o ON o.graph_id = g.id
            LEFT JOIN (
              SELECT graph_iri, count(*) AS n FROM sbol_triples
              WHERE graph_iri IS NOT NULL GROUP BY graph_iri
            ) t ON t.graph_iri = g.iri
            WHERE (?1 IS NULL OR g.kind = ?1)
            ORDER BY g.created_at DESC
            LIMIT ?2 OFFSET ?3
            "#,
        )
        .bind(kind)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_overview).collect()
    }

    pub async fn get_graph_overview(
        &self,
        id: GraphId,
    ) -> Result<Option<GraphOverview>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT
              g.id, g.iri, g.kind, g.name, g.serialization_format, g.source_uri, g.created_at,
              (SELECT count(*) FROM sbol_objects WHERE graph_id = g.id) AS object_count,
              (SELECT count(*) FROM sbol_triples WHERE graph_iri = g.iri) AS triple_count
            FROM sbol_graphs g
            WHERE g.id = ?
            "#,
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_overview).transpose()
    }

    pub async fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        let iri: Option<String> =
            sqlx::query_scalar::<_, String>("SELECT iri FROM sbol_graphs WHERE id = ?")
                .bind(id.0.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        let Some(iri) = iri else {
            return Ok(None);
        };
        let total: i64 =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sbol_triples WHERE graph_iri = ?")
                .bind(&iri)
                .fetch_one(&self.pool)
                .await
                .map_err(db_err)?;
        let rows = sqlx::query(
            r#"
            SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                   object_iri, object_blank, object_literal, datatype_iri, language
            FROM sbol_triples
            WHERE graph_iri = ?
            ORDER BY subject_iri, subject_blank, predicate_iri, id
            LIMIT ? OFFSET ?
            "#,
        )
        .bind(&iri)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let triples = rows
            .into_iter()
            .map(row_to_triple)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(GraphTriplesPage { total, triples }))
    }

    pub async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        let rows = sqlx::query(
            "SELECT sbol_class, count(*) AS n FROM sbol_objects \
             WHERE sbol_class IS NOT NULL GROUP BY sbol_class ORDER BY n DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(ClassCount {
                    iri: row.try_get("sbol_class").map_err(db_err)?,
                    count: row.try_get("n").map_err(db_err)?,
                })
            })
            .collect()
    }
}

fn row_to_overview(row: sqlx::sqlite::SqliteRow) -> Result<GraphOverview, DomainError> {
    let id: String = row.try_get("id").map_err(db_err)?;
    Ok(GraphOverview {
        id: GraphId(uuid::Uuid::parse_str(&id).map_err(db_err)?),
        iri: row.try_get("iri").map_err(db_err)?,
        kind: row.try_get("kind").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        source_uri: row.try_get("source_uri").map_err(db_err)?,
        serialization_format: row.try_get("serialization_format").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        object_count: row.try_get("object_count").map_err(db_err)?,
        triple_count: row.try_get("triple_count").map_err(db_err)?,
    })
}

fn row_to_triple(row: sqlx::sqlite::SqliteRow) -> Result<Triple, DomainError> {
    let graph_iri: Option<String> = row.try_get("graph_iri").map_err(db_err)?;
    let subject_iri: Option<String> = row.try_get("subject_iri").map_err(db_err)?;
    let subject_blank: Option<String> = row.try_get("subject_blank").map_err(db_err)?;
    let predicate_iri: String = row.try_get("predicate_iri").map_err(db_err)?;
    let object_iri: Option<String> = row.try_get("object_iri").map_err(db_err)?;
    let object_blank: Option<String> = row.try_get("object_blank").map_err(db_err)?;
    let object_literal: Option<String> = row.try_get("object_literal").map_err(db_err)?;
    let datatype_iri: Option<String> = row.try_get("datatype_iri").map_err(db_err)?;
    let language: Option<String> = row.try_get("language").map_err(db_err)?;

    let subject = match (subject_iri, subject_blank) {
        (Some(iri), _) => SubjectTerm::Iri(IriString::unchecked(iri)),
        (None, Some(blank)) => SubjectTerm::BlankNode(blank),
        (None, None) => return Err(DomainError::Database("triple missing subject".into())),
    };
    let object = match (object_iri, object_blank, object_literal) {
        (Some(iri), _, _) => ObjectTerm::Iri(IriString::unchecked(iri)),
        (None, Some(blank), _) => ObjectTerm::BlankNode(blank),
        (None, None, Some(value)) => ObjectTerm::Literal {
            value,
            datatype: IriString::unchecked(
                datatype_iri
                    .unwrap_or_else(|| "http://www.w3.org/2001/XMLSchema#string".to_owned()),
            ),
            language,
        },
        (None, None, None) => return Err(DomainError::Database("triple missing object".into())),
    };
    Ok(Triple {
        graph_iri: graph_iri.map(IriString::unchecked),
        subject,
        predicate: IriString::unchecked(predicate_iri),
        object,
    })
}
