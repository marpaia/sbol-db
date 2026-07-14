//! Repository for `sbol3`-kind graphs: the named graphs created by importing
//! an SBOL document. Each is dedicated to one import and carries that import's
//! metadata (name, serialization format, source, content hash). `GraphId` is
//! the graph's surrogate id; its IRI is `graph:document:{id}`. Standalone
//! `verbatim` graphs (written through the Graph Store / SPARQL Update
//! endpoints) are not managed here.

use sbol_db_core::{DomainError, GraphId, GraphRecord, IriString, NewGraph, SerializationFormat};
use sbol_db_rdf::GRAPH_IRI_PREFIX;
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

use sbol_db_storage::ListGraphsFilter;

#[derive(Clone)]
pub struct GraphRepository {
    pool: PgPool,
}

impl GraphRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the `sbol3`-kind graph that backs a document. The caller mints
    /// the `GraphId` so the document's triples can be attributed to its
    /// `graph:document:{id}` IRI before this write.
    pub async fn insert(
        &self,
        conn: &mut sqlx::PgConnection,
        id: GraphId,
        input: NewGraph,
    ) -> Result<GraphId, DomainError> {
        let id = id.0;
        let iri = format!("{GRAPH_IRI_PREFIX}{id}");
        sqlx::query(
            r#"
            INSERT INTO sbol_graphs (
                iri, id, kind, document_iri, name, description,
                serialization_format, source_uri, content_hash, created_by
            ) VALUES ($1, $2, 'sbol3', $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(&iri)
        .bind(id)
        .bind(input.document_iri.as_ref().map(|i| i.as_str()))
        .bind(input.name)
        .bind(input.description)
        .bind(input.serialization_format.as_db_str())
        .bind(input.source_uri)
        .bind(input.content_hash)
        .bind(input.created_by)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

        Ok(GraphId(id))
    }

    /// Returns true when any document graph already carries this content hash.
    /// Used by bulk import to dedup re-uploaded files without re-parsing.
    pub async fn exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        let row = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM sbol_graphs \
             WHERE kind = 'sbol3' AND content_hash = $1) AS hit",
        )
        .bind(hash)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row.try_get::<bool, _>("hit").map_err(db_err)
    }

    pub async fn get(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_graphs
            WHERE id = $1 AND kind = 'sbol3'
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
    pub async fn list(&self, filter: &ListGraphsFilter) -> Result<Vec<GraphRecord>, DomainError> {
        let format_value = filter.format.map(|f| f.as_db_str().to_owned());
        let rows = sqlx::query(
            r#"
            SELECT id, document_iri, name, description, serialization_format,
                   source_uri, content_hash, created_at, updated_at
            FROM sbol_graphs
            WHERE kind = 'sbol3'
              AND ($1::text IS NULL OR name ILIKE '%' || $1 || '%')
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

    /// Resolve a document graph's surrogate id from its (unique) document IRI.
    pub async fn id_by_document_iri(
        &self,
        document_iri: &str,
    ) -> Result<Option<GraphId>, DomainError> {
        let row =
            sqlx::query("SELECT id FROM sbol_graphs WHERE document_iri = $1 AND kind = 'sbol3'")
                .bind(document_iri)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        match row {
            Some(row) => Ok(Some(GraphId(row.try_get("id").map_err(db_err)?))),
            None => Ok(None),
        }
    }

    /// Delete one document by dropping its graph. The graph's triples cascade
    /// via their FK; the graph's derived objects (whose `graph_id` FK is
    /// `ON DELETE SET NULL`) are removed explicitly first, so the object view
    /// no longer returns them and their revisions cascade away. Both deletes
    /// share one transaction. Returns `true` if a graph row was removed.
    pub async fn delete(&self, id: GraphId) -> Result<bool, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let removed = delete_graph_on(&mut tx, id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(removed)
    }

    /// Connection-scoped [`id_by_document_iri`](Self::id_by_document_iri), so a
    /// replace can resolve and delete the old graph inside the import's
    /// transaction.
    pub async fn id_by_document_iri_in(
        &self,
        conn: &mut sqlx::PgConnection,
        document_iri: &str,
    ) -> Result<Option<GraphId>, DomainError> {
        let row =
            sqlx::query("SELECT id FROM sbol_graphs WHERE document_iri = $1 AND kind = 'sbol3'")
                .bind(document_iri)
                .fetch_optional(&mut *conn)
                .await
                .map_err(db_err)?;
        match row {
            Some(row) => Ok(Some(GraphId(row.try_get("id").map_err(db_err)?))),
            None => Ok(None),
        }
    }

    /// Connection-scoped [`delete`](Self::delete), for an in-transaction replace.
    pub async fn delete_in(
        &self,
        conn: &mut sqlx::PgConnection,
        id: GraphId,
    ) -> Result<bool, DomainError> {
        delete_graph_on(conn, id).await
    }
}

/// Delete a document graph's derived objects then the graph row itself on one
/// connection. Objects are removed before the graph so their `ON DELETE SET
/// NULL` `graph_id` still resolves the set; `sbol_object_revisions` cascade
/// from the object rows.
async fn delete_graph_on(conn: &mut sqlx::PgConnection, id: GraphId) -> Result<bool, DomainError> {
    sqlx::query("DELETE FROM sbol_objects WHERE graph_id = $1")
        .bind(id.0)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    let affected = sqlx::query("DELETE FROM sbol_graphs WHERE id = $1 AND kind = 'sbol3'")
        .bind(id.0)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?
        .rows_affected();
    Ok(affected > 0)
}

fn row_to_record(row: sqlx::postgres::PgRow) -> Result<GraphRecord, DomainError> {
    let id: uuid::Uuid = row.try_get("id").map_err(db_err)?;
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
        id: GraphId(id),
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
