//! The graph-owned triplestore over RocksDB's permuted indexes.
//!
//! A triple is interned into the term dictionary and written to every relevant
//! permuted index in one write batch; the index key *is* the triple, so a
//! re-insert is a no-op and set semantics hold without a uniqueness check.
//! Pattern scans pick the single index whose key orders the bound positions
//! first, turning the match into one prefix range scan.

use std::collections::HashSet;

use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, IriString, Triple};
use sbol_db_storage::{GraphFilter, PatternObject, PatternSubject};

use crate::codec::{Term, TermId};
use crate::db::Db;
use crate::keys::{self, Index, Quad, DOSP, DPOS, DSPO, GOSP, GPOS, GSPO, OSPG, POSG, SPOG};

#[derive(Clone)]
pub struct TripleRepository {
    db: Db,
}

impl TripleRepository {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The four ids and terms of a triple, ready to intern and index.
    fn decompose(triple: &Triple) -> ([(TermId, Term); 4], Quad, usize) {
        let s = Term::from_subject(&triple.subject);
        let p = Term::named(triple.predicate.as_str());
        let o = Term::from_object(&triple.object);
        let g = triple.graph_iri.as_ref().map(|i| Term::named(i.as_str()));

        let sid = s.id();
        let pid = p.id();
        let oid = o.id();
        let gid = g.as_ref().map(Term::id);

        let mut terms = [
            (sid, s),
            (pid, p),
            (oid, o),
            (
                gid.unwrap_or([0u8; 16]),
                g.unwrap_or(Term::Named(String::new())),
            ),
        ];
        let term_count = if gid.is_some() { 4 } else { 3 };
        // Keep only the live terms first; callers intern `&terms[..count]`.
        if gid.is_none() {
            terms[3] = terms[2].clone();
        }

        let quad = Quad {
            g: gid,
            s: sid,
            p: pid,
            o: oid,
        };
        (terms, quad, term_count)
    }

