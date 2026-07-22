//! The tantivy ranked text index.
//!
//! [`RankedTextIndex`] is the native, Elasticsearch-free replacement for
//! SBOLExplorer's search. It indexes each top-level object's metadata plus a
//! synthetic [`keywords`](crate::keywords) field, and answers a free-text query
//! with SBOLExplorer's exact ranking: a fuzzy multi-field text match whose score
//! is combined with PageRank as `bm25 * ln(pagerank + 1)`, then penalized so a
//! Sequence-typed hit is divided by 10 and a cluster-duplicate is divided by 2.
//!
//! The combine and penalties reproduce SBOLExplorer's `search_es` script score
//! and `create_bindings`. The cluster-duplicate step consumes a [`ClusterMap`]
//! built from the persisted cluster assignments ([`cluster_map`]): a hit whose
//! cluster mate already ranked ahead of it is divided by 2, SBOLExplorer's
//! greedy dedup.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TantivyDocument, Value, FAST, STORED, STRING, TEXT,
};
use tantivy::{DocId, Index, IndexReader, ReloadPolicy, Score, SegmentReader, Term};

/// The rdf:type IRI whose presence divides a hit's score by 10.
const SEQUENCE_TYPE: &str = "http://sbols.org/v2#Sequence";

/// The query-time boost applied to display-id matches, SBOLExplorer's
/// `displayId^3`.
const DISPLAY_ID_BOOST: Score = 3.0;

/// The candidate pool pulled from tantivy before penalties and paging, matching
/// SBOLExplorer's Elasticsearch `size`. Penalties can reorder hits, so the pool
/// is scored and collected before the offset/limit window is taken.
const FETCH_CAP: usize = 10_000;

/// The heap budget for the single index writer, in bytes.
const WRITER_HEAP_BYTES: usize = 50_000_000;

/// A cluster map from a subject to its cluster mates (the other members of its
/// cluster). The search combine step reads it to apply the divide-by-2
/// duplicate penalty; [`cluster_map`] builds it from persisted assignments.
pub type ClusterMap = HashMap<String, Vec<String>>;

/// Build the [`ClusterMap`] from persisted `(subject, cluster)` assignments,
/// mapping each subject to the other members of its cluster.
///
/// This is SBOLExplorer's `uclust2clusters` transform: it groups assignments by
/// cluster id, then maps every member to the set of its cluster mates (itself
/// excluded). A singleton cluster maps its sole member to an empty list, which
/// the penalty treats as no duplicates.
pub fn cluster_map(
    assignments: impl IntoIterator<Item = (String, crate::cluster::ClusterId)>,
) -> ClusterMap {
    let mut by_cluster: HashMap<crate::cluster::ClusterId, Vec<String>> = HashMap::new();
    for (subject, cluster) in assignments {
        by_cluster.entry(cluster).or_default().push(subject);
    }
    let mut map = ClusterMap::new();
    for members in by_cluster.values() {
        for member in members {
            let mates: Vec<String> = members.iter().filter(|m| *m != member).cloned().collect();
            map.insert(member.clone(), mates);
        }
    }
    map
}

/// The graphs a search is authorized to read. The facade maps its
/// `GraphScope` onto this so the index enforces the scope in the query rather
/// than the caller filtering results.
#[derive(Clone, Debug)]
pub enum GraphFilter {
    /// No restriction: every indexed graph is in scope.
    Any,
    /// Only the listed graphs are in scope. An empty list matches nothing.
    Only(Vec<String>),
}

/// One object to index: its identity, the graph holding it, its searchable
/// metadata, the synthetic keyword string, and its PageRank score.
#[derive(Clone, Debug)]
pub struct IndexedPart {
    pub subject: String,
    pub graph: String,
    pub display_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub type_iris: Vec<String>,
    pub keywords: String,
    pub pagerank: f64,
}

/// One ranked search result, projecting the columns the SynBioHub v1 `/search`
/// response emits plus the final combined score.
#[derive(Clone, Debug)]
pub struct Hit {
    pub subject: String,
    pub display_id: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub type_iri: Option<String>,
    pub score: f64,
}

/// The tantivy fields, resolved once from the schema.
struct Fields {
    subject: Field,
    graph: Field,
    display_id: Field,
    name: Field,
    description: Field,
    version: Field,
    type_field: Field,
    keywords: Field,
    pagerank: Field,
}

/// The ranked text index over a single tantivy index, either on a filesystem
/// directory (the shared production sidecar) or in RAM (tests).
pub struct RankedTextIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

