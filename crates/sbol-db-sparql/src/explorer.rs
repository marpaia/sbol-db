//! Recognizes the query shapes SynBioHub sends when it delegates ranked search,
//! `/similar`, and sequence search to SBOLExplorer, and describes the exact
//! SPARQL-results JSON that service returns.
//!
//! SBOLExplorer impersonates a Virtuoso SPARQL endpoint: SynBioHub smuggles
//! intent through SPARQL comment markers (`# SIMILAR:<uri>`, `# flag_*`) and a
//! fixed `search.sparql` body, and SBOLExplorer answers from its own indexes
//! rather than evaluating the SPARQL. Recognizing the same shapes lets sbol-db
//! stand in for SBOLExplorer over its native REST search.
//!
//! Recognition is textual, mirroring SBOLExplorer's own regex extraction: the
//! markers are SPARQL comments a parser discards, so the raw query string is
//! inspected here rather than the parsed algebra. A query that matches none of
//! the shapes returns `None`, and the caller evaluates it normally, so
//! correctness never depends on a match.

use serde_json::{json, Value};

/// The ordered `head.vars` of a recognized search or sequence-search result,
/// identical to the projection classic SynBioHub receives from SBOLExplorer.
pub const SEARCH_VARS: [&str; 9] = [
    "subject",
    "displayId",
    "version",
    "name",
    "description",
    "type",
    "percentMatch",
    "strandAlignment",
    "CIGAR",
];

/// The single `head.vars` of a recognized count result.
pub const COUNT_VARS: [&str; 1] = ["count"];

/// Paging and count flag shared by every recognized shape. `count` is set when
/// the query is the `searchCount.sparql` aggregate rather than the row listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paging {
    pub offset: usize,
    pub limit: usize,
    pub count: bool,
}

/// A recognized SBOLExplorer request extracted from a SPARQL query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerQuery {
    /// `# SIMILAR:<uri>`: the target's cluster mates, ranked by PageRank alone.
    Similar { uri: String, paging: Paging },
    /// `?seq sbol2:elements "<seq>"` with optional `# flag_*` markers: align
    /// `sequence` against the indexed sequences. `exact` mirrors the
    /// `flag_search_exact` marker classic emits for the `sequence=` facet (an
    /// exact match); its absence is the global-alignment `globalsequence=`.
    Sequence {
        sequence: String,
        exact: bool,
        paging: Paging,
    },
    /// The ranked free-text shape: one or more
    /// `CONTAINS(lcase(?displayId), lcase('<term>'))` filters. `terms` are the
    /// keywords in order; the caller joins them for the ranked-text engine.
    Text { terms: Vec<String>, paging: Paging },
}

/// Recognize an SBOLExplorer query shape in the raw SPARQL string, or `None`
/// for anything the caller should evaluate normally.
///
/// Precedence follows SBOLExplorer's `search`: a literal sequence wins over the
/// `# SIMILAR:` marker, which wins over free-text keywords. The `# USES` /
/// `# TWINS` advanced-search markers carry no sequence literal, similar marker,
/// or `?displayId` keyword, so they match nothing here and fall through to
/// generic evaluation over the real triples.
pub fn recognize(query: &str) -> Option<ExplorerQuery> {
    let paging = Paging {
        offset: extract_uint_after(query, "OFFSET").unwrap_or(0),
        limit: extract_uint_after(query, "LIMIT").unwrap_or(DEFAULT_LIMIT),
        count: is_count(query),
    };

    if let Some(sequence) = extract_sequence(query) {
        return Some(ExplorerQuery::Sequence {
            sequence,
            exact: query.contains("flag_search_exact"),
            paging,
        });
    }
    if let Some(uri) = extract_similar(query) {
        return Some(ExplorerQuery::Similar { uri, paging });
    }
    let terms = extract_keywords(query);
    if !terms.is_empty() {
        return Some(ExplorerQuery::Text { terms, paging });
    }
    None
}

/// SBOLExplorer's default page size when the query carries no `LIMIT`.
const DEFAULT_LIMIT: usize = 50;

/// The SPARQL-results JSON envelope for a recognized search/sequence result:
/// the exact [`SEARCH_VARS`] head over the caller's binding rows.
pub fn search_results(bindings: Vec<Value>) -> Value {
    json!({
        "head": { "vars": SEARCH_VARS },
        "results": { "bindings": bindings },
    })
}

