//! Term dictionary encoding.
//!
//! Every RDF term maps to a stable 16-byte id: the leading bytes of its sha3
//! hash. Because the id is derived from the term, interning needs no counter
//! and no reverse-dictionary lookup; writing the same term twice is idempotent.
//! The `id2term` column family maps id back to a reversible byte encoding so
//! reads can materialize full terms from index keys.

use sbol_db_core::{DomainError, IriString, ObjectTerm, SubjectTerm};
use sbol_db_rdf::hash_bytes;

/// Width of a term id in bytes. 128 bits keeps collisions astronomically
/// unlikely even at billions of terms while halving key width versus a full
/// sha3 digest.
pub const ID_LEN: usize = 16;

/// A 16-byte term id, the unit every index key is built from.
pub type TermId = [u8; ID_LEN];

const TAG_NAMED: u8 = 1;
const TAG_BLANK: u8 = 2;
const TAG_LITERAL: u8 = 3;

/// An RDF term in any triple position. The graph position is a [`Term::Named`]
/// for a named graph and is absent (handled structurally) for the default
/// graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    Named(String),
    Blank(String),
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}

impl Term {
    /// The reversible byte encoding stored as the `id2term` value. This is also
    /// the input hashed to derive the term's id, so distinct terms (including a
    /// named node and a literal with the same lexical form) never collide.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Term::Named(s) => {
                let mut out = Vec::with_capacity(1 + s.len());
                out.push(TAG_NAMED);
                out.extend_from_slice(s.as_bytes());
                out
            }
            Term::Blank(s) => {
                let mut out = Vec::with_capacity(1 + s.len());
                out.push(TAG_BLANK);
                out.extend_from_slice(s.as_bytes());
                out
            }
            Term::Literal {
                value,
                datatype,
                language,
            } => {
                let lang = language.as_deref().unwrap_or("");
                let mut out =
                    Vec::with_capacity(1 + 4 + datatype.len() + 1 + 4 + lang.len() + value.len());
                out.push(TAG_LITERAL);
                out.extend_from_slice(&(datatype.len() as u32).to_le_bytes());
                out.extend_from_slice(datatype.as_bytes());
                out.push(language.is_some() as u8);
                out.extend_from_slice(&(lang.len() as u32).to_le_bytes());
                out.extend_from_slice(lang.as_bytes());
                out.extend_from_slice(value.as_bytes());
                out
            }
        }
    }

    /// Decode a term from its `id2term` value.
    pub fn decode(bytes: &[u8]) -> Result<Term, DomainError> {
        let corrupt = || DomainError::Database("corrupt term encoding".into());
        let (&tag, rest) = bytes.split_first().ok_or_else(corrupt)?;
        match tag {
            TAG_NAMED => Ok(Term::Named(utf8(rest)?)),
            TAG_BLANK => Ok(Term::Blank(utf8(rest)?)),
            TAG_LITERAL => {
                let mut cur = rest;
                let dt_len = take_u32(&mut cur)? as usize;
                let datatype = utf8(take(&mut cur, dt_len)?)?;
                let has_lang = take_u8(&mut cur)? != 0;
                let lang_len = take_u32(&mut cur)? as usize;
                let lang = utf8(take(&mut cur, lang_len)?)?;
                let value = utf8(cur)?;
                Ok(Term::Literal {
                    value,
                    datatype,
                    language: has_lang.then_some(lang),
                })
            }
            _ => Err(corrupt()),
        }
    }

    /// This term's content-derived id.
    pub fn id(&self) -> TermId {
        let digest = hash_bytes(&self.encode());
        let mut id = [0u8; ID_LEN];
        id.copy_from_slice(&digest[..ID_LEN]);
        id
    }

    pub fn from_subject(s: &SubjectTerm) -> Term {
        match s {
            SubjectTerm::Iri(iri) => Term::Named(iri.as_str().to_owned()),
            SubjectTerm::BlankNode(node) => Term::Blank(node.clone()),
        }
    }

    pub fn from_object(o: &ObjectTerm) -> Term {
        match o {
            ObjectTerm::Iri(iri) => Term::Named(iri.as_str().to_owned()),
            ObjectTerm::BlankNode(node) => Term::Blank(node.clone()),
            ObjectTerm::Literal {
                value,
                datatype,
                language,
            } => Term::Literal {
                value: value.clone(),
                datatype: datatype.as_str().to_owned(),
                language: language.clone(),
            },
        }
    }

    pub fn named(iri: &str) -> Term {
        Term::Named(iri.to_owned())
    }

    /// Reconstruct a subject from a decoded term. A literal can never be a
    /// subject, so it is rejected as corruption.
    pub fn into_subject(self) -> Result<SubjectTerm, DomainError> {
        match self {
            Term::Named(iri) => Ok(SubjectTerm::Iri(IriString::unchecked(iri))),
            Term::Blank(node) => Ok(SubjectTerm::BlankNode(node)),
            Term::Literal { .. } => {
                Err(DomainError::Database("literal in subject position".into()))
            }
        }
    }

    /// Reconstruct an object from a decoded term.
    pub fn into_object(self) -> ObjectTerm {
        match self {
            Term::Named(iri) => ObjectTerm::Iri(IriString::unchecked(iri)),
            Term::Blank(node) => ObjectTerm::BlankNode(node),
            Term::Literal {
                value,
                datatype,
                language,
            } => ObjectTerm::Literal {
                value,
                datatype: IriString::unchecked(datatype),
                language,
            },
        }
    }

    /// The named-graph IRI a decoded graph term carries.
    pub fn into_graph_iri(self) -> Result<IriString, DomainError> {
        match self {
            Term::Named(iri) => Ok(IriString::unchecked(iri)),
            _ => Err(DomainError::Database("non-IRI in graph position".into())),
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<String, DomainError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| DomainError::Database("non-utf8 term".into()))
}

fn take<'a>(cur: &mut &'a [u8], n: usize) -> Result<&'a [u8], DomainError> {
    if cur.len() < n {
        return Err(DomainError::Database("truncated term encoding".into()));
    }
    let (head, tail) = cur.split_at(n);
    *cur = tail;
    Ok(head)
}

fn take_u8(cur: &mut &[u8]) -> Result<u8, DomainError> {
    Ok(take(cur, 1)?[0])
}

fn take_u32(cur: &mut &[u8]) -> Result<u32, DomainError> {
    let bytes = take(cur, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(term: Term) {
        let encoded = term.encode();
        assert_eq!(Term::decode(&encoded).unwrap(), term);
    }

    #[test]
    fn terms_roundtrip() {
        roundtrip(Term::Named("https://example.org/x".into()));
        roundtrip(Term::Blank("b0".into()));
        roundtrip(Term::Literal {
            value: "hello".into(),
            datatype: "http://www.w3.org/2001/XMLSchema#string".into(),
            language: None,
        });
        roundtrip(Term::Literal {
            value: "bonjour".into(),
            datatype: "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString".into(),
            language: Some("fr".into()),
        });
    }

    #[test]
    fn named_and_literal_with_same_lexical_form_differ() {
        let named = Term::Named("x".into());
        let literal = Term::Literal {
            value: "x".into(),
            datatype: "http://www.w3.org/2001/XMLSchema#string".into(),
            language: None,
        };
        assert_ne!(named.id(), literal.id());
    }

    #[test]
    fn id_is_stable_and_sized() {
        let term = Term::Named("https://example.org/stable".into());
        assert_eq!(term.id(), term.id());
        assert_eq!(term.id().len(), ID_LEN);
    }
}