/// The searchable text fields, paired with each field's query-time boost. These
/// are SBOLExplorer's `multi_match` fields, with `displayId` boosted 3x.
fn searchable_fields(fields: &Fields) -> [(Field, Score); 7] {
    [
        (fields.subject, 1.0),
        (fields.display_id, DISPLAY_ID_BOOST),
        (fields.version, 1.0),
        (fields.name, 1.0),
        (fields.description, 1.0),
        (fields.type_field, 1.0),
        (fields.keywords, 1.0),
    ]
}

/// SBOLExplorer's Elasticsearch `fuzziness: AUTO`: an exact match for very short
/// terms, one edit for short terms, two otherwise.
fn auto_distance(term_len: usize) -> u8 {
    match term_len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let subject = builder.add_text_field("subject", TEXT | STORED);
    // The graph is matched exactly for scope enforcement, so it is untokenized.
    let graph = builder.add_text_field("graph", STRING);
    let display_id = builder.add_text_field("displayId", TEXT | STORED);
    let name = builder.add_text_field("name", TEXT | STORED);
    let description = builder.add_text_field("description", TEXT | STORED);
    let version = builder.add_text_field("version", TEXT | STORED);
    let type_field = builder.add_text_field("type", TEXT | STORED);
    let keywords = builder.add_text_field("keywords", TEXT);
    let pagerank = builder.add_f64_field("pagerank", FAST | STORED);
    let schema = builder.build();
    let fields = Fields {
        subject,
        graph,
        display_id,
        name,
        description,
        version,
        type_field,
        keywords,
        pagerank,
    };
    (schema, fields)
}

impl RankedTextIndex {
    /// Open the shared index at `path`, creating it if absent.
    pub fn open_or_create(path: &Path) -> tantivy::Result<Self> {
        let (schema, fields) = build_schema();
        let directory = MmapDirectory::open(path)?;
        let index = Index::open_or_create(directory, schema)?;
        Self::from_index(index, fields)
    }

    /// An in-RAM index, for tests and ephemeral use.
    pub fn in_ram() -> tantivy::Result<Self> {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        Self::from_index(index, fields)
    }