    fn indexes_for(quad: &Quad) -> &'static [Index] {
        if quad.g.is_some() {
            &keys::NAMED_INDEXES
        } else {
            &keys::DEFAULT_INDEXES
        }
    }

    /// The canonical index whose presence decides set membership.
    fn primary(quad: &Quad) -> Index {
        if quad.g.is_some() {
            SPOG
        } else {
            DSPO
        }
    }

    /// Stage a batch of inserts, skipping triples already present (set
    /// semantics) and those repeated within this batch. Returns the count
    /// actually staged.
    pub fn stage_insert(
        &self,
        batch: &mut WriteBatch,
        seen: &mut HashSet<Vec<u8>>,
        triples: &[Triple],
    ) -> Result<usize, DomainError> {
        let mut inserted = 0usize;
        for triple in triples {
            let (terms, quad, count) = Self::decompose(triple);
            let primary_key = quad.key(Self::primary(&quad));

            if !seen.insert(primary_key.clone()) {
                continue;
            }
            if self.db.exists_cf(Self::primary(&quad).cf, &primary_key)? {
                continue;
            }

            let id2term = self.db.cf("id2term");
            for (id, term) in &terms[..count] {
                batch.put_cf(&id2term, id, term.encode());
            }
            for index in Self::indexes_for(&quad) {
                batch.put_cf(&self.db.cf(index.cf), quad.key(*index), []);
            }
            inserted += 1;
        }
        Ok(inserted)
    }

    /// Stage deletion of every triple matching one of `triples` on all RDF
    /// positions. Returns the count actually present and deleted.
    pub fn stage_delete(
        &self,
        batch: &mut WriteBatch,
        triples: &[Triple],
    ) -> Result<usize, DomainError> {
        let mut deleted = 0usize;
        for triple in triples {
            let (_, quad, _) = Self::decompose(triple);
            let primary_key = quad.key(Self::primary(&quad));
            if !self.db.exists_cf(Self::primary(&quad).cf, &primary_key)? {
                continue;
            }
            for index in Self::indexes_for(&quad) {
                batch.delete_cf(&self.db.cf(index.cf), quad.key(*index));
            }
            deleted += 1;
        }
        Ok(deleted)
    }

    /// Stage deletion of every triple in a named graph, or the default
    /// partition when `graph` is `None`. Returns the count deleted.
    pub fn stage_clear_graph(
        &self,
        batch: &mut WriteBatch,
        graph: Option<&str>,
    ) -> Result<usize, DomainError> {
        match graph {
            Some(g) => {
                let gid = Term::named(g).id();
                self.stage_delete_named_graph(batch, gid)
            }
            None => {
                // Every default-graph triple lives in the three d-indexes.
                let mut quads = Vec::new();
                self.db.for_each(DSPO.cf, |key, _| {
                    quads.push(keys::decode_key(DSPO, key));
                    Ok(true)
                })?;
                for quad in &quads {
                    for index in keys::DEFAULT_INDEXES {
                        batch.delete_cf(&self.db.cf(index.cf), quad.key(index));
                    }
                }
                Ok(quads.len())
            }
        }
    }

    /// Stage deletion of every triple in one named graph, found by prefix-scan
    /// over `gspo`. Returns the count deleted.
    pub fn stage_delete_named_graph(
        &self,
        batch: &mut WriteBatch,
        gid: TermId,
    ) -> Result<usize, DomainError> {
        let mut quads = Vec::new();
        self.db.for_each_prefix(GSPO.cf, &gid, |key, _| {
            quads.push(keys::decode_key(GSPO, key));
            Ok(true)
        })?;
        for quad in &quads {
            for index in keys::NAMED_INDEXES {
                batch.delete_cf(&self.db.cf(index.cf), quad.key(index));
            }
        }
        Ok(quads.len())
    }

    pub async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let db = self.clone();
        let subject = PatternSubject::Iri(subject_iri.to_owned());
        tokio::task::spawn_blocking(move || {
            db.scan_pattern(Some(&subject), None, None, None, i64::MAX)
        })
        .await
        .map_err(join_err)?
    }

    pub async fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let db = self.clone();
        let filter = graph.map(|g| GraphFilter::Iri(g.to_owned()));
        let filter = filter.or(Some(GraphFilter::DefaultOnly));
        tokio::task::spawn_blocking(move || {
            db.scan_pattern(None, None, None, filter.as_ref(), limit)
        })
        .await
        .map_err(join_err)?
    }

    pub fn distinct_named_graphs_blocking(&self) -> Result<Vec<String>, DomainError> {
        let mut out = Vec::new();
        let mut last: Option<TermId> = None;
        self.for_each_distinct_graph(&mut |gid| {
            if last != Some(gid) {
                last = Some(gid);
                let term = self.db.resolve_term(&gid)?;
                out.push(term.into_graph_iri()?.into_inner());
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn for_each_distinct_graph(
        &self,
        f: &mut dyn FnMut(TermId) -> Result<(), DomainError>,
    ) -> Result<(), DomainError> {
        self.db.for_each(GSPO.cf, |key, _| {
            f(keys::id_at(key, 0))?;
            Ok(true)
        })
    }

    /// Synchronous pattern scan, used directly by the SPARQL evaluator's
    /// blocking task and by the async helpers above.
    pub fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let sid = subject.map(subject_id);
        let pid = predicate.map(|p| Term::named(p).id());
        let oid = object.map(object_id);
        let gid = match graph {
            Some(GraphFilter::Iri(g)) => Some(Term::named(g).id()),
            _ => None,
        };

        let plans = plan_scans(sid, pid, oid, graph, gid);
        let mut out = Vec::new();
        let limit = if limit < 0 { i64::MAX } else { limit };

        for (index, prefix) in plans {
            if out.len() as i64 >= limit {
                break;
            }
            self.db.for_each_prefix(index.cf, &prefix, |key, _| {
                let quad = keys::decode_key(index, key);
                out.push(self.materialize(&quad)?);
                Ok((out.len() as i64) < limit)
            })?;
        }
        Ok(out)
    }

    fn materialize(&self, quad: &Quad) -> Result<Triple, DomainError> {
        let subject = self.db.resolve_term(&quad.s)?.into_subject()?;
        let predicate = match self.db.resolve_term(&quad.p)? {
            Term::Named(iri) => IriString::unchecked(iri),
            _ => {
                return Err(DomainError::Database(
                    "non-IRI in predicate position".into(),
                ))
            }
        };
        let object = self.db.resolve_term(&quad.o)?.into_object();
        let graph_iri = match quad.g {
            Some(gid) => Some(self.db.resolve_term(&gid)?.into_graph_iri()?),
            None => None,
        };
        Ok(Triple {
            graph_iri,
            subject,
            predicate,
            object,
        })
    }
}

fn subject_id(s: &PatternSubject) -> TermId {
    match s {
        PatternSubject::Iri(iri) => Term::Named(iri.clone()).id(),
        PatternSubject::Blank(b) => Term::Blank(b.clone()).id(),
    }
}

fn object_id(o: &PatternObject) -> TermId {
    match o {
        PatternObject::Iri(iri) => Term::Named(iri.clone()).id(),
        PatternObject::Blank(b) => Term::Blank(b.clone()).id(),
        PatternObject::Literal {
            value,
            datatype,
            language,
        } => Term::Literal {
            value: value.clone(),
            datatype: datatype.clone(),
            language: language.clone(),
        }
        .id(),
    }
}

/// Pick the index and prefix for each scan a pattern needs. With no graph
/// filter, both the default-graph and any-named branches run; otherwise a
/// single branch does.
fn plan_scans(
    s: Option<TermId>,
    p: Option<TermId>,
    o: Option<TermId>,
    graph: Option<&GraphFilter>,
    gid: Option<TermId>,
) -> Vec<(Index, Vec<u8>)> {
    match graph {
        Some(GraphFilter::DefaultOnly) => vec![default_scan(s, p, o)],
        Some(GraphFilter::AnyNamed) => vec![any_named_scan(s, p, o)],
        Some(GraphFilter::Iri(_)) => {
            vec![named_graph_scan(
                gid.expect("Iri graph filter without id"),
                s,
                p,
                o,
            )]
        }
        None => vec![default_scan(s, p, o), any_named_scan(s, p, o)],
    }
}

/// Default-graph index choice: the bound positions among S/P/O always form a
/// prefix of one of the three d-indexes.
fn default_scan(s: Option<TermId>, p: Option<TermId>, o: Option<TermId>) -> (Index, Vec<u8>) {
    match (s, p, o) {
        (Some(s), Some(p), Some(o)) => (DSPO, keys::prefix(&[s, p, o])),
        (Some(s), Some(p), None) => (DSPO, keys::prefix(&[s, p])),
        (Some(s), None, Some(o)) => (DOSP, keys::prefix(&[o, s])),
        (Some(s), None, None) => (DSPO, keys::prefix(&[s])),
        (None, Some(p), Some(o)) => (DPOS, keys::prefix(&[p, o])),
        (None, Some(p), None) => (DPOS, keys::prefix(&[p])),
        (None, None, Some(o)) => (DOSP, keys::prefix(&[o])),
        (None, None, None) => (DSPO, Vec::new()),
    }
}

/// Any-named index choice: g floats as the key suffix, so bound S/P/O lead.
fn any_named_scan(s: Option<TermId>, p: Option<TermId>, o: Option<TermId>) -> (Index, Vec<u8>) {
    match (s, p, o) {
        (Some(s), Some(p), Some(o)) => (SPOG, keys::prefix(&[s, p, o])),
        (Some(s), Some(p), None) => (SPOG, keys::prefix(&[s, p])),
        (Some(s), None, Some(o)) => (OSPG, keys::prefix(&[o, s])),
        (Some(s), None, None) => (SPOG, keys::prefix(&[s])),
        (None, Some(p), Some(o)) => (POSG, keys::prefix(&[p, o])),
        (None, Some(p), None) => (POSG, keys::prefix(&[p])),
        (None, None, Some(o)) => (OSPG, keys::prefix(&[o])),
        (None, None, None) => (SPOG, Vec::new()),
    }
}

/// Specific-named-graph index choice: g leads, then bound S/P/O form a prefix.
fn named_graph_scan(
    g: TermId,
    s: Option<TermId>,
    p: Option<TermId>,
    o: Option<TermId>,
) -> (Index, Vec<u8>) {
    match (s, p, o) {
        (Some(s), Some(p), Some(o)) => (GSPO, keys::prefix(&[g, s, p, o])),
        (Some(s), Some(p), None) => (GSPO, keys::prefix(&[g, s, p])),
        (Some(s), None, Some(o)) => (GOSP, keys::prefix(&[g, o, s])),
        (Some(s), None, None) => (GSPO, keys::prefix(&[g, s])),
        (None, Some(p), Some(o)) => (GPOS, keys::prefix(&[g, p, o])),
        (None, Some(p), None) => (GPOS, keys::prefix(&[g, p])),
        (None, None, Some(o)) => (GOSP, keys::prefix(&[g, o])),
        (None, None, None) => (GSPO, keys::prefix(&[g])),
    }
}

fn join_err(e: tokio::task::JoinError) -> DomainError {
    DomainError::Database(format!("rocksdb task panicked: {e}"))
}
