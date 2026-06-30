use sbol_db_core::{DomainError, ObjectTerm, SubjectTerm, Triple};
use sqlx::{QueryBuilder, Row};

use crate::repo::db_err;
use crate::PgPool;

use sbol_db_storage::{GraphFilter, PatternObject, PatternSubject};

#[derive(Clone)]
pub struct TripleRepository {
    pool: PgPool,
}

impl TripleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Write a batch of triples in one `UNNEST`-backed round-trip. This is the
    /// RDF write primitive (Graph Store CRUD `POST`, SPARQL `INSERT`, and SBOL
    /// document import), tagged with `source` (e.g. `"graph-store"`,
    /// `"sparql-update"`, `"sbol"`). Each triple's `graph_iri` is its owner (the
    /// graph row must already exist; see [`Self::ensure_graph`]).
    ///
    /// A graph is a set of triples, so a triple already present in its graph is
    /// not stored again: the `sbol_triples` identity index makes the write a
    /// no-op via `ON CONFLICT DO NOTHING`, including duplicates within this same
    /// batch. Returns the count of triples actually inserted (new to the graph).
    pub async fn insert_triples(
        &self,
        conn: &mut sqlx::PgConnection,
        triples: &[Triple],
        source: &str,
    ) -> Result<usize, DomainError> {
        if triples.is_empty() {
            return Ok(0);
        }

        let cols = TripleColumns::from_triples(triples);
        let inserted = sqlx::query(
            r#"
            INSERT INTO sbol_triples (
                graph_iri,
                subject_iri,
                subject_blank,
                predicate_iri,
                object_iri,
                object_blank,
                object_literal,
                datatype_iri,
                language,
                source
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
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(&cols.graph_iri)
        .bind(&cols.subject_iri)
        .bind(&cols.subject_blank)
        .bind(&cols.predicate_iri)
        .bind(&cols.object_iri)
        .bind(&cols.object_blank)
        .bind(&cols.object_literal)
        .bind(&cols.datatype_iri)
        .bind(&cols.language)
        .bind(source)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

        Ok(inserted.rows_affected() as usize)
    }

    /// Delete every row that exactly matches one of `triples` on all RDF
    /// positions (graph, subject, predicate, object, datatype, language) in one
    /// `DELETE ... USING UNNEST` round-trip. `IS NOT DISTINCT FROM` makes NULL
    /// positions (default graph, blank vs IRI) match by value, not collapse.
    /// This is the delete half of SPARQL `DELETE`/`DELETE DATA` and Graph Store
    /// CRUD edits. Ignores `graph_id`/`source`: a triple is matched by its RDF
    /// content regardless of which writer produced it. Returns the deleted count.
    pub async fn delete_triples(
        &self,
        conn: &mut sqlx::PgConnection,
        triples: &[Triple],
    ) -> Result<usize, DomainError> {
        if triples.is_empty() {
            return Ok(0);
        }

        let cols = TripleColumns::from_triples(triples);
        let deleted = sqlx::query(
            r#"
            DELETE FROM sbol_triples q
            USING UNNEST(
                $1::text[],
                $2::text[],
                $3::text[],
                $4::text[],
                $5::text[],
                $6::text[],
                $7::text[],
                $8::text[],
                $9::text[]
            ) AS d(
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
            WHERE q.graph_iri IS NOT DISTINCT FROM d.graph_iri
              AND q.subject_iri IS NOT DISTINCT FROM d.subject_iri
              AND q.subject_blank IS NOT DISTINCT FROM d.subject_blank
              AND q.predicate_iri = d.predicate_iri
              AND q.object_iri IS NOT DISTINCT FROM d.object_iri
              AND q.object_blank IS NOT DISTINCT FROM d.object_blank
              AND q.object_literal IS NOT DISTINCT FROM d.object_literal
              AND q.datatype_iri IS NOT DISTINCT FROM d.datatype_iri
              AND q.language IS NOT DISTINCT FROM d.language
            "#,
        )
        .bind(&cols.graph_iri)
        .bind(&cols.subject_iri)
        .bind(&cols.subject_blank)
        .bind(&cols.predicate_iri)
        .bind(&cols.object_iri)
        .bind(&cols.object_blank)
        .bind(&cols.object_literal)
        .bind(&cols.datatype_iri)
        .bind(&cols.language)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

        Ok(deleted.rows_affected() as usize)
    }

    /// Delete every triple in a named graph. Backs SPARQL `CLEAR`/`DROP GRAPH`
    /// and Graph Store CRUD `DELETE`. `graph` of `None` clears the default
    /// (graphless) partition. Returns the deleted count.
    pub async fn clear_graph(
        &self,
        conn: &mut sqlx::PgConnection,
        graph: Option<&str>,
    ) -> Result<usize, DomainError> {
        let deleted = match graph {
            Some(g) => sqlx::query("DELETE FROM sbol_triples WHERE graph_iri = $1")
                .bind(g)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?,
            None => sqlx::query("DELETE FROM sbol_triples WHERE graph_iri IS NULL")
                .execute(&mut *conn)
                .await
                .map_err(db_err)?,
        };
        Ok(deleted.rows_affected() as usize)
    }

    /// Register a named graph if it is not already present. Every triple with a
    /// named `graph_iri` is owned by its `sbol_graphs` row (FK, `ON DELETE
    /// CASCADE`), so writers must ensure the graph exists before inserting.
    /// `kind` is `"document"` for a graph dedicated to one imported SBOL
    /// document, or `"rdf"` for a standalone/shared graph (the triplestore
    /// case). A pre-existing graph keeps its original kind.
    pub async fn ensure_graph(
        &self,
        conn: &mut sqlx::PgConnection,
        iri: &str,
        kind: &str,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_graphs (iri, kind) VALUES ($1, $2) ON CONFLICT (iri) DO NOTHING",
        )
        .bind(iri)
        .bind(kind)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Delete a graph entirely: remove its `sbol_graphs` row, which cascades to
    /// its triples (derived-view objects and validation runs have `ON DELETE SET
    /// NULL` and survive). Backs Graph Store CRUD `DELETE`, SPARQL `DROP GRAPH`,
    /// and document deletion (an `sbol3`-kind graph is a document). Returns
    /// whether a row was removed.
    pub async fn delete_graph(
        &self,
        conn: &mut sqlx::PgConnection,
        iri: &str,
    ) -> Result<bool, DomainError> {
        let affected = sqlx::query("DELETE FROM sbol_graphs WHERE iri = $1")
            .bind(iri)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                   object_iri, object_blank, object_literal, datatype_iri, language
            FROM sbol_triples
            WHERE subject_iri = $1
            "#,
        )
        .bind(subject_iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(row_to_triple).collect()
    }

    /// Triple-pattern scan over `sbol_triples`.
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
    ) -> Result<Vec<Triple>, DomainError> {
        let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
            "SELECT graph_iri, subject_iri, subject_blank, predicate_iri, \
             object_iri, object_blank, object_literal, datatype_iri, language \
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

    /// Enumerate distinct named graphs present in `sbol_triples`. Used by SPARQL
    /// evaluation to discover the named-graph universe.
    pub async fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        let rows = sqlx::query(
            "SELECT DISTINCT graph_iri::text AS graph_iri \
             FROM sbol_triples WHERE graph_iri IS NOT NULL",
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

    /// Fetch every triple in one named graph (`Some`) or the default/graphless
    /// partition (`None`). Backs the Graph Store CRUD `GET`. `limit` caps the
    /// rows returned.
    pub async fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let rows = match graph {
            Some(g) => sqlx::query(
                r#"
                SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                       object_iri, object_blank, object_literal, datatype_iri, language
                FROM sbol_triples
                WHERE graph_iri = $1
                LIMIT $2
                "#,
            )
            .bind(g)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?,
            None => sqlx::query(
                r#"
                SELECT graph_iri, subject_iri, subject_blank, predicate_iri,
                       object_iri, object_blank, object_literal, datatype_iri, language
                FROM sbol_triples
                WHERE graph_iri IS NULL
                LIMIT $1
                "#,
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?,
        };
        rows.into_iter().map(row_to_triple).collect()
    }
}

/// Column-major decomposition of a triple batch, one parallel `Vec` per
/// `sbol_triples` column, ready to bind as Postgres array parameters for an
/// `UNNEST`-backed insert or delete. Shared by [`TripleRepository::insert_triples`]
/// and [`TripleRepository::delete_triples`] so the term-to-column mapping lives in
/// exactly one place.
struct TripleColumns {
    graph_iri: Vec<Option<String>>,
    subject_iri: Vec<Option<String>>,
    subject_blank: Vec<Option<String>>,
    predicate_iri: Vec<String>,
    object_iri: Vec<Option<String>>,
    object_blank: Vec<Option<String>>,
    object_literal: Vec<Option<String>>,
    datatype_iri: Vec<Option<String>>,
    language: Vec<Option<String>>,
}

impl TripleColumns {
    fn from_triples(triples: &[Triple]) -> Self {
        let mut cols = TripleColumns {
            graph_iri: Vec::with_capacity(triples.len()),
            subject_iri: Vec::with_capacity(triples.len()),
            subject_blank: Vec::with_capacity(triples.len()),
            predicate_iri: Vec::with_capacity(triples.len()),
            object_iri: Vec::with_capacity(triples.len()),
            object_blank: Vec::with_capacity(triples.len()),
            object_literal: Vec::with_capacity(triples.len()),
            datatype_iri: Vec::with_capacity(triples.len()),
            language: Vec::with_capacity(triples.len()),
        };
        for q in triples {
            cols.graph_iri
                .push(q.graph_iri.as_ref().map(|i| i.as_str().to_owned()));
            match &q.subject {
                SubjectTerm::Iri(iri) => {
                    cols.subject_iri.push(Some(iri.as_str().to_owned()));
                    cols.subject_blank.push(None);
                }
                SubjectTerm::BlankNode(node) => {
                    cols.subject_iri.push(None);
                    cols.subject_blank.push(Some(node.clone()));
                }
            }
            cols.predicate_iri.push(q.predicate.as_str().to_owned());
            match &q.object {
                ObjectTerm::Iri(iri) => {
                    cols.object_iri.push(Some(iri.as_str().to_owned()));
                    cols.object_blank.push(None);
                    cols.object_literal.push(None);
                    cols.datatype_iri.push(None);
                    cols.language.push(None);
                }
                ObjectTerm::BlankNode(node) => {
                    cols.object_iri.push(None);
                    cols.object_blank.push(Some(node.clone()));
                    cols.object_literal.push(None);
                    cols.datatype_iri.push(None);
                    cols.language.push(None);
                }
                ObjectTerm::Literal {
                    value,
                    datatype,
                    language: lang,
                } => {
                    cols.object_iri.push(None);
                    cols.object_blank.push(None);
                    cols.object_literal.push(Some(value.clone()));
                    cols.datatype_iri.push(Some(datatype.as_str().to_owned()));
                    cols.language.push(lang.clone());
                }
            }
        }
        cols
    }
}

fn row_to_triple(row: sqlx::postgres::PgRow) -> Result<Triple, DomainError> {
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
