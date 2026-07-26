//! Reproducible offline evaluation for any sbol-db search strategy.
//!
//! Evaluation suites contain structured SDK requests and graded relevance
//! judgments. Reports can be persisted as JSON by callers and compared with a
//! paired gate before changing a deployment's default strategy.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use sbol_db_search_sdk::{
    DocumentId, SearchContext, SearchError, SearchRequest, SearchStrategy, StrategyRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelevanceJudgment {
    pub document_id: DocumentId,
    /// Zero means explicitly non-relevant; larger values express graded gain.
    pub relevance: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationCase {
    pub id: String,
    pub request: SearchRequest,
    pub judgments: Vec<RelevanceJudgment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationSuite {
    pub id: String,
    pub revision: String,
    pub cases: Vec<EvaluationCase>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationConfig {
    pub cutoffs: Vec<usize>,
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            cutoffs: vec![1, 5, 10, 20],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankingMetrics {
    pub precision: f64,
    pub recall: f64,
    pub reciprocal_rank: f64,
    pub ndcg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaseReport {
    pub case_id: String,
    pub returned: Vec<DocumentId>,
    pub metrics: BTreeMap<usize, RankingMetrics>,
    pub elapsed_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub mean_precision: f64,
    pub mean_recall: f64,
    pub mean_reciprocal_rank: f64,
    pub mean_ndcg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub suite_id: String,
    pub suite_revision: String,
    pub strategy: StrategyRef,
    pub cases: Vec<CaseReport>,
    pub aggregate: BTreeMap<usize, AggregateMetrics>,
    pub mean_elapsed_ms: f64,
}

/// Execute one strategy against a suite. Every case is requested at the
/// largest configured cutoff, ensuring metrics compare the same prefix even if
/// the case fixture used a smaller interactive page size.
pub async fn evaluate_strategy(
    strategy: &dyn SearchStrategy,
    context: SearchContext,
    suite: &EvaluationSuite,
    config: &EvaluationConfig,
) -> Result<EvaluationReport, SearchError> {
    validate_suite(suite, config)?;
    let max_cutoff = *config
        .cutoffs
        .iter()
        .max()
        .expect("validation rejects empty cutoffs");
    let mut cases = Vec::with_capacity(suite.cases.len());
    let mut observed_strategy = None;

    for case in &suite.cases {
        let mut request = case.request.clone();
        request.strategy = Some(strategy.descriptor().id.clone());
        request.page.limit = max_cutoff;
        request.page.cursor = None;
        let started = Instant::now();
        let page = strategy.search(context.clone(), request).await?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        if let Some(observed) = &observed_strategy {
            if observed != &page.strategy {
                return Err(SearchError::Backend(
                    "strategy identity changed during evaluation".to_owned(),
                ));
            }
        } else {
            observed_strategy = Some(page.strategy.clone());
        }

        let returned: Vec<_> = page.items.into_iter().map(|hit| hit.document_id).collect();
        reject_duplicate_results(&case.id, &returned)?;
        cases.push(CaseReport {
            case_id: case.id.clone(),
            metrics: metrics_at_cutoffs(&returned, &case.judgments, &config.cutoffs),
            returned,
            elapsed_ms,
        });
    }

    let strategy = observed_strategy.unwrap_or_else(|| StrategyRef {
        id: strategy.descriptor().id.clone(),
        version: strategy.descriptor().version.clone(),
    });
    Ok(EvaluationReport {
        suite_id: suite.id.clone(),
        suite_revision: suite.revision.clone(),
        strategy,
        aggregate: aggregate(&cases, &config.cutoffs),
        mean_elapsed_ms: mean(cases.iter().map(|case| case.elapsed_ms)),
        cases,
    })
}

pub fn metrics_at_cutoffs(
    returned: &[DocumentId],
    judgments: &[RelevanceJudgment],
    cutoffs: &[usize],
) -> BTreeMap<usize, RankingMetrics> {
    let judged: HashMap<_, _> = judgments
        .iter()
        .map(|judgment| (&judgment.document_id, judgment.relevance))
        .collect();
    let relevant = judgments
        .iter()
        .filter(|judgment| judgment.relevance > 0)
        .count();
    let mut ideal_relevance: Vec<_> = judgments
        .iter()
        .map(|judgment| judgment.relevance)
        .filter(|relevance| *relevance > 0)
        .collect();
    ideal_relevance.sort_unstable_by(|left, right| right.cmp(left));

    cutoffs
        .iter()
        .copied()
        .map(|cutoff| {
            let prefix = returned.iter().take(cutoff);
            let gains: Vec<_> = prefix
                .map(|document_id| judged.get(document_id).copied().unwrap_or(0))
                .collect();
            let relevant_retrieved = gains.iter().filter(|gain| **gain > 0).count();
            let precision = relevant_retrieved as f64 / cutoff as f64;
            let recall = if relevant == 0 {
                0.0
            } else {
                relevant_retrieved as f64 / relevant as f64
            };
            let reciprocal_rank = gains
                .iter()
                .position(|gain| *gain > 0)
                .map(|rank| 1.0 / (rank + 1) as f64)
                .unwrap_or(0.0);
            let dcg = discounted_gain(gains.iter().copied());
            let ideal = discounted_gain(ideal_relevance.iter().copied().take(cutoff));
            let ndcg = if ideal == 0.0 { 0.0 } else { dcg / ideal };
            (
                cutoff,
                RankingMetrics {
                    precision,
                    recall,
                    reciprocal_rank,
                    ndcg,
                },
            )
        })
        .collect()
}

fn discounted_gain(relevances: impl Iterator<Item = u8>) -> f64 {
    relevances
        .enumerate()
        .map(|(rank, relevance)| {
            let gain = 2_f64.powi(i32::from(relevance)) - 1.0;
            gain / ((rank + 2) as f64).log2()
        })
        .sum()
}

fn aggregate(cases: &[CaseReport], cutoffs: &[usize]) -> BTreeMap<usize, AggregateMetrics> {
    cutoffs
        .iter()
        .copied()
        .map(|cutoff| {
            let metrics: Vec<_> = cases
                .iter()
                .filter_map(|case| case.metrics.get(&cutoff))
                .collect();
            (
                cutoff,
                AggregateMetrics {
                    mean_precision: mean(metrics.iter().map(|metric| metric.precision)),
                    mean_recall: mean(metrics.iter().map(|metric| metric.recall)),
                    mean_reciprocal_rank: mean(metrics.iter().map(|metric| metric.reciprocal_rank)),
                    mean_ndcg: mean(metrics.iter().map(|metric| metric.ndcg)),
                },
            )
        })
        .collect()
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<_> = values.collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityGate {
    pub cutoff: usize,
    /// Minimum required candidate minus baseline mean nDCG.
    pub min_mean_ndcg_delta: f64,
    /// Per-query nDCG drops at or below this tolerance count as ties.
    pub regression_tolerance: f64,
    /// Maximum fraction of cases allowed to regress beyond the tolerance.
    pub max_regressed_fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub cutoff: usize,
    pub baseline_strategy: StrategyRef,
    pub candidate_strategy: StrategyRef,
    pub mean_precision_delta: f64,
    pub mean_recall_delta: f64,
    pub mean_reciprocal_rank_delta: f64,
    pub mean_ndcg_delta: f64,
    pub wins: usize,
    pub ties: usize,
    pub regressions: usize,
    pub regressed_fraction: f64,
    pub accepted: bool,
}

/// Compare the same cases pairwise. Reports from different fixture revisions
/// are rejected so a changed benchmark cannot masquerade as a strategy gain.
pub fn compare(
    baseline: &EvaluationReport,
    candidate: &EvaluationReport,
    gate: &QualityGate,
) -> Result<ComparisonReport, SearchError> {
    if baseline.suite_id != candidate.suite_id
        || baseline.suite_revision != candidate.suite_revision
    {
        return Err(SearchError::InvalidRequest(
            "evaluation reports use different suite identities or revisions".to_owned(),
        ));
    }
    if !(0.0..=1.0).contains(&gate.max_regressed_fraction) || gate.regression_tolerance < 0.0 {
        return Err(SearchError::InvalidRequest(
            "quality gate regression bounds are invalid".to_owned(),
        ));
    }
    let baseline_aggregate = baseline.aggregate.get(&gate.cutoff).ok_or_else(|| {
        SearchError::InvalidRequest(format!("baseline has no cutoff {}", gate.cutoff))
    })?;
    let candidate_aggregate = candidate.aggregate.get(&gate.cutoff).ok_or_else(|| {
        SearchError::InvalidRequest(format!("candidate has no cutoff {}", gate.cutoff))
    })?;
    let candidate_cases: HashMap<_, _> = candidate
        .cases
        .iter()
        .map(|case| (case.case_id.as_str(), case))
        .collect();
    if baseline.cases.len() != candidate_cases.len() {
        return Err(SearchError::InvalidRequest(
            "evaluation reports contain different case sets".to_owned(),
        ));
    }

    let mut wins = 0;
    let mut ties = 0;
    let mut regressions = 0;
    for baseline_case in &baseline.cases {
        let candidate_case = candidate_cases
            .get(baseline_case.case_id.as_str())
            .ok_or_else(|| {
                SearchError::InvalidRequest(format!(
                    "candidate report is missing case {:?}",
                    baseline_case.case_id
                ))
            })?;
        let baseline_ndcg = baseline_case
            .metrics
            .get(&gate.cutoff)
            .ok_or_else(|| missing_case_cutoff(&baseline_case.case_id, gate.cutoff))?
            .ndcg;
        let candidate_ndcg = candidate_case
            .metrics
            .get(&gate.cutoff)
            .ok_or_else(|| missing_case_cutoff(&candidate_case.case_id, gate.cutoff))?
            .ndcg;
        let delta = candidate_ndcg - baseline_ndcg;
        if delta > gate.regression_tolerance {
            wins += 1;
        } else if delta < -gate.regression_tolerance {
            regressions += 1;
        } else {
            ties += 1;
        }
    }
    let regressed_fraction = if baseline.cases.is_empty() {
        0.0
    } else {
        regressions as f64 / baseline.cases.len() as f64
    };
    let mean_ndcg_delta = candidate_aggregate.mean_ndcg - baseline_aggregate.mean_ndcg;

    Ok(ComparisonReport {
        cutoff: gate.cutoff,
        baseline_strategy: baseline.strategy.clone(),
        candidate_strategy: candidate.strategy.clone(),
        mean_precision_delta: candidate_aggregate.mean_precision
            - baseline_aggregate.mean_precision,
        mean_recall_delta: candidate_aggregate.mean_recall - baseline_aggregate.mean_recall,
        mean_reciprocal_rank_delta: candidate_aggregate.mean_reciprocal_rank
            - baseline_aggregate.mean_reciprocal_rank,
        mean_ndcg_delta,
        wins,
        ties,
        regressions,
        regressed_fraction,
        accepted: mean_ndcg_delta >= gate.min_mean_ndcg_delta
            && regressed_fraction <= gate.max_regressed_fraction,
    })
}

fn validate_suite(suite: &EvaluationSuite, config: &EvaluationConfig) -> Result<(), SearchError> {
    if suite.id.trim().is_empty() || suite.revision.trim().is_empty() {
        return Err(SearchError::InvalidRequest(
            "evaluation suite id and revision cannot be empty".to_owned(),
        ));
    }
    if config.cutoffs.is_empty() || config.cutoffs.contains(&0) {
        return Err(SearchError::InvalidRequest(
            "evaluation cutoffs must be non-empty and greater than zero".to_owned(),
        ));
    }
    let mut case_ids = HashSet::new();
    for case in &suite.cases {
        if case.id.trim().is_empty() || !case_ids.insert(&case.id) {
            return Err(SearchError::InvalidRequest(format!(
                "evaluation case id {:?} is empty or duplicated",
                case.id
            )));
        }
        let mut document_ids = HashSet::new();
        if case
            .judgments
            .iter()
            .any(|judgment| !document_ids.insert(&judgment.document_id))
        {
            return Err(SearchError::InvalidRequest(format!(
                "evaluation case {:?} has duplicate judgments",
                case.id
            )));
        }
    }
    Ok(())
}

fn reject_duplicate_results(case_id: &str, returned: &[DocumentId]) -> Result<(), SearchError> {
    let mut seen = HashSet::new();
    if returned.iter().any(|document_id| !seen.insert(document_id)) {
        return Err(SearchError::Backend(format!(
            "strategy returned duplicate document ids for case {case_id:?}"
        )));
    }
    Ok(())
}

fn missing_case_cutoff(case_id: &str, cutoff: usize) -> SearchError {
    SearchError::InvalidRequest(format!(
        "evaluation case {case_id:?} has no cutoff {cutoff}"
    ))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sbol_db_search_sdk::{
        DataEgress, ExecutionMetadata, FilterCapability, PaginationCapability, ScoreKind,
        SearchHit, SearchInput, SearchInputKind, SearchPage, StrategyCapabilities,
        StrategyDescriptor, StrategyRequirements, Total, TotalCapability,
    };

    use super::*;

    fn id(value: &str) -> DocumentId {
        DocumentId(value.to_owned())
    }

    fn metrics_for(order: &[&str]) -> BTreeMap<usize, RankingMetrics> {
        metrics_at_cutoffs(
            &order.iter().map(|value| id(value)).collect::<Vec<_>>(),
            &[
                RelevanceJudgment {
                    document_id: id("best"),
                    relevance: 3,
                },
                RelevanceJudgment {
                    document_id: id("good"),
                    relevance: 1,
                },
            ],
            &[1, 3],
        )
    }

    #[test]
    fn metrics_reward_graded_relevance_and_early_ranks() {
        let ideal = metrics_for(&["best", "noise", "good"]);
        let reversed = metrics_for(&["good", "noise", "best"]);
        assert_eq!(ideal[&1].precision, 1.0);
        assert_eq!(ideal[&3].recall, 1.0);
        assert_eq!(ideal[&1].ndcg, 1.0);
        assert!(ideal[&3].ndcg > reversed[&3].ndcg);
    }

    fn report(strategy: &str, metrics: BTreeMap<usize, RankingMetrics>) -> EvaluationReport {
        let case = CaseReport {
            case_id: "q1".to_owned(),
            returned: Vec::new(),
            metrics,
            elapsed_ms: 1.0,
        };
        let aggregate = aggregate(std::slice::from_ref(&case), &[3]);
        EvaluationReport {
            suite_id: "suite".to_owned(),
            suite_revision: "r1".to_owned(),
            strategy: StrategyRef {
                id: strategy.to_owned(),
                version: "1".to_owned(),
            },
            cases: vec![case],
            aggregate,
            mean_elapsed_ms: 1.0,
        }
    }

    #[test]
    fn paired_gate_accepts_a_material_improvement() {
        let baseline = report("baseline", metrics_for(&["noise", "good", "best"]));
        let candidate = report("candidate", metrics_for(&["best", "good", "noise"]));
        let comparison = compare(
            &baseline,
            &candidate,
            &QualityGate {
                cutoff: 3,
                min_mean_ndcg_delta: 0.05,
                regression_tolerance: 0.001,
                max_regressed_fraction: 0.0,
            },
        )
        .unwrap();
        assert!(comparison.accepted);
        assert_eq!(comparison.wins, 1);
        assert!(comparison.mean_ndcg_delta > 0.05);
    }

    struct FixedStrategy {
        descriptor: StrategyDescriptor,
    }

    #[async_trait]
    impl SearchStrategy for FixedStrategy {
        fn descriptor(&self) -> &StrategyDescriptor {
            &self.descriptor
        }

        async fn search(
            &self,
            _ctx: SearchContext,
            _request: SearchRequest,
        ) -> Result<SearchPage, SearchError> {
            Ok(SearchPage {
                strategy: StrategyRef {
                    id: self.descriptor.id.clone(),
                    version: self.descriptor.version.clone(),
                },
                items: ["best", "noise", "good"]
                    .into_iter()
                    .map(|value| SearchHit {
                        document_id: id(value),
                        uri: value.to_owned(),
                        graph: None,
                        score: 1.0,
                        score_kind: ScoreKind::Custom("fixed".to_owned()),
                        display_id: None,
                        version: None,
                        name: None,
                        description: None,
                        object_types: Vec::new(),
                        evidence: Vec::new(),
                    })
                    .collect(),
                total: Total::Exact(3),
                next_cursor: None,
                execution: ExecutionMetadata::default(),
            })
        }
    }

    #[tokio::test]
    async fn executor_runs_an_sdk_strategy_against_versioned_cases() {
        let strategy = FixedStrategy {
            descriptor: StrategyDescriptor {
                id: "fixed.v1".to_owned(),
                version: "1".to_owned(),
                display_name: "Fixed".to_owned(),
                description: String::new(),
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
        };
        let suite = EvaluationSuite {
            id: "parts".to_owned(),
            revision: "fixture-sha".to_owned(),
            cases: vec![EvaluationCase {
                id: "q1".to_owned(),
                request: SearchRequest {
                    strategy: None,
                    query: SearchInput::Text {
                        text: "promoter".to_owned(),
                    },
                    filters: Default::default(),
                    page: Default::default(),
                    options: Default::default(),
                },
                judgments: vec![RelevanceJudgment {
                    document_id: id("best"),
                    relevance: 3,
                }],
            }],
        };
        let report = evaluate_strategy(
            &strategy,
            SearchContext::new(sbol_db_search_sdk::SearchScope::Union, Default::default()),
            &suite,
            &EvaluationConfig { cutoffs: vec![3] },
        )
        .await
        .unwrap();
        assert_eq!(report.strategy.id, "fixed.v1");
        assert_eq!(report.aggregate[&3].mean_ndcg, 1.0);
    }
}
