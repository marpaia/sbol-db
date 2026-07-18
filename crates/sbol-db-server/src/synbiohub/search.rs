//! The classic SynBioHub `/search` path-grammar parser.
//!
//! Classic SynBioHub encodes a faceted query in a single path segment,
//! `/search/<key>=<value>&<key>=<value>&.../<free text>`, and expands it into a
//! SPARQL `criteria` block in `lib/search.js`'s `lucene()`. This module is the
//! quarantine for that wire quirk: it parses the grammar into the facade's typed
//! [`FacetedSearch`], which is all that reaches `AppServices`. The SPARQL text
//! and the ranking never see the raw path.

use sbol_db_app::{AlignMode, DateField, FacetedSearch};

use crate::ApiError;

/// A sequence-search request extracted from the classic `/search` grammar: the
/// query nucleotide string and the alignment mode it selects.
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceQuery {
    pub sequence: String,
    pub mode: AlignMode,
}

/// Detect a sequence-search facet in the classic `/search` grammar. `sequence=`
/// and `globalsequence=` run the banded global aligner (vsearch
/// `usearch_global`); `exactsequence=` takes the exact substring path (vsearch
/// `--search_exact`). Returns `None` for an ordinary faceted or free-text query,
/// which the caller then answers through the SPARQL / ranked path.
pub fn extract_sequence(path: &str) -> Option<SequenceQuery> {
    for facet in path.split('&') {
        let Some((key, value)) = facet.split_once('=') else {
            continue;
        };
        let mode = match key {
            "sequence" | "globalsequence" => AlignMode::GlobalAlign,
            "exactsequence" => AlignMode::Exact,
            _ => continue,
        };
        if value.is_empty() {
            continue;
        }
        return Some(SequenceQuery {
            sequence: value.to_owned(),
            mode,
        });
    }
    None
}

/// The SBOL2 namespace, the default for a bare `objectType` short name and a
/// bare predicate key.
const SBOL2_NS: &str = "http://sbols.org/v2#";

/// Parse a classic `/search` path segment into a typed [`FacetedSearch`].
///
/// `path` is the already URL-decoded segment after `/search/` (or `""` for a
/// bare `/search`). Paging is not part of the grammar; the caller sets
/// `offset`/`limit` from the query string.
///
/// The grammar, mirroring `lucene()`:
/// - `objectType=<X>` sets the class, defaulting a bare short name to `sbol2:`.
/// - a key containing `:` or a full-IRI key is a predicate-equality facet.
/// - `collection=<uri>` requires membership in that collection.
/// - `createdBefore`/`createdAfter`/`modifiedBefore`/`modifiedAfter` set the
///   date range.
/// - a bare key becomes an `sbol2:<key>` predicate facet.
/// - the trailing segment is the free text, with standalone `and`/`or`/`not`
///   operators dropped.
pub fn parse_search_path(path: &str) -> Result<FacetedSearch, ApiError> {
    let mut search = FacetedSearch::default();

    if path.is_empty() {
        return Ok(search);
    }

    // A `key=value` segment is a facet; only the trailing segment may be bare
    // free text. Classifying by `=` presence (rather than assuming the last
    // segment is always free text) keeps a facet-only query like
    // `objectType=ComponentDefinition` a facet instead of misreading it as a
    // free-text term, which would drop the type filter.
    let parts: Vec<&str> = path.split('&').filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        if part.contains('=') {
            parse_facet(part, &mut search)?;
        } else if i == parts.len() - 1 {
            search.free_text = parse_free_text(part);
        } else {
            return Err(ApiError::BadRequest(format!(
                "search facet '{part}' is not a key=value pair"
            )));
        }
    }

    Ok(search)
}

/// Parse one `key=value` facet segment into `search`.
fn parse_facet(facet: &str, search: &mut FacetedSearch) -> Result<(), ApiError> {
    let (key, value) = facet.split_once('=').ok_or_else(|| {
        ApiError::BadRequest(format!("search facet '{facet}' is not a key=value pair"))
    })?;
    if key.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "search facet '{facet}' has an empty key"
        )));
    }

    match key {
        "objectType" => search.class = Some(object_type_iri(value)),
        "collection" => search.collection_member = Some(value.to_owned()),
        "createdBefore" => {
            search.date_field = Some(DateField::Created);
            search.date_before = Some(value.to_owned());
        }
        "createdAfter" => {
            search.date_field = Some(DateField::Created);
            search.date_after = Some(value.to_owned());
        }
        "modifiedBefore" => {
            search.date_field = Some(DateField::Modified);
            search.date_before = Some(value.to_owned());
        }
        "modifiedAfter" => {
            search.date_field = Some(DateField::Modified);
            search.date_after = Some(value.to_owned());
        }
        // A key with a prefix separator or a full IRI is a predicate facet
        // as-is; a bare key is an `sbol2:` predicate.
        _ if key.contains(':') => search.predicate_eq.push((key.to_owned(), value.to_owned())),
        _ => search
            .predicate_eq
            .push((format!("sbol2:{key}"), value.to_owned())),
    }
    Ok(())
}

