//! Derived-view SBOL objects over SQLite. Ontology-term arrays (`types`,
//! `roles`) and the `data` property bag are stored as JSON text.

use sbol_db_core::{DomainError, GraphId, IriString, ObjectId, ObjectSummary, SbolObjectRecord};
use sbol_db_storage::ListObjectsFilter;
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::pool::db_err;

#[derive(Clone)]
pub struct SbolObjectRepository {
    pool: SqlitePool,
}

const SELECT_COLS: &str = "SELECT id, iri, sbol_class, display_id, name, description, \
    graph_id, types, roles, data, content_hash FROM sbol_objects";

impl SbolObjectRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or update by IRI. The object keeps its id across updates.
    pub async fn upsert(
        &self,
        conn: &mut SqliteConnection,
        summary: &ObjectSummary,
        graph_id: Option<GraphId>,
    ) -> Result<(), DomainError> {
        let now = chrono::Utc::now();
        let types = serde_json::to_string(&summary.types).map_err(db_err)?;
        let roles = serde_json::to_string(&summary.roles).map_err(db_err)?;
        let data = serde_json::to_string(&summary.data).map_err(db_err)?;
        sqlx::query(
            r#"
            INSERT INTO sbol_objects (
                id, iri, sbol_class, display_id, name, description,
                graph_id, types, roles, data, content_hash, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(iri) DO UPDATE SET
                sbol_class   = excluded.sbol_class,
                display_id   = excluded.display_id,
                name         = excluded.name,
                description  = excluded.description,
                graph_id     = excluded.graph_id,
                types        = excluded.types,
                roles        = excluded.roles,
                data         = excluded.data,
                content_hash = excluded.content_hash,
                updated_at   = excluded.updated_at
            "#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(summary.iri.as_str())
        .bind(&summary.sbol_class)
        .bind(summary.display_id.as_deref())
        .bind(summary.name.as_deref())
        .bind(summary.description.as_deref())
        .bind(graph_id.map(|g| g.0.to_string()))
        .bind(types)
        .bind(roles)
        .bind(data)
        .bind(&summary.content_hash)
        .bind(now)
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn get_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        let row = sqlx::query("SELECT iri FROM sbol_objects WHERE id = ?")
            .bind(id.0.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(|r| r.try_get::<String, _>("iri").map_err(db_err))
            .transpose()
    }

    pub async fn get_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        let row = sqlx::query(&format!("{SELECT_COLS} WHERE iri = ? AND is_deleted = 0"))
            .bind(iri)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(row_to_record).transpose()
    }

    pub async fn get_by_iris(&self, iris: &[&str]) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let mut out = Vec::with_capacity(iris.len());
        for iri in iris {
            if let Some(record) = self.get_by_iri(iri).await? {
                out.push(record);
            }
        }
        Ok(out)
    }

    pub async fn list(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let limit = filter.limit.clamp(1, 5000) as i64;
        let rows = sqlx::query(&format!(
            r#"
            {SELECT_COLS}
            WHERE is_deleted = 0
              AND (?1 IS NULL OR sbol_class = ?1)
              AND (?2 IS NULL OR EXISTS (SELECT 1 FROM json_each(roles) WHERE value = ?2))
              AND (?3 IS NULL OR graph_id = ?3)
              AND (?4 IS NULL OR iri > ?4)
            ORDER BY iri ASC
            LIMIT ?5
            "#
        ))
        .bind(filter.sbol_class.as_deref())
        .bind(filter.role.as_deref())
        .bind(filter.graph_id.map(|g| g.0.to_string()))
        .bind(filter.after_iri.as_deref())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_record).collect()
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<SbolObjectRecord, DomainError> {
    let id: String = row.try_get("id").map_err(db_err)?;
    let iri: String = row.try_get("iri").map_err(db_err)?;
    let sbol_class: String = row.try_get("sbol_class").map_err(db_err)?;
    let display_id: Option<String> = row.try_get("display_id").map_err(db_err)?;
    let name: Option<String> = row.try_get("name").map_err(db_err)?;
    let description: Option<String> = row.try_get("description").map_err(db_err)?;
    let graph_id: Option<String> = row.try_get("graph_id").map_err(db_err)?;
    let types: String = row.try_get("types").map_err(db_err)?;
    let roles: String = row.try_get("roles").map_err(db_err)?;
    let data: String = row.try_get("data").map_err(db_err)?;
    let content_hash: Option<Vec<u8>> = row.try_get("content_hash").map_err(db_err)?;

    let graph_id = graph_id
        .map(|g| uuid::Uuid::parse_str(&g))
        .transpose()
        .map_err(db_err)?
        .map(GraphId);

    Ok(SbolObjectRecord {
        id: ObjectId(uuid::Uuid::parse_str(&id).map_err(db_err)?),
        iri: IriString::unchecked(iri),
        sbol_class,
        display_id,
        name,
        description,
        graph_id,
        types: serde_json::from_str(&types).map_err(db_err)?,
        roles: serde_json::from_str(&roles).map_err(db_err)?,
        data: serde_json::from_str(&data).map_err(db_err)?,
        content_hash: content_hash.unwrap_or_default(),
    })
}
