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

use tantivy::collector::{Count, TopDocs};
use tantivy::directory::MmapDirectory;
use tantivy::query::{AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TantivyDocument, TextFieldIndexing, TextOptions, Value, FAST,
    STORED, STRING,
};
use tantivy::tokenizer::{LowerCaser, RegexTokenizer, TextAnalyzer};
use tantivy::{DocId, Index, IndexReader, Order, ReloadPolicy, Score, SegmentReader, Term};

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

/// Analyzer matching Elasticsearch's standard analyzer for SBOL identifiers:
/// letters, digits, and connector underscores stay in one lower-cased token.
/// Tantivy's default tokenizer splits on `_`, which turns a precise identifier
/// such as `col_igem_sbol2_151015212923` into several broad OR terms.
const SBOL_EXPLORER_TOKENIZER: &str = "sbol_explorer_standard";
const SBOL_EXPLORER_TOKEN_PATTERN: &str = r"[\p{L}\p{N}_]+";
const PAGERANK_FIELD: &str = "pagerank";

/// A cluster map from a subject to its cluster id. The search combine step
/// remembers which ids have already appeared and applies the divide-by-2
/// duplicate penalty to later members. Keeping the compact assignment instead
/// of expanding every subject to all of its mates preserves the ranking while
/// making a production-scale cold cache linear in the number of sequences.
pub type ClusterMap = HashMap<String, crate::cluster::ClusterId>;

