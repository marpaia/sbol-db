//! The graph-owned triplestore over SQLite.

use sbol_db_core::{DomainError, IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_rdf::hash_bytes;
use sbol_db_storage::{GraphFilter, PatternObject, PatternSubject};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

use crate::pool::db_err;

/// Unit separator; cannot occur in an IRI and is not emitted by the
/// serializers, so distinct triples cannot collide when their positions are
/// concatenated for the identity hash.
const SEP: char = '\u{1F}';

#[derive(Clone)]
pub struct TripleRepository {
    pool: SqlitePool,
}

/// One triple decomposed into its nullable column values.
struct Cols {
    graph_iri: Option<String>,
    subject_iri: Option<String>,
    subject_blank: Option<String>,
    predicate_iri: String,
    object_iri: Option<String>,
    object_blank: Option<String>,
    object_literal: Option<String>,
    datatype_iri: Option<String>,
    language: Option<String>,
}

impl Cols {
    fn from_triple(t: &Triple) -> Self {
        let (subject_iri, subject_blank) = match &t.subject {
            SubjectTerm::Iri(iri) => (Some(iri.as_str().to_owned()), None),
            SubjectTerm::BlankNode(node) => (None, Some(node.clone())),
        };
        let (object_iri, object_blank, object_literal, datatype_iri, language) = match &t.object {
            ObjectTerm::Iri(iri) => (Some(iri.as_str().to_owned()), None, None, None, None),
            ObjectTerm::BlankNode(node) => (None, Some(node.clone()), None, None, None),
            ObjectTerm::Literal {
                value,
                datatype,
                language,
            } => (
                None,
                None,
                Some(value.clone()),
                Some(datatype.as_str().to_owned()),
                language.clone(),
            ),
        };
        Self {
            graph_iri: t.graph_iri.as_ref().map(|i| i.as_str().to_owned()),
            subject_iri,
            subject_blank,
            predicate_iri: t.predicate.as_str().to_owned(),
            object_iri,
            object_blank,
            object_literal,
            datatype_iri,
            language,
        }
    }

    /// The triple's set-identity: a hash over all RDF positions. Two triples
    /// are the same iff their positions match, so re-inserting one is a no-op.
    fn identity(&self) -> Vec<u8> {
        let blank = String::new();
        let joined = [
            self.graph_iri.as_deref().unwrap_or(&blank),
            self.subject_iri.as_deref().unwrap_or(&blank),
            self.subject_blank.as_deref().unwrap_or(&blank),
            self.predicate_iri.as_str(),
            self.object_iri.as_deref().unwrap_or(&blank),
            self.object_blank.as_deref().unwrap_or(&blank),
            self.object_literal.as_deref().unwrap_or(&blank),
            self.datatype_iri.as_deref().unwrap_or(&blank),
            self.language.as_deref().unwrap_or(&blank),
        ]
        .join(&SEP.to_string());
        hash_bytes(joined.as_bytes())
    }
}

impl TripleRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a batch, skipping triples already present in their graph (set
    /// semantics via the unique `triple_key`). Returns the count actually
    /// inserted.
    pub async fn insert_triples(
        &self,
        conn: &mut SqliteConnection,
        triples: &[Triple],
        source: &str,
    ) -> Result<usize, DomainError> {
        let mut inserted = 0usize;
        for triple in triples {
            let c = Cols::from_triple(triple);
            let key = c.identity();
            let affected = sqlx::query(
                r#"
                INSERT OR IGNORE INTO sbol_triples (
                    graph_iri, subject_iri, subject_blank, predicate_iri,
                    object_iri, object_blank, object_literal, datatype_iri,
                    language, source, triple_key
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&c.graph_iri)
            .bind(&c.subject_iri)
            .bind(&c.subject_blank)
            .bind(&c.predicate_iri)
            .bind(&c.object_iri)
            .bind(&c.object_blank)
            .bind(&c.object_literal)
            .bind(&c.datatype_iri)
            .bind(&c.language)
            .bind(source)
            .bind(key)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?
            .rows_affected();
            inserted += affected as usize;
        }
        Ok(inserted)
    }

    /// Delete every row matching one of `triples` on all RDF positions.
    pub async fn delete_triples(
        &self,
        conn: &mut SqliteConnection,
        triples: &[Triple],
    ) -> Result<usize, DomainError> {
        let mut deleted = 0usize;
        for triple in triples {
            let key = Cols::from_triple(triple).identity();
            let affected = sqlx::query("DELETE FROM sbol_triples WHERE triple_key = ?")
                .bind(key)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?
                .rows_affected();
            deleted += affected as usize;
        }
        Ok(deleted)
    }

    /// Delete every triple in a named graph, or the default partition when
    /// `graph` is `None`.
    pub async fn clear_graph(
        &self,
        conn: &mut SqliteConnection,
        graph: Option<&str>,
    ) -> Result<usize, DomainError> {
        let affected = match graph {
            Some(g) => sqlx::query("DELETE FROM sbol_triples WHERE graph_iri = ?")
                .bind(g)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?,
            None => sqlx::query("DELETE FROM sbol_triples WHERE graph_iri IS NULL")
                .execute(&mut *conn)
                .await
                .map_err(db_err)?,
        };
        Ok(affected.rows_affected() as usize)
    }

    /// Register a named graph if absent. Triples owned by a named graph need
    /// their `sbol_graphs` row (the FK target) to exist first.
    pub async fn ensure_graph(
        &self,
        conn: &mut SqliteConnection,
        iri: &str,
        kind: &str,
    ) -> Result<(), DomainError> {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT OR IGNORE INTO sbol_graphs (iri, id, kind, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(iri)
        .bind(id)
        .bind(kind)
        .bind(&now)
        .bind(&now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Drop a graph row, cascading to its triples. Returns whether a row was
    /// removed.
    pub async fn delete_graph(
        &self,
        conn: &mut SqliteConnection,
        iri: &str,
    ) -> Result<bool, DomainError> {
        let affected = sqlx::query("DELETE FROM sbol_graphs WHERE iri = ?")
            .bind(iri)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let rows = sqlx::query(
            "SELECT graph_iri, subject_iri, subject_blank, predicate_iri, object_iri, \
             object_blank, object_literal, datatype_iri, language \
             FROM sbol_triples WHERE subject_iri = ?",
        )
        .bind(subject_iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_triple).collect()
    }

    pub async fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let base = "SELECT graph_iri, subject_iri, subject_blank, predicate_iri, object_iri, \
                    object_blank, object_literal, datatype_iri, language FROM sbol_triples";
        let rows = match graph {
            Some(g) => {
                sqlx::query(&format!("{base} WHERE graph_iri = ? LIMIT ?"))
                    .bind(g)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await
            }
            None => {
                sqlx::query(&format!("{base} WHERE graph_iri IS NULL LIMIT ?"))
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await
            }
        }
        .map_err(db_err)?;
        rows.into_iter().map(row_to_triple).collect()
    }

    pub async fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        let rows =
            sqlx::query("SELECT DISTINCT graph_iri FROM sbol_triples WHERE graph_iri IS NOT NULL")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
        rows.into_iter()
            .map(|r| r.try_get::<String, _>("graph_iri").map_err(db_err))
            .collect()
    }

    /// Triple-pattern scan: any position may be bound or wildcarded.
    pub async fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT graph_iri, subject_iri, subject_blank, predicate_iri, object_iri, \
             object_blank, object_literal, datatype_iri, language \
             FROM sbol_triples WHERE 1=1",
        );
        match subject {
            Some(PatternSubject::Iri(s)) => {
                qb.push(" AND subject_iri = ").push_bind(s.clone());
            }
            Some(PatternSubject::Blank(b)) => {
                qb.push(" AND subject_blank = ").push_bind(b.clone());
            }
            None => {}
        }
        if let Some(p) = predicate {
            qb.push(" AND predicate_iri = ").push_bind(p.to_owned());
        }
        match object {
            Some(PatternObject::Iri(o)) => {
                qb.push(" AND object_iri = ").push_bind(o.clone());
            }
            Some(PatternObject::Blank(b)) => {
                qb.push(" AND object_blank = ").push_bind(b.clone());
            }
            Some(PatternObject::Literal {
                value,
                datatype,
                language,
            }) => {
                qb.push(" AND object_literal = ").push_bind(value.clone());
                qb.push(" AND datatype_iri = ").push_bind(datatype.clone());
                match language {
                    Some(lang) => {
                        qb.push(" AND language = ").push_bind(lang.clone());
                    }
                    None => {
                        qb.push(" AND language IS NULL");
                    }
                }
            }
            None => {}
        }
        match graph {
            Some(GraphFilter::AnyNamed) => {
                qb.push(" AND graph_iri IS NOT NULL");
            }
            Some(GraphFilter::DefaultOnly) => {
                qb.push(" AND graph_iri IS NULL");
            }
            Some(GraphFilter::Iri(g)) => {
                qb.push(" AND graph_iri = ").push_bind(g.clone());
            }
            None => {}
        }
        qb.push(" LIMIT ").push_bind(limit);

        let rows = qb.build().fetch_all(&self.pool).await.map_err(db_err)?;
        rows.into_iter().map(row_to_triple).collect()
    }
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