/// The single-row `[{count}]` SPARQL-results JSON envelope for a recognized
/// count, with the exact [`COUNT_VARS`] head.
pub fn count_results(count: usize) -> Value {
    json!({
        "head": { "vars": COUNT_VARS },
        "results": {
            "bindings": [{
                "count": {
                    "type": "literal",
                    "value": count.to_string(),
                    "datatype": "http://www.w3.org/2001/XMLSchema#integer",
                },
            }],
        },
    })
}

/// Whether the query is the `searchCount.sparql` aggregate. SBOLExplorer keys on
/// the substring `SELECT (count(distinct`; this matches it independent of
/// whitespace and case.
fn is_count(query: &str) -> bool {
    let compact: String = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("select(count(distinct")
}

/// The first unsigned integer following a keyword (`OFFSET`/`LIMIT`), matched
/// case-insensitively. Skips occurrences not followed by digits.
fn extract_uint_after(query: &str, keyword: &str) -> Option<usize> {
    let lower = query.to_ascii_lowercase();
    let kw = keyword.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&kw) {
        let idx = from + pos + kw.len();
        let digits: String = query[idx..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
        from = idx;
    }
    None
}

/// The URI of a `# SIMILAR:<uri>` marker: the first non-whitespace token after
/// `SIMILAR:`. A URI carries no whitespace, so this ignores any trailing
/// template whitespace the marker sits in.
fn extract_similar(query: &str) -> Option<String> {
    const MARKER: &str = "SIMILAR:";
    let pos = query.find(MARKER)?;
    let uri: String = query[pos + MARKER.len()..]
        .trim_start()
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    (!uri.is_empty()).then_some(uri)
}

/// The nucleotide literal of a `sbol2:elements "<seq>"` pattern: the quoted
/// string immediately after `sbol2:elements`, requiring an alphabetic body.
///
/// A variable object (`?seq sbol2:elements ?elements`, the `# TWINS` shape) has
/// no quoted literal between the predicate and any later quote, so it returns
/// `None` and is evaluated over the real triples.
fn extract_sequence(query: &str) -> Option<String> {
    const PRED: &str = "sbol2:elements";
    let after = &query[query.find(PRED)? + PRED.len()..];
    let open = after.find('"')?;
    // Only whitespace may separate the predicate from its literal; anything
    // else means the quote belongs to a later pattern, not this object.
    if !after[..open].trim().is_empty() {
        return None;
    }
    let rest = &after[open + 1..];
    let close = rest.find('"')?;
    let seq = &rest[..close];
    if seq.is_empty() || !seq.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(seq.to_owned())
}