/// Build the [`ClusterMap`] from persisted `(subject, cluster)` assignments,
/// mapping each subject to its cluster id. This is the compact equivalent of
/// SBOLExplorer's `uclust2clusters` transform: the first ranked member of an id
/// remains whole and every later member is a duplicate.
pub fn cluster_map(
    assignments: impl IntoIterator<Item = (String, crate::cluster::ClusterId)>,
) -> ClusterMap {
    assignments.into_iter().collect()
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
    /// Every rdf:type recorded for the object. `type_iri` retains the classic
    /// single-value projection; native discovery uses this complete set so a
    /// secondary type can be filtered without depending on triple order.
    pub type_iris: Vec<String>,
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
    let text = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(SBOL_EXPLORER_TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let text_unstored = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(SBOL_EXPLORER_TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let subject = builder.add_text_field("subject", text.clone());
    // The graph is matched exactly for scope enforcement, so it is untokenized.
    let graph = builder.add_text_field("graph", STRING);
    let display_id = builder.add_text_field("displayId", text.clone());
    let name = builder.add_text_field("name", text.clone());
    let description = builder.add_text_field("description", text.clone());
    let version = builder.add_text_field("version", text.clone());
    let type_field = builder.add_text_field("type", text);
    let keywords = builder.add_text_field("keywords", text_unstored);
    let pagerank = builder.add_f64_field(PAGERANK_FIELD, FAST | STORED);
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
        register_tokenizer(&index)?;
        Self::from_index(index, fields)
    }

    /// An in-RAM index, for tests and ephemeral use.
    pub fn in_ram() -> tantivy::Result<Self> {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        register_tokenizer(&index)?;
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

    /// Number of committed documents currently visible to this reader.
    ///
    /// Servers use this at startup to distinguish an already-populated durable
    /// index from a new/empty directory before scheduling expensive maintenance.
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
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
        let empty_query = self.tokenize(query).is_empty();
        let text_query = self.text_query(query);
        let scoped_query = self.apply_graph_filter(text_query, graphs);

        let desired = offset.saturating_add(limit);
        let fetch = desired.max(FETCH_CAP);
        let hits =
            self.collect_hits(scoped_query.as_ref(), empty_query, fetch, desired, clusters)?;
        Ok(hits.into_iter().skip(offset).take(limit).collect())
    }

    /// Exact number of documents matching `query` inside `graphs`, without
    /// loading stored documents or constructing a top-K scorer.
    pub fn count(&self, query: &str, graphs: &GraphFilter) -> tantivy::Result<usize> {
        let searcher = self.reader.searcher();
        let text_query = self.text_query(query);
        let scoped_query = self.apply_graph_filter(text_query, graphs);
        searcher.search(scoped_query.as_ref(), &Count)
    }

    /// Rank and page one exact rdf:type without first materializing every
    /// document in a million-object graph. The token conjunction is only a
    /// candidate filter; stored type IRIs are checked byte-for-byte before the
    /// exact total and page are returned.
    pub fn search_by_type(
        &self,
        query: &str,
        object_type: &str,
        offset: usize,
        limit: usize,
        graphs: &GraphFilter,
        clusters: &ClusterMap,
    ) -> tantivy::Result<(Vec<Hit>, usize)> {
        let empty_query = self.tokenize(query).is_empty();
        let text_query = self.text_query(query);
        let Some(type_query) = self.type_candidate_query(object_type) else {
            return Ok((Vec::new(), 0));
        };
        let combined: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
            (Occur::Must, text_query),
            (Occur::Must, type_query),
        ]));
        let scoped_query = self.apply_graph_filter(combined, graphs);
        let searcher = self.reader.searcher();
        let candidates = searcher.search(scoped_query.as_ref(), &Count)?;
        if candidates == 0 {
            return Ok((Vec::new(), 0));
        }
        let hits = self.collect_hits(
            scoped_query.as_ref(),
            empty_query,
            candidates,
            candidates,
            clusters,
        )?;
        let filtered: Vec<Hit> = hits
            .into_iter()
            .filter(|hit| hit.type_iris.iter().any(|value| value == object_type))
            .collect();
        let total = filtered.len();
        Ok((
            filtered.into_iter().skip(offset).take(limit).collect(),
            total,
        ))
    }

    fn collect_hits(
        &self,
        scoped_query: &dyn Query,
        empty_query: bool,
        fetch: usize,
        desired: usize,
        clusters: &ClusterMap,
    ) -> tantivy::Result<Vec<Hit>> {
        if fetch == 0 || desired == 0 {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let scored: Vec<(f64, tantivy::DocAddress)> = if empty_query {
            // For a term-less search the BM25 score is the AllQuery constant,
            // so `bm25 * ln(pagerank + 1)` is ordered exactly by PageRank.
            // Let Tantivy's specialized fast-field collector keep the top-K;
            // the generic score tweaker is dramatically slower on million-doc
            // indexes because it constructs a scorer for every match.
            let collector =
                TopDocs::with_limit(fetch).order_by_fast_field::<f64>(PAGERANK_FIELD, Order::Desc);
            searcher
                .search(scoped_query, &collector)?
                .into_iter()
                .map(|(pagerank, address)| ((pagerank + 1.0).ln(), address))
                .collect()
        } else {
            let collector =
                TopDocs::with_limit(fetch).tweak_score(move |segment_reader: &SegmentReader| {
                    let pagerank_reader = segment_reader
                        .fast_fields()
                        .f64(PAGERANK_FIELD)
                        .expect("pagerank fast field present");
                    move |doc: DocId, original_score: Score| {
                        let rank = pagerank_reader.first(doc).unwrap_or(1.0);
                        f64::from(original_score) * (rank + 1.0).ln()
                    }
                });
            searcher.search(scoped_query, &collector)?
        };

        // Penalties are applied in candidate-score order (SBOLExplorer's ES
        // order): a subject whose cluster mate already ranked ahead of it is
        // halved, so a non-centroid cluster member is demoted. An empty cluster
        // map leaves every hit whole.
        let desired = desired.min(scored.len());
        let mut hits = Vec::with_capacity(desired);
        let mut seen_clusters = HashSet::new();
        for (candidate_index, (base_score, address)) in scored.iter().copied().enumerate() {
            let doc: TantivyDocument = searcher.doc(address)?;
            let subject = self
                .stored_string(&doc, self.fields.subject)
                .unwrap_or_default();
            let types: Vec<String> = doc
                .get_all(self.fields.type_field)
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect();

            let mut score = base_score;
            if let Some(cluster_id) = clusters.get(&subject) {
                if !seen_clusters.insert(*cluster_id) {
                    score /= 2.0;
                }
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
                type_iri: types.first().cloned(),
                type_iris: types,
                score,
            });

            // Penalties only lower a candidate's base score. Once the current
            // window's lowest final score is strictly above the next unseen
            // base score, no remaining candidate can enter the requested
            // window. This avoids decoding SBOLExplorer's entire 10k candidate
            // pool for the common first page while preserving the exact order.
            let check_boundary = hits.len() == desired
                || hits.len().saturating_sub(desired) % 64 == 0
                || candidate_index + 1 == scored.len();
            if hits.len() >= desired && check_boundary {
                hits.sort_by(compare_hits);
                let next_base = scored.get(candidate_index + 1).map(|(score, _)| *score);
                if next_base.is_none_or(|next| hits[desired - 1].score > next) {
                    break;
                }
            }
        }

        hits.sort_by(compare_hits);
        hits.truncate(desired);
        Ok(hits)
    }

    /// Return the complete ranked match set under the graph ceiling.
    ///
    /// The compatibility path deliberately keeps its historical 10,000-item
    /// candidate window. Native registry discovery needs exact totals and must
    /// make every matching object reachable by pagination, so it first counts
    /// the scoped Tantivy query and then asks the same scorer for that complete
    /// window. Callers page only after applying their remaining facets.
    pub fn search_all(
        &self,
        query: &str,
        graphs: &GraphFilter,
        clusters: &ClusterMap,
    ) -> tantivy::Result<Vec<Hit>> {
        let searcher = self.reader.searcher();
        let text_query = self.text_query(query);
        let scoped_query = self.apply_graph_filter(text_query, graphs);
        let total = searcher.search(&scoped_query, &Count)?;
        self.search(query, 0, total, graphs, clusters)
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

    /// Narrow a stored rdf:type IRI using every analyzer token. The caller
    /// still checks the stored original string, so token collisions can add
    /// work but can never add a false result.
    fn type_candidate_query(&self, object_type: &str) -> Option<Box<dyn Query>> {
        let clauses: Vec<(Occur, Box<dyn Query>)> = self
            .tokenize(object_type)
            .into_iter()
            .map(|token| {
                let term = Term::from_field_text(self.fields.type_field, &token);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Must, query)
            })
            .collect();
        (!clauses.is_empty()).then(|| Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>)
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
        let Some(mut analyzer) = self.index.tokenizers().get(SBOL_EXPLORER_TOKENIZER) else {
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

fn register_tokenizer(index: &Index) -> tantivy::Result<()> {
    let analyzer = TextAnalyzer::builder(RegexTokenizer::new(SBOL_EXPLORER_TOKEN_PATTERN)?)
        .filter(LowerCaser)
        .build();
    index
        .tokenizers()
        .register(SBOL_EXPLORER_TOKENIZER, analyzer);
    Ok(())
}

fn compare_hits(left: &Hit, right: &Hit) -> std::cmp::Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.subject.cmp(&right.subject))
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
    fn underscore_identifier_remains_one_elasticsearch_compatible_token() {
        let exact = "col_igem_sbol2_151015212923";
        let index = index_with(vec![
            part("http://example.org/exact", exact, "exact collection", 1.0),
            part(
                "http://example.org/common-fragments",
                "different_part",
                "col igem sbol2 151015212923",
                1.0,
            ),
        ]);

        let hits = search(&index, exact);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://example.org/exact");
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
        let clusters = cluster_map(vec![
            (
                "http://example.org/a".to_owned(),
                crate::cluster::ClusterId(0),
            ),
            (
                "http://example.org/b".to_owned(),
                crate::cluster::ClusterId(0),
            ),
        ]);

        let hits = index
            .search("promoter", 0, 100, &GraphFilter::Any, &clusters)
            .expect("search");
        assert_eq!(hits.len(), 2);
        assert!((hits[0].score / hits[1].score - 2.0).abs() < 1e-6);
    }

    #[test]
    fn bounded_window_keeps_candidate_promoted_by_sequence_penalty() {
        let mut high_sequence = part("http://example.org/seq", "part", "same", 99.0);
        high_sequence.type_iris = vec![SEQUENCE_TYPE.to_owned()];
        let non_sequence = part("http://example.org/component", "part", "same", 4.0);
        let tail = part("http://example.org/tail", "part", "same", 1.0);
        let index = index_with(vec![high_sequence, non_sequence, tail]);

        let hits = index
            .search("part", 0, 1, &GraphFilter::Any, &ClusterMap::new())
            .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://example.org/component");
    }

    #[test]
    fn bounded_window_keeps_candidate_promoted_by_cluster_penalty() {
        use crate::cluster::ClusterId;

        let first = part("http://example.org/first", "part", "same", 99.0);
        let duplicate = part("http://example.org/duplicate", "part", "same", 80.0);
        let independent = part("http://example.org/independent", "part", "same", 70.0);
        let index = index_with(vec![first, duplicate, independent]);
        let clusters = cluster_map(vec![
            ("http://example.org/first".to_owned(), ClusterId(0)),
            ("http://example.org/duplicate".to_owned(), ClusterId(0)),
        ]);

        let hits = index
            .search("part", 0, 2, &GraphFilter::Any, &clusters)
            .expect("search");
        let subjects: Vec<&str> = hits.iter().map(|hit| hit.subject.as_str()).collect();

        assert_eq!(
            subjects,
            vec!["http://example.org/first", "http://example.org/independent"]
        );
    }

    #[test]
    fn cluster_map_keeps_compact_assignments() {
        use crate::cluster::ClusterId;
        let map = cluster_map(vec![
            ("a".to_owned(), ClusterId(0)),
            ("b".to_owned(), ClusterId(0)),
            ("c".to_owned(), ClusterId(0)),
            ("d".to_owned(), ClusterId(1)),
        ]);
        assert_eq!(map["a"], ClusterId(0));
        assert_eq!(map["b"], ClusterId(0));
        assert_eq!(map["c"], ClusterId(0));
        assert_eq!(map["d"], ClusterId(1));
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

    #[test]
    fn count_is_exact_and_respects_graph_scope() {
        let mut a = part("http://example.org/a", "promoter", "text", 1.0);
        a.graph = "http://synbiohub.org/graphA".to_owned();
        let mut b = part("http://example.org/b", "promoter", "text", 1.0);
        b.graph = "http://synbiohub.org/graphB".to_owned();
        let index = index_with(vec![a, b]);

        assert_eq!(index.num_docs(), 2);
        assert_eq!(
            index
                .count(
                    "promoter",
                    &GraphFilter::Only(vec!["http://synbiohub.org/graphA".to_owned()]),
                )
                .expect("count"),
            1
        );
        assert_eq!(
            index.count("promoter", &GraphFilter::Any).expect("count"),
            2
        );
    }

    #[test]
    fn type_search_exactly_filters_and_pages() {
        let wanted_type = "http://sbols.org/v2#ComponentDefinition";
        let mut first = part("http://example.org/first", "promoter", "text", 5.0);
        let mut second = part("http://example.org/second", "promoter", "text", 3.0);
        let mut other = part("http://example.org/other", "promoter", "text", 10.0);
        other.type_iris = vec!["http://sbols.org/v2#Component".to_owned()];
        first.type_iris = vec![wanted_type.to_owned()];
        second.type_iris = vec![wanted_type.to_owned()];
        let index = index_with(vec![first, second, other]);

        let (hits, total) = index
            .search_by_type(
                "promoter",
                wanted_type,
                1,
                1,
                &GraphFilter::Any,
                &ClusterMap::new(),
            )
            .expect("typed search");

        assert_eq!(total, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "http://example.org/second");
    }
}
