//! Dashboard / graph-browser reads for the lab UI (the [`LabStore`] surface).

use sbol_db_core::{DomainError, GraphId, IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_storage::{ClassCount, CorpusCounts, GraphOverview, GraphTriplesPage};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct LabRepository {
    pool: PgPool,
}

impl LabRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT
              resources                                   AS objects,
              named_graphs                                AS graphs,
              triples,
              sequences,
              (SELECT count(*) FROM sbol_validation_runs) AS validation_runs,
              ontologies
            FROM sbol_catalog_stats
            WHERE singleton
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
            validation_runs: row.try_get("validation_runs").map_err(db_err)?,
            ontologies: row.try_get("ontologies").map_err(db_err)?,
        })
    }

    pub async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sbol_graphs WHERE ($1::text IS NULL OR kind = $1)",
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
              g.resource_count AS object_count,
              g.triple_count
            FROM sbol_graphs g
            WHERE ($1::text IS NULL OR g.kind = $1)
            ORDER BY g.created_at DESC
            LIMIT $2 OFFSET $3
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

    pub async fn catalog_graphs(
        &self,
        after: Option<&str>,
        text: Option<&str>,
        limit: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        let text = text
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        let rows = sqlx::query(
            r#"
            SELECT
              g.id, g.iri, g.kind, g.name, g.serialization_format, g.source_uri, g.created_at,
              g.resource_count AS object_count,
              g.triple_count
            FROM sbol_graphs g
            WHERE ($1::text IS NULL OR g.iri > $1)
              AND ($2::text IS NULL OR position($2 in lower(g.iri)) > 0
                   OR position($2 in lower(coalesce(g.name, ''))) > 0)
            ORDER BY g.iri
            LIMIT $3
            "#,
        )
        .bind(after)
        .bind(text.as_deref())
        .bind(limit)
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
              g.resource_count AS object_count,
              g.triple_count
            FROM sbol_graphs g
            WHERE g.id = $1
            "#,
        )
        .bind(id.0)
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
        let graph =
            sqlx::query("SELECT iri::text AS iri, triple_count FROM sbol_graphs WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        let Some(graph) = graph else {
            return Ok(None);
        };
        let iri: String = graph.try_get("iri").map_err(db_err)?;
        let total: i64 = graph.try_get("triple_count").map_err(db_err)?;
        let rows = sqlx::query(
            r#"
            SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                   object_iri, object_blank, object_literal, datatype_iri, language
            FROM sbol_triples
            WHERE graph_iri = $1
            ORDER BY subject_iri NULLS LAST, subject_blank NULLS LAST, predicate_iri, id
            LIMIT $2 OFFSET $3
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
        Ok(Some(GraphTriplesPage {
            total: Some(total),
            triples,
        }))
    }

    pub async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT type_iri AS sbol_class, count(DISTINCT iri) AS n
            FROM accel_type
            GROUP BY type_iri
            ORDER BY n DESC
            LIMIT $1
            "#,
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

fn row_to_overview(row: sqlx::postgres::PgRow) -> Result<GraphOverview, DomainError> {
    Ok(GraphOverview {
        id: GraphId(row.try_get("id").map_err(db_err)?),
        iri: row.try_get("iri").map_err(db_err)?,
        kind: row.try_get("kind").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        source_uri: row.try_get("source_uri").map_err(db_err)?,
        serialization_format: row.try_get("serialization_format").map_err(db_err)?,
        created_at: Some(row.try_get("created_at").map_err(db_err)?),
        object_count: Some(row.try_get("object_count").map_err(db_err)?),
        triple_count: Some(row.try_get("triple_count").map_err(db_err)?),
    })
}

fn row_to_triple(row: sqlx::postgres::PgRow) -> Result<Triple, DomainError> {
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