/// Resolve an `objectType` value to a full rdf:type IRI: a bare short name
/// (`ComponentDefinition`) becomes `sbol2:ComponentDefinition`; an `sbol2:`
/// prefixed name expands to the SBOL2 namespace; a full IRI passes through.
fn object_type_iri(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("sbol2:") {
        format!("{SBOL2_NS}{rest}")
    } else if value.starts_with("http://") || value.starts_with("https://") {
        value.to_owned()
    } else if value.contains(':') {
        // A curie in another prefix; keep it verbatim for the SPARQL path,
        // which resolves the prefix, though the ranked class filter cannot.
        value.to_owned()
    } else {
        format!("{SBOL2_NS}{value}")
    }
}

/// Reduce the free-text segment to the search terms, dropping the standalone
/// `and`/`or`/`not` boolean operators the classic FILTER grammar carried.
/// Returns `None` when nothing but operators (or whitespace) remains.
fn parse_free_text(segment: &str) -> Option<String> {
    let terms: Vec<&str> = segment
        .split_whitespace()
        .filter(|token| !matches!(*token, "and" | "or" | "not"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_type_predicate_and_free_text() {
        let search = parse_search_path(
            "objectType=ComponentDefinition&sbol2:role=http://identifiers.org/so/SO:0000167&promoter",
        )
        .expect("parse");
        assert_eq!(
            search.class.as_deref(),
            Some("http://sbols.org/v2#ComponentDefinition")
        );
        assert_eq!(
            search.predicate_eq,
            vec![(
                "sbol2:role".to_owned(),
                "http://identifiers.org/so/SO:0000167".to_owned()
            )]
        );
        assert_eq!(search.free_text.as_deref(), Some("promoter"));
    }

    #[test]
    fn bare_segment_is_only_free_text() {
        let search = parse_search_path("plasmid").expect("parse");
        assert!(search.class.is_none());
        assert!(search.predicate_eq.is_empty());
        assert_eq!(search.free_text.as_deref(), Some("plasmid"));
    }

    #[test]
    fn facet_only_query_keeps_the_facet() {
        // A single `objectType=` segment with no trailing free text is a facet,
        // not a free-text term: the class filter must be set and free text empty.
        let search = parse_search_path("objectType=ComponentDefinition").expect("parse");
        assert_eq!(
            search.class.as_deref(),
            Some("http://sbols.org/v2#ComponentDefinition")
        );
        assert!(search.free_text.is_none());
    }

    #[test]
    fn bare_key_becomes_sbol2_predicate() {
        let search = parse_search_path("displayId=BBa_J23100&").expect("parse");
        assert_eq!(
            search.predicate_eq,
            vec![("sbol2:displayId".to_owned(), "BBa_J23100".to_owned())]
        );
        assert!(search.free_text.is_none());
    }

    #[test]
    fn collection_and_dates() {
        let search = parse_search_path("collection=http://example.org/c&createdAfter=2020-01-01&")
            .expect("parse");
        assert_eq!(
            search.collection_member.as_deref(),
            Some("http://example.org/c")
        );
        assert_eq!(search.date_field, Some(DateField::Created));
        assert_eq!(search.date_after.as_deref(), Some("2020-01-01"));
    }

    #[test]
    fn boolean_operators_are_dropped_from_free_text() {
        let search = parse_search_path("gfp or rfp").expect("parse");
        assert_eq!(search.free_text.as_deref(), Some("gfp rfp"));
    }

    #[test]
    fn empty_path_matches_everything() {
        let search = parse_search_path("").expect("parse");
        assert!(search.class.is_none());
        assert!(search.free_text.is_none());
        assert!(search.predicate_eq.is_empty());
    }

    #[test]
    fn malformed_facet_is_an_error_not_a_panic() {
        assert!(parse_search_path("objectType&promoter").is_err());
        assert!(parse_search_path("=value&promoter").is_err());
    }
}
