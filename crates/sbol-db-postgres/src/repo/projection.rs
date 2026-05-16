use sbol_db_core::{DomainError, IriString};
use serde_json::Value;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct ProjectionEventRepository {
    _pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct ProjectionEvent {
    pub event_type: String,
    pub subject_iri: Option<IriString>,
    pub graph_iri: Option<IriString>,
    pub payload: Value,
}

impl ProjectionEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { _pool: pool }
    }

    pub async fn append(
        &self,
        conn: &mut sqlx::PgConnection,
        event: ProjectionEvent,
    ) -> Result<i64, DomainError> {
        use sqlx::Row;
        let row = sqlx::query(
            r#"
            INSERT INTO rdf_projection_events (event_type, subject_iri, graph_iri, payload)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(event.event_type)
        .bind(event.subject_iri.as_ref().map(|i| i.as_str()))
        .bind(event.graph_iri.as_ref().map(|i| i.as_str()))
        .bind(event.payload)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
        let id: i64 = row.try_get("id").map_err(db_err)?;
        Ok(id)
    }
}
