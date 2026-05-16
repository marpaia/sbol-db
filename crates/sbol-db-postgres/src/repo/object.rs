use sbol_db_core::{DocumentId, DomainError, IriString, ObjectId, ObjectSummary, SbolObjectRecord};
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct SbolObjectRepository {
    pool: PgPool,
}

pub struct UpsertResult {
    pub object_id: ObjectId,
    pub revision_number: i64,
    pub previous_content_hash: Option<Vec<u8>>,
}

impl SbolObjectRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or update by IRI. Returns the resulting object id, the new
    /// revision number, and the previous content hash (if any) so callers
    /// can detect a no-op.
    pub async fn upsert(
        &self,
        conn: &mut sqlx::PgConnection,
        summary: &ObjectSummary,
        document_id: Option<DocumentId>,
    ) -> Result<UpsertResult, DomainError> {
        let row = sqlx::query(
            r#"
            WITH prev AS (
                SELECT id, content_hash FROM sbol_objects WHERE iri = $1
            ),
            upsert AS (
                INSERT INTO sbol_objects (
                    iri, sbol_class, display_id, name, description,
                    document_id, types, roles, data, content_hash, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7::text[]::sbol_ontology_term[],
                          $8::text[]::sbol_ontology_term[], $9, $10, now())
                ON CONFLICT (iri) DO UPDATE SET
                    sbol_class = EXCLUDED.sbol_class,
                    display_id = EXCLUDED.display_id,
                    name = EXCLUDED.name,
                    description = EXCLUDED.description,
                    document_id = EXCLUDED.document_id,
                    types = EXCLUDED.types,
                    roles = EXCLUDED.roles,
                    data = EXCLUDED.data,
                    content_hash = EXCLUDED.content_hash,
                    updated_at = now()
                RETURNING id
            ),
            next_revision AS (
                INSERT INTO sbol_object_revisions (
                    object_id, iri, revision_number, data, content_hash
                )
                SELECT
                    (SELECT id FROM upsert),
                    $1,
                    COALESCE(
                        (SELECT MAX(revision_number) + 1 FROM sbol_object_revisions
                            WHERE object_id = (SELECT id FROM upsert)),
                        1
                    ),
                    $9,
                    $10
                RETURNING revision_number
            )
            SELECT
                (SELECT id FROM upsert) AS id,
                (SELECT revision_number FROM next_revision) AS revision_number,
                (SELECT content_hash FROM prev) AS previous_content_hash
            "#,
        )
        .bind(summary.iri.as_str())
        .bind(&summary.sbol_class)
        .bind(summary.display_id.as_deref())
        .bind(summary.name.as_deref())
        .bind(summary.description.as_deref())
        .bind(document_id.map(|d| d.0))
        .bind(&summary.types)
        .bind(&summary.roles)
        .bind(&summary.data)
        .bind(&summary.content_hash)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;

        let id: Uuid = row.try_get("id").map_err(db_err)?;
        let revision_number: i64 = row.try_get("revision_number").map_err(db_err)?;
        let previous_content_hash: Option<Vec<u8>> =
            row.try_get("previous_content_hash").map_err(db_err)?;

        Ok(UpsertResult {
            object_id: ObjectId(id),
            revision_number,
            previous_content_hash,
        })
    }

    pub async fn get_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        let row = sqlx::query("SELECT iri FROM sbol_objects WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(row) => Ok(Some(row.try_get::<String, _>("iri").map_err(db_err)?)),
            None => Ok(None),
        }
    }

    pub async fn get_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, iri::text AS iri, sbol_class, display_id, name, description,
                   document_id, types::text[] AS types, roles::text[] AS roles,
                   data, content_hash
            FROM sbol_objects
            WHERE iri = $1 AND is_deleted = false
            "#,
        )
        .bind(iri)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        row.map(row_to_record).transpose()
    }

    /// Bulk variant of [`get_by_iri`]. Returns matching live rows in arbitrary
    /// order; IRIs that don't resolve are simply absent from the result. A
    /// single indexed scan via `iri = ANY($1)`.
    pub async fn get_by_iris(&self, iris: &[&str]) -> Result<Vec<SbolObjectRecord>, DomainError> {
        if iris.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<String> = iris.iter().map(|s| (*s).to_owned()).collect();
        let rows = sqlx::query(
            r#"
            SELECT id, iri::text AS iri, sbol_class, display_id, name, description,
                   document_id, types::text[] AS types, roles::text[] AS roles,
                   data, content_hash
            FROM sbol_objects
            WHERE iri::text = ANY($1::text[]) AND is_deleted = false
            "#,
        )
        .bind(&owned)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(row_to_record).collect()
    }

    /// Paginated listing for corpus-scale export. Keyset cursor on `iri`
    /// (lexicographic ascending); pass the last `iri` of the prior page as
    /// `after_iri` to fetch the next. Filters compose: when any of
    /// `sbol_class`, `role`, or `document_id` is `Some`, the corresponding
    /// predicate is added to the WHERE clause.
    pub async fn list(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let limit = filter.limit.clamp(1, 5000) as i64;
        let rows = sqlx::query(
            r#"
            SELECT id, iri::text AS iri, sbol_class, display_id, name, description,
                   document_id, types::text[] AS types, roles::text[] AS roles,
                   data, content_hash
            FROM sbol_objects
            WHERE is_deleted = false
              AND ($1::text IS NULL OR sbol_class = $1)
              AND ($2::text IS NULL OR $2 = ANY(roles::text[]))
              AND ($3::uuid IS NULL OR document_id = $3)
              AND ($4::text IS NULL OR iri::text > $4)
            ORDER BY iri::text ASC
            LIMIT $5
            "#,
        )
        .bind(filter.sbol_class.as_deref())
        .bind(filter.role.as_deref())
        .bind(filter.document_id.map(|d| d.0))
        .bind(filter.after_iri.as_deref())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(row_to_record).collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ListObjectsFilter {
    pub sbol_class: Option<String>,
    pub role: Option<String>,
    pub document_id: Option<DocumentId>,
    pub after_iri: Option<String>,
    pub limit: u32,
}

fn row_to_record(row: sqlx::postgres::PgRow) -> Result<SbolObjectRecord, DomainError> {
    let id: Uuid = row.try_get("id").map_err(db_err)?;
    let iri: String = row.try_get("iri").map_err(db_err)?;
    let sbol_class: String = row.try_get("sbol_class").map_err(db_err)?;
    let display_id: Option<String> = row.try_get("display_id").map_err(db_err)?;
    let name: Option<String> = row.try_get("name").map_err(db_err)?;
    let description: Option<String> = row.try_get("description").map_err(db_err)?;
    let document_id: Option<Uuid> = row.try_get("document_id").map_err(db_err)?;
    let types: Vec<String> = row.try_get("types").map_err(db_err)?;
    let roles: Vec<String> = row.try_get("roles").map_err(db_err)?;
    let data: serde_json::Value = row.try_get("data").map_err(db_err)?;
    let content_hash: Option<Vec<u8>> = row.try_get("content_hash").map_err(db_err)?;
    Ok(SbolObjectRecord {
        id: ObjectId(id),
        iri: IriString::unchecked(iri),
        sbol_class,
        display_id,
        name,
        description,
        document_id: document_id.map(DocumentId),
        types,
        roles,
        data,
        content_hash: content_hash.unwrap_or_default(),
    })
}
