//! Ontology storage + transitive-closure builder, plus a small HTTP loader
//! that fetches an OBO file from a canonical URL (OBO Foundry / EBI) and
//! materialises it into `ontologies` / `ontology_terms` /
//! `ontology_term_aliases` / `ontology_closure`.
//!
//! The closure is computed in Rust at load time -- one BFS per term up
//! through `is_a` parents -- so role-expansion queries reduce to one
//! indexed lookup against `ontology_closure.ancestor_iri`.
//!
//! IRI canonicalisation: every term gets its OBO Foundry PURL
//! (`http://purl.obolibrary.org/obo/{PREFIX}_{NUMBER}`) as its canonical
//! IRI plus an identifiers.org alias (`http://identifiers.org/{prefix}/{CURIE}`)
//! since SBOL documents commonly use the latter.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use sbol_db_core::{obo::parse_obo, DomainError};
use serde::Serialize;
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct OntologyRepository {
    pool: PgPool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OntologyRecord {
    pub prefix: String,
    pub name: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub term_count: i32,
    pub imported_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OntologyTermRecord {
    pub iri: String,
    pub prefix: String,
    pub curie: String,
    pub name: String,
    pub definition: Option<String>,
    pub is_obsolete: bool,
    pub synonyms: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OntologyLoadReport {
    pub prefix: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub term_count: usize,
    pub closure_count: usize,
    pub alias_count: usize,
}

impl OntologyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch an OBO file from `source_url`, parse it, and persist the
    /// ontology + terms + closure in a single transaction. `prefix` (e.g.
    /// "SO", "SBO") drives IRI generation and is the row key in
    /// `ontologies`.
    pub async fn load_from_url(
        &self,
        prefix: &str,
        name: &str,
        source_url: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let client = reqwest::Client::builder()
            .user_agent("sbol-db/0.1 (+https://github.com/marpaia/sbol-db)")
            .build()
            .map_err(|e| DomainError::InvalidInput(format!("reqwest client: {e}")))?;
        let body = client
            .get(source_url)
            .send()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("fetch {source_url}: {e}")))?
            .error_for_status()
            .map_err(|e| DomainError::InvalidInput(format!("HTTP {source_url}: {e}")))?
            .text()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("decode {source_url}: {e}")))?;
        self.load_from_text(prefix, name, Some(source_url), &body)
            .await
    }

    /// Same as [`load_from_url`] but takes already-fetched OBO text.
    pub async fn load_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let parsed = parse_obo(text);
        let prefix_upper = prefix.to_ascii_uppercase();
        let prefix_lower = prefix.to_ascii_lowercase();
        let version = parsed.data_version.clone();

        // Build canonical IRIs for every term, including alt_ids as aliases.
        let mut terms: Vec<MaterialisedTerm> = Vec::with_capacity(parsed.terms.len());
        let mut curie_to_canonical: HashMap<String, String> = HashMap::new();
        for t in &parsed.terms {
            if !t.curie.starts_with(&format!("{prefix_upper}:")) {
                continue;
            }
            let canonical = curie_to_iri(&prefix_upper, &t.curie);
            curie_to_canonical.insert(t.curie.clone(), canonical.clone());
            terms.push(MaterialisedTerm {
                canonical_iri: canonical,
                curie: t.curie.clone(),
                name: t.name.clone(),
                definition: t.definition.clone(),
                is_obsolete: t.is_obsolete,
                parents: t.parents.clone(),
                alt_ids: t.alt_ids.clone(),
                synonyms: t.synonyms.clone(),
            });
        }
        // alt_ids also resolve to the same canonical IRI -- record them so
        // queries for the alt CURIE can resolve back to the parent term.
        for t in &terms {
            for alt in &t.alt_ids {
                curie_to_canonical
                    .entry(alt.clone())
                    .or_insert_with(|| t.canonical_iri.clone());
            }
        }
        // Compute closure: (ancestor → descendant). Both must be in-ontology.
        // For each term, BFS up through parents and record every ancestor.
        let mut closure_pairs: HashSet<(String, String, i16)> = HashSet::new();
        // parent map: canonical_iri → Vec<canonical_iri parents>
        let mut parent_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for t in &terms {
            let parents: Vec<&str> = t
                .parents
                .iter()
                .filter_map(|p| curie_to_canonical.get(p.as_str()).map(|s| s.as_str()))
                .collect();
            parent_map.insert(t.canonical_iri.as_str(), parents);
        }
        for t in &terms {
            closure_pairs.insert((t.canonical_iri.clone(), t.canonical_iri.clone(), 0));
            // BFS upward.
            let mut visited: HashSet<&str> = HashSet::new();
            visited.insert(t.canonical_iri.as_str());
            let mut frontier: VecDeque<(&str, i16)> = VecDeque::new();
            frontier.push_back((t.canonical_iri.as_str(), 0));
            while let Some((cur, depth)) = frontier.pop_front() {
                if depth > 1024 {
                    break;
                }
                let Some(parents) = parent_map.get(cur) else {
                    continue;
                };
                for p in parents {
                    if visited.insert(p) {
                        closure_pairs.insert(((*p).to_owned(), t.canonical_iri.clone(), depth + 1));
                        frontier.push_back((p, depth + 1));
                    }
                }
            }
        }

        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // Replace any previous ontology of this prefix.
        sqlx::query("DELETE FROM ontologies WHERE prefix = $1")
            .bind(&prefix_upper)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        sqlx::query(
            r#"
            INSERT INTO ontologies (prefix, name, source_url, version, term_count, imported_at)
            VALUES ($1, $2, $3, $4, $5, now())
            "#,
        )
        .bind(&prefix_upper)
        .bind(name)
        .bind(source_url)
        .bind(&version)
        .bind(terms.len() as i32)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        // Per-term insert. Synonyms are text[]; Postgres rejects ragged
        // multidim arrays, so we cannot UNNEST text[][] safely -- and an
        // OBO file is a few thousand rows, well within row-by-row insert
        // throughput inside one transaction.
        for t in &terms {
            sqlx::query(
                r#"
                INSERT INTO ontology_terms
                    (iri, prefix, curie, name, definition, is_obsolete, synonyms)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(&t.canonical_iri)
            .bind(&prefix_upper)
            .bind(&t.curie)
            .bind(&t.name)
            .bind(&t.definition)
            .bind(t.is_obsolete)
            .bind(&t.synonyms)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        // Aliases: identifiers.org form + alt_ids.
        let mut alias_iri: Vec<String> = Vec::new();
        let mut alias_canon: Vec<String> = Vec::new();
        let mut alias_seen: BTreeSet<String> = BTreeSet::new();
        for t in &terms {
            // identifiers.org variant
            let identif = format!("http://identifiers.org/{prefix_lower}/{}", t.curie);
            if identif != t.canonical_iri && alias_seen.insert(identif.clone()) {
                alias_iri.push(identif);
                alias_canon.push(t.canonical_iri.clone());
            }
            for alt in &t.alt_ids {
                if alt.starts_with(&format!("{prefix_upper}:")) {
                    let alt_iri = curie_to_iri(&prefix_upper, alt);
                    if alt_iri != t.canonical_iri && alias_seen.insert(alt_iri.clone()) {
                        alias_iri.push(alt_iri);
                        alias_canon.push(t.canonical_iri.clone());
                    }
                    let alt_identif = format!("http://identifiers.org/{prefix_lower}/{alt}");
                    if alt_identif != t.canonical_iri && alias_seen.insert(alt_identif.clone()) {
                        alias_iri.push(alt_identif);
                        alias_canon.push(t.canonical_iri.clone());
                    }
                }
            }
        }
        let alias_count = alias_iri.len();
        if !alias_iri.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO ontology_term_aliases (alias_iri, canonical_iri)
                SELECT alias, canonical
                FROM UNNEST($1::text[], $2::text[]) AS u(alias, canonical)
                ON CONFLICT (alias_iri) DO UPDATE
                  SET canonical_iri = EXCLUDED.canonical_iri
                "#,
            )
            .bind(&alias_iri)
            .bind(&alias_canon)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        // Bulk insert closure.
        let mut anc: Vec<String> = Vec::with_capacity(closure_pairs.len());
        let mut des: Vec<String> = Vec::with_capacity(closure_pairs.len());
        let mut dep: Vec<i16> = Vec::with_capacity(closure_pairs.len());
        for (a, d, depth) in &closure_pairs {
            anc.push(a.clone());
            des.push(d.clone());
            dep.push(*depth);
        }
        if !anc.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO ontology_closure (ancestor_iri, descendant_iri, depth)
                SELECT ancestor, descendant, depth
                FROM UNNEST($1::text[], $2::text[], $3::int2[])
                  AS u(ancestor, descendant, depth)
                "#,
            )
            .bind(&anc)
            .bind(&des)
            .bind(&dep)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(OntologyLoadReport {
            prefix: prefix_upper,
            source_url: source_url.map(|s| s.to_owned()),
            version,
            term_count: terms.len(),
            closure_count: closure_pairs.len(),
            alias_count,
        })
    }

    pub async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT prefix, name, source_url, version, term_count, imported_at
            FROM ontologies
            ORDER BY prefix
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(OntologyRecord {
                prefix: row.try_get("prefix").map_err(db_err)?,
                name: row.try_get("name").map_err(db_err)?,
                source_url: row.try_get("source_url").map_err(db_err)?,
                version: row.try_get("version").map_err(db_err)?,
                term_count: row.try_get("term_count").map_err(db_err)?,
                imported_at: row.try_get("imported_at").map_err(db_err)?,
            });
        }
        Ok(out)
    }

    /// Resolve any input IRI (canonical or alias) to a canonical term IRI.
    pub async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT iri::text AS canonical
            FROM ontology_terms WHERE iri = $1
            UNION ALL
            SELECT canonical_iri::text AS canonical
            FROM ontology_term_aliases WHERE alias_iri = $1
            LIMIT 1
            "#,
        )
        .bind(iri)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(match row {
            Some(r) => Some(r.try_get("canonical").map_err(db_err)?),
            None => None,
        })
    }

    /// Return the term plus every descendant IRI under it (transitively).
    /// The root itself is included with depth 0.
    pub async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let rows = sqlx::query(
            r#"
            SELECT descendant_iri::text AS iri, depth
            FROM ontology_closure
            WHERE ancestor_iri = $1
            ORDER BY depth ASC, descendant_iri ASC
            "#,
        )
        .bind(&canonical)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push((
                row.try_get::<String, _>("iri").map_err(db_err)?,
                row.try_get::<i16, _>("depth").map_err(db_err)?,
            ));
        }
        Ok(out)
    }

    pub async fn get_term(&self, iri: &str) -> Result<Option<OntologyTermRecord>, DomainError> {
        let canonical = self
            .canonicalize(iri)
            .await?
            .unwrap_or_else(|| iri.to_owned());
        let row = sqlx::query(
            r#"
            SELECT iri::text AS iri, prefix, curie, name, definition,
                   is_obsolete, synonyms
            FROM ontology_terms WHERE iri = $1
            "#,
        )
        .bind(&canonical)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(match row {
            Some(r) => Some(OntologyTermRecord {
                iri: r.try_get("iri").map_err(db_err)?,
                prefix: r.try_get("prefix").map_err(db_err)?,
                curie: r.try_get("curie").map_err(db_err)?,
                name: r.try_get("name").map_err(db_err)?,
                definition: r.try_get("definition").map_err(db_err)?,
                is_obsolete: r.try_get("is_obsolete").map_err(db_err)?,
                synonyms: r.try_get("synonyms").map_err(db_err)?,
            }),
            None => None,
        })
    }
}

struct MaterialisedTerm {
    canonical_iri: String,
    curie: String,
    name: String,
    definition: Option<String>,
    is_obsolete: bool,
    parents: Vec<String>,
    alt_ids: Vec<String>,
    synonyms: Vec<String>,
}

fn curie_to_iri(prefix_upper: &str, curie: &str) -> String {
    // SO:0000167 -> http://purl.obolibrary.org/obo/SO_0000167
    let suffix = curie
        .strip_prefix(&format!("{prefix_upper}:"))
        .unwrap_or(curie);
    format!("http://purl.obolibrary.org/obo/{prefix_upper}_{suffix}")
}
