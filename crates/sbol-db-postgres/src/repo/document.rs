use sbol_db_core::{
    DocumentId, DocumentRecord, DomainError, IriString, NewDocument, SerializationFormat,
};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        conn: &mut sqlx::PgConnection,
        input: NewDocument,
    ) -> Result<DocumentId, DomainError> {
        let row = sqlx::query(
            r#"
            INSERT INTO sbol_documents (
                document_iri,
                name,
                description,
                serialization_format,
                source_uri,
                raw_payload,
                content_hash,
                created_by
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
            "#,
        )
        .bind(input.document_iri.as_ref().map(|i| i.as_str()))
        .bind(input.name)
        .bind(input.description)
        .bind(input.serialization_format.as_db_str())
        .bind(input.source_uri)
        .bind(input.raw_payload)
        .bind(input.content_hash)
        .bind(input.created_by)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;

        let id: uuid::Uuid = row.try_get("id").map_err(db_err)?;
        Ok(DocumentId(id))
    }

    /// Returns true when any document row already carries this content hash.
    /// Used by bulk import to dedup re-uploaded files without re-parsing.
    pub async fn exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        let row = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM sbol_documents WHERE content_hash = $1) AS hit",
        )
        .bind(hash)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row.try_get::<bool, _>("hit").map_err(db_err)
    }

    pub async fn get(&self, id: DocumentId) -> Result<Option<DocumentRecord>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_documents
            WHERE id = $1
            "#,
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        row.map(row_to_record).transpose()
    }

    /// Paginated newest-first listing for the CLI / lab UI. Filters are
    /// optional and ANDed; cardinality is small so we don't expose a
    /// keyset cursor here.
    pub async fn list(
        &self,
        filter: &ListDocumentsFilter,
    ) -> Result<Vec<DocumentRecord>, DomainError> {
        let format_value = filter.format.map(|f| f.as_db_str().to_owned());
        let rows = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_documents
            WHERE ($1::text IS NULL OR name ILIKE '%' || $1 || '%')
              AND ($2::text IS NULL OR serialization_format = $2)
            ORDER BY created_at DESC
            LIMIT $3
            "#,
        )
        .bind(filter.name.as_deref())
        .bind(format_value)
        .bind(filter.limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_record).collect()
    }

    /// Delete one document. The `sbol_quads` FK is `ON DELETE CASCADE`, so
    /// all quads in the document's graph go with it; the `sbol_objects` FK
    /// is `ON DELETE SET NULL`, so objects survive but lose their
    /// `document_id`. Returns `true` if a row was actually deleted.
    pub async fn delete(&self, id: DocumentId) -> Result<bool, DomainError> {
        let affected = sqlx::query("DELETE FROM sbol_documents WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected > 0)
    }
}

/// Filter for [`DocumentRepository::list`]. Empty fields mean no
/// restriction; the limit is required and applied last.
#[derive(Clone, Debug, Default)]
pub struct ListDocumentsFilter {
    /// Case-insensitive substring match against `sbol_documents.name`.
    pub name: Option<String>,
    /// Exact match on the serialization format.
    pub format: Option<SerializationFormat>,
    /// Hard cap on the rows returned.
    pub limit: u32,
}

fn row_to_record(row: sqlx::postgres::PgRow) -> Result<DocumentRecord, DomainError> {
    let id: uuid::Uuid = row.try_get("id").map_err(db_err)?;
    let document_iri: Option<String> = row.try_get("document_iri").map_err(db_err)?;
    let name: Option<String> = row.try_get("name").map_err(db_err)?;
    let description: Option<String> = row.try_get("description").map_err(db_err)?;
    let serialization_format: String = row.try_get("serialization_format").map_err(db_err)?;
    let source_uri: Option<String> = row.try_get("source_uri").map_err(db_err)?;
    let content_hash: Option<Vec<u8>> = row.try_get("content_hash").map_err(db_err)?;
    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(db_err)?;
    let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(db_err)?;
    let serialization_format = parse_serialization_format(&serialization_format)?;
    Ok(DocumentRecord {
        id: DocumentId(id),
        document_iri: document_iri.map(IriString::unchecked),
        name,
        description,
        serialization_format,
        source_uri,
        content_hash: content_hash.unwrap_or_default(),
        created_at,
        updated_at,
    })
}

fn parse_serialization_format(value: &str) -> Result<SerializationFormat, DomainError> {
    Ok(match value {
        "json" => SerializationFormat::Json,
        "jsonld" => SerializationFormat::JsonLd,
        "rdfxml" => SerializationFormat::RdfXml,
        "turtle" => SerializationFormat::Turtle,
        "trig" => SerializationFormat::TriG,
        "ntriples" => SerializationFormat::NTriples,
        "nquads" => SerializationFormat::NQuads,
        "genbank" => SerializationFormat::GenBank,
        "fasta" => SerializationFormat::Fasta,
        other => {
            return Err(DomainError::Database(format!(
                "unknown serialization_format: {other}"
            )))
        }
    })
}