    fn from_index(index: Index, fields: Fields) -> tantivy::Result<Self> {
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    /// Replace every document in the index with `parts` in a single writer
    /// commit, then reload the reader so the new corpus is visible.
    pub fn rebuild(&self, parts: impl IntoIterator<Item = IndexedPart>) -> tantivy::Result<()> {
        let mut writer = self.index.writer(WRITER_HEAP_BYTES)?;
        writer.delete_all_documents()?;
        for part in parts {
            let mut doc = TantivyDocument::default();
            doc.add_text(self.fields.subject, &part.subject);
            doc.add_text(self.fields.graph, &part.graph);
            if let Some(v) = &part.display_id {
                doc.add_text(self.fields.display_id, v);
            }
            if let Some(v) = &part.name {
                doc.add_text(self.fields.name, v);
            }
            if let Some(v) = &part.description {
                doc.add_text(self.fields.description, v);
            }
            if let Some(v) = &part.version {
                doc.add_text(self.fields.version, v);
            }
            for type_iri in &part.type_iris {
                doc.add_text(self.fields.type_field, type_iri);
            }
            doc.add_text(self.fields.keywords, &part.keywords);
            doc.add_f64(self.fields.pagerank, part.pagerank);
            writer.add_document(doc)?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Rank the documents matching `query` within the authorized graphs.
    ///
    /// The base score is `bm25 * ln(pagerank + 1)` (an empty query ranks purely
    /// by `ln(pagerank + 1)`). A cluster-duplicate hit is then divided by 2 and a
    /// Sequence-typed hit by 10, matching SBOLExplorer's penalty order, before
    /// the results are ordered by final score and the `offset`/`limit` window is
    /// taken.
    pub fn search(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        graphs: &GraphFilter,
        clusters: &ClusterMap,
    ) -> tantivy::Result<Vec<Hit>> {
        let searcher = self.reader.searcher();
        let text_query = self.text_query(query);
        let scoped_query = self.apply_graph_filter(text_query, graphs);

        let fetch = (offset + limit).max(FETCH_CAP);
        let collector =
            TopDocs::with_limit(fetch).tweak_score(move |segment_reader: &SegmentReader| {
                let pagerank_reader = segment_reader
                    .fast_fields()
                    .f64("pagerank")
                    .expect("pagerank fast field present");
                move |doc: DocId, original_score: Score| {
                    let rank = pagerank_reader.first(doc).unwrap_or(1.0);
                    f64::from(original_score) * (rank + 1.0).ln()
                }
            });
        let scored: Vec<(f64, tantivy::DocAddress)> = searcher.search(&scoped_query, &collector)?;

        // Penalties are applied in candidate-score order (SBOLExplorer's ES
        // order): a subject whose cluster mate already ranked ahead of it is
        // halved, so a non-centroid cluster member is demoted. An empty cluster
        // map leaves every hit whole.
        let mut hits = Vec::with_capacity(scored.len());
        let mut expanded: HashSet<String> = HashSet::new();
        for (base_score, address) in scored {
            let doc: TantivyDocument = searcher.doc(address)?;
            let subject = self
                .stored_string(&doc, self.fields.subject)
                .unwrap_or_default();
            let types: Vec<String> = doc
                .get_all(self.fields.type_field)
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect();

            let mut score = base_score;
            if expanded.contains(&subject) {
                score /= 2.0;
            } else if let Some(duplicates) = clusters.get(&subject) {
                expanded.extend(duplicates.iter().cloned());
            }
            if types.iter().any(|t| t == SEQUENCE_TYPE) {
                score /= 10.0;
            }

            hits.push(Hit {
                subject,
                display_id: self.stored_string(&doc, self.fields.display_id),
                version: self.stored_string(&doc, self.fields.version),
                name: self.stored_string(&doc, self.fields.name),
                description: self.stored_string(&doc, self.fields.description),
                type_iri: types.into_iter().next(),
                score,
            });
        }

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(hits.into_iter().skip(offset).take(limit).collect())
    }

    /// The boolean OR of per-field fuzzy term queries. An empty or whitespace
    /// query matches every document so the base score is `ln(pagerank + 1)`
    /// alone.
    fn text_query(&self, query: &str) -> Box<dyn Query> {
        let tokens = self.tokenize(query);
        if tokens.is_empty() {
            return Box::new(AllQuery);
        }
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for (field, boost) in searchable_fields(&self.fields) {
            for token in &tokens {
                let distance = auto_distance(token.chars().count());
                let term = Term::from_field_text(field, token);
                let fuzzy = FuzzyTermQuery::new(term, distance, true);
                let query: Box<dyn Query> = if boost != 1.0 {
                    Box::new(BoostQuery::new(Box::new(fuzzy), boost))
                } else {
                    Box::new(fuzzy)
                };
                clauses.push((Occur::Should, query));
            }
        }
        Box::new(BooleanQuery::new(clauses))
    }

    /// Constrain a query to the authorized graphs. `Any` passes through;
    /// `Only` requires the graph field to equal one of the listed IRIs (an
    /// empty list matches nothing).
    fn apply_graph_filter(&self, query: Box<dyn Query>, graphs: &GraphFilter) -> Box<dyn Query> {
        let allowed = match graphs {
            GraphFilter::Any => return query,
            GraphFilter::Only(allowed) => allowed,
        };
        let graph_clauses: Vec<(Occur, Box<dyn Query>)> = allowed
            .iter()
            .map(|graph| {
                let term = Term::from_field_text(self.fields.graph, graph);
                let term_query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Should, term_query)
            })
            .collect();
        let graph_filter = BooleanQuery::new(graph_clauses);
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, query),
            (Occur::Must, Box::new(graph_filter)),
        ]))
    }

    fn tokenize(&self, query: &str) -> Vec<String> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let Some(mut analyzer) = self.index.tokenizers().get("default") else {
            return Vec::new();
        };
        let mut tokens = Vec::new();
        let mut stream = analyzer.token_stream(query);
        while stream.advance() {
            tokens.push(stream.token().text.clone());
        }
        tokens
    }

    fn stored_string(&self, doc: &TantivyDocument, field: Field) -> Option<String> {
        doc.get_first(field)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(subject: &str, display_id: &str, description: &str, pagerank: f64) -> IndexedPart {
        IndexedPart {
            subject: subject.to_owned(),
            graph: "http://synbiohub.org/public".to_owned(),
            display_id: Some(display_id.to_owned()),
            name: None,
            description: Some(description.to_owned()),
            version: Some("1".to_owned()),
            type_iris: vec!["http://sbols.org/v2#ComponentDefinition".to_owned()],
            keywords: display_id.split('_').collect::<Vec<_>>().join(" "),
            pagerank,
        }
    }

    fn index_with(parts: Vec<IndexedPart>) -> RankedTextIndex {
        let index = RankedTextIndex::in_ram().expect("in-ram index");
        index.rebuild(parts).expect("rebuild");
        index
    }

    fn search(index: &RankedTextIndex, query: &str) -> Vec<Hit> {
        index
            .search(query, 0, 100, &GraphFilter::Any, &ClusterMap::new())
            .expect("search")
    }

    #[test]
    fn display_id_exact_outranks_description_only() {
        let index = index_with(vec![
            part("http://example.org/promoter", "promoter", "a widget", 1.0),
            part(
                "http://example.org/widget",
                "widget",
                "a strong promoter element",
                1.0,
            ),
        ]);
        let hits = search(&index, "promoter");
        assert_eq!(hits[0].subject, "http://example.org/promoter");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn higher_pagerank_wins_a_text_tie() {
        // Both objects match the query identically; the higher PageRank wins.
        let index = index_with(vec![
            part("http://example.org/low", "promoter", "same text", 1.0),
            part("http://example.org/high", "promoter", "same text", 9.0),
        ]);
        let hits = search(&index, "promoter");
        assert_eq!(hits[0].subject, "http://example.org/high");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn sequence_hit_never_outranks_equal_non_sequence() {
        let mut sequence = part("http://example.org/seq", "promoter", "same text", 1.0);
        sequence.type_iris = vec![SEQUENCE_TYPE.to_owned()];
        let component = part("http://example.org/cd", "promoter", "same text", 1.0);
        let index = index_with(vec![sequence, component]);

        let hits = search(&index, "promoter");
        assert_eq!(
            hits[0].subject, "http://example.org/cd",
            "the non-Sequence hit ranks first"
        );
        // The Sequence hit is divided by 10, so it trails despite an equal text
        // score.
        assert!((hits[0].score / hits[1].score - 10.0).abs() < 1e-6);
    }

    #[test]
    fn cluster_duplicate_is_halved() {
        let index = index_with(vec![
            part("http://example.org/a", "promoter", "same text", 1.0),
            part("http://example.org/b", "promoter", "same text", 1.0),
        ]);
        let mut clusters = ClusterMap::new();
        // The first-ranked subject declares the second its duplicate, halving it.
        let unpenalized = search(&index, "promoter");
        let leader = unpenalized[0].subject.clone();
        let follower = unpenalized[1].subject.clone();
        clusters.insert(leader, vec![follower.clone()]);

        let hits = index
            .search("promoter", 0, 100, &GraphFilter::Any, &clusters)
            .expect("search");
        let follower_hit = hits.iter().find(|h| h.subject == follower).unwrap();
        let follower_base = unpenalized
            .iter()
            .find(|h| h.subject == follower)
            .unwrap()
            .score;
        assert!((follower_base / follower_hit.score - 2.0).abs() < 1e-6);
    }

    #[test]
    fn cluster_map_groups_members_into_mates() {
        use crate::cluster::ClusterId;
        let map = cluster_map(vec![
            ("a".to_owned(), ClusterId(0)),
            ("b".to_owned(), ClusterId(0)),
            ("c".to_owned(), ClusterId(0)),
            ("d".to_owned(), ClusterId(1)),
        ]);
        let mut a_mates = map["a"].clone();
        a_mates.sort();
        assert_eq!(a_mates, vec!["b".to_owned(), "c".to_owned()]);
        assert!(map["d"].is_empty(), "a singleton has no mates");
    }

    #[test]
    fn empty_query_ranks_by_pagerank() {
        let index = index_with(vec![
            part("http://example.org/low", "alpha", "one", 1.0),
            part("http://example.org/high", "beta", "two", 5.0),
            part("http://example.org/mid", "gamma", "three", 3.0),
        ]);
        let hits = search(&index, "   ");
        let order: Vec<&str> = hits.iter().map(|h| h.subject.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "http://example.org/high",
                "http://example.org/mid",
                "http://example.org/low",
            ]
        );
    }

    #[test]
    fn graph_filter_hides_out_of_scope_objects() {
        let mut a = part("http://example.org/a", "promoter", "text", 1.0);
        a.graph = "http://synbiohub.org/graphA".to_owned();
        let mut b = part("http://example.org/b", "promoter", "text", 1.0);
        b.graph = "http://synbiohub.org/graphB".to_owned();
        let index = index_with(vec![a, b]);

        let hits = index
            .search(
                "promoter",
                0,
                100,
                &GraphFilter::Only(vec!["http://synbiohub.org/graphA".to_owned()]),
                &ClusterMap::new(),
            )
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://example.org/a");
    }
}
