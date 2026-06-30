//! Document-graph registry over SQLite.

use sbol_db_core::{DomainError, GraphId, GraphRecord, IriString, NewGraph, SerializationFormat};
use sbol_db_rdf::GRAPH_IRI_PREFIX;
use sbol_db_storage::ListGraphsFilter;
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::pool::db_err;

#[derive(Clone)]
pub struct GraphRepository {
    pool: SqlitePool,
}

impl GraphRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create the `sbol3`-kind graph backing a document under the caller-minted
    /// `id`, returning it.
    pub async fn insert(
        &self,
        conn: &mut SqliteConnection,
        id: GraphId,
        input: NewGraph,
    ) -> Result<GraphId, DomainError> {
        let iri = format!("{GRAPH_IRI_PREFIX}{}", id.0);
        let now = chrono::Utc::now();
        sqlx::query(
            r#"
            INSERT INTO sbol_graphs (
                iri, id, kind, document_iri, name, description,
                serialization_format, source_uri, content_hash, created_by,
                created_at, updated_at
            ) VALUES (?, ?, 'sbol3', ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&iri)
        .bind(id.0.to_string())
        .bind(input.document_iri.as_ref().map(|i| i.as_str().to_owned()))
        .bind(input.name)
        .bind(input.description)
        .bind(input.serialization_format.as_db_str())
        .bind(input.source_uri)
        .bind(input.content_hash)
        .bind(input.created_by)
        .bind(now)
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    pub async fn exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        let row = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM sbol_graphs \
             WHERE kind = 'sbol3' AND content_hash = ?) AS hit",
        )
        .bind(hash)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.try_get::<i64, _>("hit").map_err(db_err)? != 0)
    }

    pub async fn get(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_graphs
            WHERE id = ? AND kind = 'sbol3'
            "#,
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_record).transpose()
    }

    pub async fn list(&self, filter: &ListGraphsFilter) -> Result<Vec<GraphRecord>, DomainError> {
        let format_value = filter.format.map(|f| f.as_db_str().to_owned());
        let rows = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_graphs
            WHERE kind = 'sbol3'
              AND (?1 IS NULL OR name LIKE '%' || ?1 || '%')
              AND (?2 IS NULL OR serialization_format = ?2)
            ORDER BY created_at DESC
            LIMIT ?3
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

    pub async fn delete(&self, id: GraphId) -> Result<bool, DomainError> {
        let affected = sqlx::query("DELETE FROM sbol_graphs WHERE id = ? AND kind = 'sbol3'")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected > 0)
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<GraphRecord, DomainError> {
    let id: String = row.try_get("id").map_err(db_err)?;
    let document_iri: Option<String> = row.try_get("document_iri").map_err(db_err)?;
    let name: Option<String> = row.try_get("name").map_err(db_err)?;
    let description: Option<String> = row.try_get("description").map_err(db_err)?;
    let serialization_format: Option<String> =
        row.try_get("serialization_format").map_err(db_err)?;
    let source_uri: Option<String> = row.try_get("source_uri").map_err(db_err)?;
    let content_hash: Option<Vec<u8>> = row.try_get("content_hash").map_err(db_err)?;
    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(db_err)?;
    let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(db_err)?;
    let serialization_format = serialization_format
        .as_deref()
        .map(parse_serialization_format)
        .transpose()?
        .unwrap_or(SerializationFormat::Json);
    Ok(GraphRecord {
        id: GraphId(uuid::Uuid::parse_str(&id).map_err(db_err)?),
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
