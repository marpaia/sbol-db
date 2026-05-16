use sbol_db_core::{DocumentId, DomainError, ObjectTerm, Quad, SubjectTerm};
use sqlx::{QueryBuilder, Row};

use crate::repo::db_err;
use crate::PgPool;

/// Filter on the named-graph column for a pattern scan.
///
/// Mirrors SPARQL `graph_name` semantics from `spareval::QueryableDataset`:
/// `None` here means "no filter" (any graph, including the default graph),
/// `Some(GraphFilter::AnyNamed)` is "any named graph" (default graph excluded),
/// `Some(GraphFilter::DefaultOnly)` is "the default graph only", and
/// `Some(GraphFilter::Iri(g))` is a specific named graph.
#[derive(Clone, Debug)]
pub enum GraphFilter {
    AnyNamed,
    DefaultOnly,
    Iri(String),
}

/// A bound subject position in a triple pattern.
#[derive(Clone, Debug)]
pub enum PatternSubject {
    Iri(String),
    Blank(String),
}

/// A bound object position in a triple pattern.
#[derive(Clone, Debug)]
pub enum PatternObject {
    Iri(String),
    Blank(String),
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}

#[derive(Clone)]
pub struct QuadRepository {
    pool: PgPool,
}

impl QuadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Delete existing quads for the document then insert the new set in one
    /// `UNNEST`-backed round-trip. Returns the inserted count.
    pub async fn replace_document_graph(
        &self,
        conn: &mut sqlx::PgConnection,
        document_id: DocumentId,
        quads: &[Quad],
    ) -> Result<usize, DomainError> {
        sqlx::query("DELETE FROM sbol_quads WHERE document_id = $1")
            .bind(document_id.0)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;

        if quads.is_empty() {
            return Ok(0);
        }

        let mut graph_iri: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut subject_iri: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut subject_blank: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut predicate_iri: Vec<String> = Vec::with_capacity(quads.len());
        let mut object_iri: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut object_blank: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut object_literal: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut datatype_iri: Vec<Option<String>> = Vec::with_capacity(quads.len());
        let mut language: Vec<Option<String>> = Vec::with_capacity(quads.len());

        for q in quads {
            graph_iri.push(q.graph_iri.as_ref().map(|i| i.as_str().to_owned()));
            match &q.subject {
                SubjectTerm::Iri(iri) => {
                    subject_iri.push(Some(iri.as_str().to_owned()));
                    subject_blank.push(None);
                }
                SubjectTerm::BlankNode(node) => {
                    subject_iri.push(None);
                    subject_blank.push(Some(node.clone()));
                }
            }
            predicate_iri.push(q.predicate.as_str().to_owned());
            match &q.object {
                ObjectTerm::Iri(iri) => {
                    object_iri.push(Some(iri.as_str().to_owned()));
                    object_blank.push(None);
                    object_literal.push(None);
                    datatype_iri.push(None);
                    language.push(None);
                }
                ObjectTerm::BlankNode(node) => {
                    object_iri.push(None);
                    object_blank.push(Some(node.clone()));
                    object_literal.push(None);
                    datatype_iri.push(None);
                    language.push(None);
                }
                ObjectTerm::Literal {
                    value,
                    datatype,
                    language: lang,
                } => {
                    object_iri.push(None);
                    object_blank.push(None);
                    object_literal.push(Some(value.clone()));
                    datatype_iri.push(Some(datatype.as_str().to_owned()));
                    language.push(lang.clone());
                }
            }
        }

        let inserted = sqlx::query(
            r#"
            INSERT INTO sbol_quads (
                graph_iri,
                subject_iri,
                subject_blank,
                predicate_iri,
                object_iri,
                object_blank,
                object_literal,
                datatype_iri,
                language,
                document_id
            )
            SELECT
                graph_iri,
                subject_iri,
                subject_blank,
                predicate_iri,
                object_iri,
                object_blank,
                object_literal,
                datatype_iri,
                language,
                $10
            FROM UNNEST(
                $1::text[],
                $2::text[],
                $3::text[],
                $4::text[],
                $5::text[],
                $6::text[],
                $7::text[],
                $8::text[],
                $9::text[]
            ) AS u(
                graph_iri,
                subject_iri,
                subject_blank,
                predicate_iri,
                object_iri,
                object_blank,
                object_literal,
                datatype_iri,
                language
            )
            "#,
        )
        .bind(&graph_iri)
        .bind(&subject_iri)
        .bind(&subject_blank)
        .bind(&predicate_iri)
        .bind(&object_iri)
        .bind(&object_blank)
        .bind(&object_literal)
        .bind(&datatype_iri)
        .bind(&language)
        .bind(document_id.0)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

        Ok(inserted.rows_affected() as usize)
    }

    pub async fn quads_for_subject(&self, subject_iri: &str) -> Result<Vec<Quad>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                   object_iri, object_blank, object_literal, datatype_iri, language
            FROM sbol_quads
            WHERE subject_iri = $1
            "#,
        )
        .bind(subject_iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(row_to_quad).collect()
    }

    /// Triple-pattern scan over `sbol_quads`.
    ///
    /// Any combination of subject / predicate / object / graph positions may be
    /// bound (`Some(...)`) or wildcarded (`None`). The SQL `WHERE` clause is
    /// built dynamically so Postgres' planner sees only the bound predicates
    /// and picks the right SPOG/POSG/OSPG/GSPO index for the access shape.
    ///
    /// `limit` caps the rows returned per call (per-pattern scan cap, not a
    /// total-result cap — that's enforced higher up).
    pub async fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Quad>, DomainError> {
        let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
            "SELECT graph_iri, subject_iri, subject_blank, predicate_iri, \
             object_iri, object_blank, object_literal, datatype_iri, language \
             FROM sbol_quads WHERE 1=1",
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
        rows.into_iter().map(row_to_quad).collect()
    }

    /// Enumerate distinct named graphs present in `sbol_quads`. Used by SPARQL
    /// evaluation to discover the named-graph universe.
    pub async fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        let rows = sqlx::query(
            "SELECT DISTINCT graph_iri::text AS graph_iri \
             FROM sbol_quads WHERE graph_iri IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                let g: String = r.try_get("graph_iri").map_err(db_err)?;
                Ok(g)
            })
            .collect()
    }

    pub async fn quads_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<Quad>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                   object_iri, object_blank, object_literal, datatype_iri, language
            FROM sbol_quads
            WHERE document_id = $1
            "#,
        )
        .bind(document_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(row_to_quad).collect()
    }
}

fn row_to_quad(row: sqlx::postgres::PgRow) -> Result<Quad, DomainError> {
    use sbol_db_core::IriString;
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
        (None, None) => return Err(DomainError::Database("quad missing subject".into())),
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
        (None, None, None) => return Err(DomainError::Database("quad missing object".into())),
    };

    Ok(Quad {
        graph_iri: graph_iri.map(IriString::unchecked),
        subject,
        predicate: IriString::unchecked(predicate_iri),
        object,
    })
}