/// The free-text keywords of the ranked search shape: the body of every
/// `CONTAINS(lcase(?displayId), lcase('<term>'))` filter, in order. Mirrors
/// SBOLExplorer's `KEYWORD_PATTERN`, which keys only on the `?displayId`
/// conjunct of each term's disjunction.
fn extract_keywords(query: &str) -> Vec<String> {
    const ANCHOR: &str = "lcase(?displayId), lcase('";
    let mut terms = Vec::new();
    let mut from = 0;
    while let Some(pos) = query[from..].find(ANCHOR) {
        let start = from + pos + ANCHOR.len();
        let Some(end) = query[start..].find('\'') else {
            break;
        };
        let term = &query[start..start + end];
        if !term.is_empty() {
            terms.push(term.to_owned());
        }
        from = start + end + 1;
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "http://synbiohub.org/public";
    const URI: &str = "http://localhost:7777/public/Foo/Foo/1";

    /// The `search.sparql` body classic sends, with `criteria` substituted.
    fn search_query(criteria: &str, limit: &str, offset: &str) -> String {
        format!(
            "PREFIX sbol2: <http://sbols.org/v2#>\n\
             SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role\n\
             FROM <{G}> WHERE {{\n{criteria}\n\
             ?subject a ?type .\n?subject sbh:topLevel ?subject\n\
             OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}\n}}\n{limit}\n{offset}"
        )
    }

    #[test]
    fn recognizes_similar_marker() {
        let q = search_query(&format!("# SIMILAR:{URI}"), "LIMIT 50", "OFFSET 0");
        assert_eq!(
            recognize(&q),
            Some(ExplorerQuery::Similar {
                uri: URI.to_owned(),
                paging: Paging {
                    offset: 0,
                    limit: 50,
                    count: false,
                },
            })
        );
    }

    #[test]
    fn recognizes_global_sequence() {
        let criteria = "?subject sbol2:sequence ?seq .\n    ?seq sbol2:elements \"ACGTACGT\" .";
        let q = search_query(criteria, "LIMIT 25", "OFFSET 10");
        assert_eq!(
            recognize(&q),
            Some(ExplorerQuery::Sequence {
                sequence: "ACGTACGT".to_owned(),
                exact: false,
                paging: Paging {
                    offset: 10,
                    limit: 25,
                    count: false,
                },
            })
        );
    }

    #[test]
    fn recognizes_exact_sequence_flag() {
        let criteria =
            "?subject sbol2:sequence ?seq .\n    ?seq sbol2:elements \"TTGACA\" . # flag_search_exact: True";
        let q = search_query(criteria, "LIMIT 50", "OFFSET 0");
        assert_eq!(
            recognize(&q),
            Some(ExplorerQuery::Sequence {
                sequence: "TTGACA".to_owned(),
                exact: true,
                paging: Paging {
                    offset: 0,
                    limit: 50,
                    count: false,
                },
            })
        );
    }

    #[test]
    fn recognizes_ranked_text() {
        let criteria = "FILTER ((CONTAINS(lcase(?displayId), lcase('promoter'))||CONTAINS(lcase(?name), lcase('promoter'))||CONTAINS(lcase(?description), lcase('promoter')))&&(CONTAINS(lcase(?displayId), lcase('strong'))||CONTAINS(lcase(?name), lcase('strong'))||CONTAINS(lcase(?description), lcase('strong'))))";
        let q = search_query(criteria, "LIMIT 50", "OFFSET 0");
        assert_eq!(
            recognize(&q),
            Some(ExplorerQuery::Text {
                terms: vec!["promoter".to_owned(), "strong".to_owned()],
                paging: Paging {
                    offset: 0,
                    limit: 50,
                    count: false,
                },
            })
        );
    }

    #[test]
    fn count_query_sets_count_flag() {
        let inner = format!(
            "SELECT (count(distinct ?subject) as ?tempcount) WHERE {{\n# SIMILAR:{URI}\n?subject a ?type . }}"
        );
        let q = format!("select (sum(?tempcount) as ?count) FROM <{G}> WHERE {{ {{ {inner} }} }}");
        match recognize(&q) {
            Some(ExplorerQuery::Similar { paging, .. }) => assert!(paging.count),
            other => panic!("expected Similar count, got {other:?}"),
        }
    }

    #[test]
    fn declines_twins_variable_elements() {
        // `# TWINS`: a variable `?elements` object, not a quoted literal.
        let criteria = "?subject sbol2:sequence ?seq .\n    ?seq sbol2:elements ?elements .\n    <http://x/1> sbol2:sequence ?seq2 .\n    ?seq2 sbol2:elements ?elements2 .\n    FILTER(?subject != <http://x/1> && ?elements = ?elements2) # TWINS";
        let q = search_query(criteria, "LIMIT 50", "OFFSET 0");
        assert_eq!(recognize(&q), None);
    }

    #[test]
    fn declines_uses_and_plain() {
        let uses = search_query(
            "{ ?subject ?p <http://x/1> } UNION { ?subject ?p ?use . ?use ?useP <http://x/1> } . # USES",
            "LIMIT 50",
            "OFFSET 0",
        );
        assert_eq!(recognize(&uses), None);
        assert_eq!(recognize("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1"), None);
    }

    #[test]
    fn head_vars_are_exact() {
        assert_eq!(
            SEARCH_VARS,
            [
                "subject",
                "displayId",
                "version",
                "name",
                "description",
                "type",
                "percentMatch",
                "strandAlignment",
                "CIGAR",
            ]
        );
        assert_eq!(COUNT_VARS, ["count"]);
        assert_eq!(search_results(vec![])["head"]["vars"], json!(SEARCH_VARS));
        assert_eq!(count_results(3)["head"]["vars"], json!(COUNT_VARS));
        assert_eq!(
            count_results(3)["results"]["bindings"][0]["count"]["value"],
            json!("3")
        );
    }
}
