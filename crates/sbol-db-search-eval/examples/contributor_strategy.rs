//! A complete contributor loop: implement one public SDK strategy, run it on a
//! versioned suite, and gate it against a baseline.

use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_eval::{
    compare, evaluate_strategy, EvaluationConfig, EvaluationSuite, QualityGate,
};
use sbol_db_search_sdk::{
    DataEgress, DocumentId, ExecutionMetadata, FilterCapability, PaginationCapability, ScoreKind,
    SearchContext, SearchError, SearchHit, SearchInput, SearchInputKind, SearchPage, SearchScope,
    SearchStrategy, StrategyCapabilities, StrategyDescriptor, StrategyRef, StrategyRequirements,
    Total, TotalCapability,
};
use serde::Deserialize;

const CORPUS_JSON: &str = include_str!("../fixtures/contributor-smoke-v1-corpus.json");
const SUITE_JSON: &str = include_str!("../fixtures/contributor-smoke-v1.json");

#[derive(Clone, Deserialize)]
struct Corpus {
    schema_version: u32,
    id: String,
    revision: String,
    license: String,
    documents: Vec<Document>,
}

#[derive(Clone, Deserialize)]
struct Document {
    id: String,
    uri: String,
    name: String,
    description: String,
    object_type: String,
}

/// The experimental behavior is one flag: the baseline matches names, while
/// the candidate also matches descriptions. Replace `score` with your idea.
struct TokenStrategy {
    descriptor: StrategyDescriptor,
    documents: Arc<Vec<Document>>,
    include_description: bool,
}

impl TokenStrategy {
    fn new(id: &str, documents: Arc<Vec<Document>>, include_description: bool) -> Self {
        Self {
            descriptor: StrategyDescriptor {
                id: id.to_owned(),
                version: "1".to_owned(),
                display_name: id.to_owned(),
                description: "Deterministic token-overlap contributor example".to_owned(),
                capabilities: StrategyCapabilities {
                    inputs: vec![SearchInputKind::Text],
                    filters: Vec::new(),
                    filter_execution: FilterCapability::None,
                    pagination: PaginationCapability::FirstPageOnly,
                    totals: TotalCapability::Exact,
                    deterministic: true,
                    explanations: false,
                    data_egress: DataEgress::None,
                },
                requirements: StrategyRequirements::default(),
            },
            documents,
            include_description,
        }
    }

    fn score(&self, query: &HashSet<String>, document: &Document) -> usize {
        let mut searchable = document.name.clone();
        if self.include_description {
            searchable.push(' ');
            searchable.push_str(&document.description);
        }
        let terms = tokens(&searchable);
        query.intersection(&terms).count()
    }
}

#[async_trait]
impl SearchStrategy for TokenStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: SearchContext,
        request: sbol_db_search_sdk::SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        if !matches!(ctx.scope(), SearchScope::Union) {
            return Err(SearchError::Unsupported(
                "the public synthetic fixture supports only union scope".to_owned(),
            ));
        }
        let SearchInput::Text { text } = request.query else {
            return Err(SearchError::Unsupported(
                "this example accepts text queries only".to_owned(),
            ));
        };
        let query = tokens(&text);
        let mut ranked: Vec<_> = self
            .documents
            .iter()
            .map(|document| (self.score(&query, document), document))
            .collect();
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.id.cmp(&right.id))
        });

        let limit = request
            .page
            .limit
            .min(ctx.budget().max_candidates)
            .min(ranked.len());
        let items = ranked
            .into_iter()
            .take(limit)
            .map(|(score, document)| SearchHit {
                document_id: DocumentId(document.id.clone()),
                uri: document.uri.clone(),
                graph: None,
                score: score as f32,
                score_kind: ScoreKind::Custom("token_overlap".to_owned()),
                display_id: None,
                version: None,
                name: Some(document.name.clone()),
                description: Some(document.description.clone()),
                object_types: vec![document.object_type.clone()],
                evidence: Vec::new(),
            })
            .collect();

        Ok(SearchPage {
            strategy: StrategyRef {
                id: self.descriptor.id.clone(),
                version: self.descriptor.version.clone(),
            },
            items,
            total: Total::Exact(self.documents.len()),
            next_cursor: None,
            execution: ExecutionMetadata::default(),
        })
    }
}

fn tokens(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let corpus: Corpus = serde_json::from_str(CORPUS_JSON)?;
    if corpus.schema_version != 1
        || corpus.id != "sbol-db-contributor-smoke"
        || corpus.revision != "1"
        || corpus.license != "CC0-1.0"
    {
        return Err("unexpected contributor corpus identity".into());
    }
    let suite: EvaluationSuite = serde_json::from_str(SUITE_JSON)?;
    let documents = Arc::new(corpus.documents);
    let baseline = TokenStrategy::new("example.name-only.v1", documents.clone(), false);
    let candidate = TokenStrategy::new("example.name-and-description.v1", documents, true);
    let context = SearchContext::new(SearchScope::Union, Default::default());
    let config = EvaluationConfig {
        cutoffs: vec![1, 3],
    };

    let baseline_report = evaluate_strategy(&baseline, context.clone(), &suite, &config).await?;
    let candidate_report = evaluate_strategy(&candidate, context, &suite, &config).await?;
    let comparison = compare(
        &suite,
        &baseline_report,
        &candidate_report,
        &QualityGate {
            cutoff: 3,
            min_cases: 6,
            min_candidate_mean_ndcg: 0.90,
            min_mean_ndcg_delta: 0.05,
            regression_tolerance: 0.001,
            max_regressed_fraction: 0.0,
        },
    )?;

    println!("{}", serde_json::to_string_pretty(&comparison)?);
    if !comparison.accepted {
        return Err("candidate strategy did not pass the quality gate".into());
    }
    Ok(())
}
