//! Reproducible offline evaluation for any sbol-db search strategy.
//!
//! Evaluation suites contain structured SDK requests and graded relevance
//! judgments. Reports can be persisted as JSON by callers and compared with a
//! paired gate before changing a deployment's default strategy.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::time::Instant;

use sbol_db_search_sdk::{
    DocumentId, SearchContext, SearchError, SearchRequest, SearchStrategy, StrategyRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EVALUATION_SUITE_SCHEMA_VERSION: u32 = 1;

/// Human-auditable sources for the corpus and relevance labels used by a
/// suite. Revisions and content hashes make fixture changes explicit in code
/// review; the suite fingerprint below binds this provenance to every report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationProvenance {
    pub corpus_source: String,
    pub corpus_revision: String,
    pub corpus_sha256: String,
    pub judgments_source: String,
    pub judgments_method: String,
}

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
    pub schema_version: u32,
    pub id: String,
    pub revision: String,
    pub provenance: EvaluationProvenance,
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
    /// SHA-256 of the complete serialized suite, including provenance and
    /// judgments. This detects fixture edits that forgot to bump a revision.
    pub suite_sha256: String,
    pub strategy: StrategyRef,
    pub cutoffs: Vec<usize>,
    pub cases: Vec<CaseReport>,
    pub aggregate: BTreeMap<usize, AggregateMetrics>,
    pub mean_elapsed_ms: f64,
}

/// Return the stable SHA-256 identity used to bind reports to an exact suite.
pub fn suite_sha256(suite: &EvaluationSuite) -> Result<String, SearchError> {
    let encoded = serde_json::to_vec(suite).map_err(|error| {
        SearchError::InvalidRequest(format!("evaluation suite cannot be serialized: {error}"))
    })?;
    let digest = Sha256::digest(encoded);
    let mut encoded_digest = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded_digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded_digest)
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
    let mut cutoffs = config.cutoffs.clone();
    cutoffs.sort_unstable();
    let max_cutoff = *config
        .cutoffs
        .iter()
        .max()
        .expect("validation rejects empty cutoffs");
    let mut cases = Vec::with_capacity(suite.cases.len());
    let expected_strategy = StrategyRef {
        id: strategy.descriptor().id.clone(),
        version: strategy.descriptor().version.clone(),
    };

    for case in &suite.cases {
        let mut request = case.request.clone();
        request.strategy = Some(strategy.descriptor().id.clone());
        request.page.limit = max_cutoff;
        request.page.cursor = None;
        let started = Instant::now();
        let page = strategy.search(context.clone(), request).await?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        if page.strategy != expected_strategy {
            return Err(SearchError::Backend(format!(
                "strategy returned identity {:?}@{:?}, expected {:?}@{:?}",
                page.strategy.id,
                page.strategy.version,
                expected_strategy.id,
                expected_strategy.version
            )));
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

    Ok(EvaluationReport {
        suite_id: suite.id.clone(),
        suite_revision: suite.revision.clone(),
        suite_sha256: suite_sha256(suite)?,
        strategy: expected_strategy,
        cutoffs: cutoffs.clone(),
        aggregate: aggregate(&cases, &cutoffs),
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
    /// Reject evidence sets that are too small for the intended rollout.
    pub min_cases: usize,
    /// Minimum required candidate minus baseline mean nDCG.
    pub min_mean_ndcg_delta: f64,
    /// Per-query nDCG drops at or below this tolerance count as ties.
    pub regression_tolerance: f64,
    /// Maximum fraction of cases allowed to regress beyond the tolerance.
    pub max_regressed_fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub suite_sha256: String,
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
    pub cases: Vec<CaseComparison>,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaseComparison {
    pub case_id: String,
    pub precision_delta: f64,
    pub recall_delta: f64,
    pub reciprocal_rank_delta: f64,
    pub ndcg_delta: f64,
}

/// Verify a persisted report from its primary evidence: exact suite,
/// per-query returned IDs, judgments, and timings. Cached case and aggregate
/// metrics are rejected when they do not reproduce.
pub fn verify_report(
    suite: &EvaluationSuite,
    report: &EvaluationReport,
) -> Result<(), SearchError> {
    let config = EvaluationConfig {
        cutoffs: report.cutoffs.clone(),
    };
    validate_suite(suite, &config)?;
    if report.suite_id != suite.id || report.suite_revision != suite.revision {
        return Err(SearchError::InvalidRequest(
            "evaluation report does not identify the supplied suite".to_owned(),
        ));
    }
    if report.strategy.id.trim().is_empty() || report.strategy.version.trim().is_empty() {
        return Err(SearchError::InvalidRequest(
            "evaluation report strategy identity cannot be empty".to_owned(),
        ));
    }
    let expected_sha256 = suite_sha256(suite)?;
    if report.suite_sha256 != expected_sha256 {
        return Err(SearchError::InvalidRequest(format!(
            "evaluation report suite fingerprint does not match {expected_sha256}"
        )));
    }

    let report_cases = report_case_map(report)?;
    if report_cases.len() != suite.cases.len() {
        return Err(SearchError::InvalidRequest(
            "evaluation report contains a different case set".to_owned(),
        ));
    }

    let mut recomputed_cases = Vec::with_capacity(suite.cases.len());
    for case in &suite.cases {
        let observed = report_cases.get(case.id.as_str()).ok_or_else(|| {
            SearchError::InvalidRequest(format!("evaluation report is missing case {:?}", case.id))
        })?;
        reject_duplicate_results(&case.id, &observed.returned)?;
        if !observed.elapsed_ms.is_finite() || observed.elapsed_ms < 0.0 {
            return Err(SearchError::InvalidRequest(format!(
                "evaluation case {:?} has an invalid elapsed time",
                case.id
            )));
        }
        let metrics = metrics_at_cutoffs(&observed.returned, &case.judgments, &report.cutoffs);
        if observed.metrics != metrics {
            return Err(SearchError::InvalidRequest(format!(
                "evaluation case {:?} metrics do not reproduce from returned IDs",
                case.id
            )));
        }
        recomputed_cases.push(CaseReport {
            case_id: case.id.clone(),
            returned: observed.returned.clone(),
            metrics,
            elapsed_ms: observed.elapsed_ms,
        });
    }

    if report.aggregate != aggregate(&recomputed_cases, &report.cutoffs) {
        return Err(SearchError::InvalidRequest(
            "evaluation aggregate metrics do not reproduce from cases".to_owned(),
        ));
    }
    if report.mean_elapsed_ms != mean(recomputed_cases.iter().map(|case| case.elapsed_ms)) {
        return Err(SearchError::InvalidRequest(
            "evaluation mean latency does not reproduce from cases".to_owned(),
        ));
    }
    Ok(())
}

/// Compare the same cases pairwise. Reports from different fixture revisions
/// are rejected so a changed benchmark cannot masquerade as a strategy gain.
pub fn compare(
    suite: &EvaluationSuite,
    baseline: &EvaluationReport,
    candidate: &EvaluationReport,
    gate: &QualityGate,
) -> Result<ComparisonReport, SearchError> {
    verify_report(suite, baseline)?;
    verify_report(suite, candidate)?;
    if baseline.suite_id != candidate.suite_id
        || baseline.suite_revision != candidate.suite_revision
        || baseline.suite_sha256 != candidate.suite_sha256
    {
        return Err(SearchError::InvalidRequest(
            "evaluation reports use different suite identities or revisions".to_owned(),
        ));
    }
    if gate.cutoff == 0
        || gate.min_cases == 0
        || !gate.max_regressed_fraction.is_finite()
        || !(0.0..=1.0).contains(&gate.max_regressed_fraction)
        || !gate.regression_tolerance.is_finite()
        || gate.regression_tolerance < 0.0
        || !gate.min_mean_ndcg_delta.is_finite()
    {
        return Err(SearchError::InvalidRequest(
            "quality gate regression bounds are invalid".to_owned(),
        ));
    }
    if suite.cases.len() < gate.min_cases {
        return Err(SearchError::InvalidRequest(format!(
            "quality gate requires at least {} cases but suite has {}",
            gate.min_cases,
            suite.cases.len()
        )));
    }
    if !baseline.cutoffs.contains(&gate.cutoff) || !candidate.cutoffs.contains(&gate.cutoff) {
        return Err(SearchError::InvalidRequest(format!(
            "evaluation reports do not both contain cutoff {}",
            gate.cutoff
        )));
    }
    let baseline_cases = report_case_map(baseline)?;
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
    let mut case_comparisons = Vec::with_capacity(suite.cases.len());
    for suite_case in &suite.cases {
        let baseline_case = baseline_cases
            .get(suite_case.id.as_str())
            .expect("verified report contains every suite case");
        let candidate_case = candidate_cases
            .get(suite_case.id.as_str())
            .expect("verified report contains every suite case");
        let baseline_metrics = metrics_at_cutoffs(
            &baseline_case.returned,
            &suite_case.judgments,
            &[gate.cutoff],
        )[&gate.cutoff]
            .clone();
        let candidate_metrics = metrics_at_cutoffs(
            &candidate_case.returned,
            &suite_case.judgments,
            &[gate.cutoff],
        )[&gate.cutoff]
            .clone();
        let comparison = CaseComparison {
            case_id: suite_case.id.clone(),
            precision_delta: candidate_metrics.precision - baseline_metrics.precision,
            recall_delta: candidate_metrics.recall - baseline_metrics.recall,
            reciprocal_rank_delta: candidate_metrics.reciprocal_rank
                - baseline_metrics.reciprocal_rank,
            ndcg_delta: candidate_metrics.ndcg - baseline_metrics.ndcg,
        };
        if comparison.ndcg_delta > gate.regression_tolerance {
            wins += 1;
        } else if comparison.ndcg_delta < -gate.regression_tolerance {
            regressions += 1;
        } else {
            ties += 1;
        }
        case_comparisons.push(comparison);
    }
    let regressed_fraction = if baseline.cases.is_empty() {
        0.0
    } else {
        regressions as f64 / baseline.cases.len() as f64
    };
    let mean_precision_delta = mean(case_comparisons.iter().map(|case| case.precision_delta));
    let mean_recall_delta = mean(case_comparisons.iter().map(|case| case.recall_delta));
    let mean_reciprocal_rank_delta = mean(
        case_comparisons
            .iter()
            .map(|case| case.reciprocal_rank_delta),
    );
    let mean_ndcg_delta = mean(case_comparisons.iter().map(|case| case.ndcg_delta));

    Ok(ComparisonReport {
        suite_sha256: baseline.suite_sha256.clone(),
        cutoff: gate.cutoff,
        baseline_strategy: baseline.strategy.clone(),
        candidate_strategy: candidate.strategy.clone(),
        mean_precision_delta,
        mean_recall_delta,
        mean_reciprocal_rank_delta,
        mean_ndcg_delta,
        wins,
        ties,
        regressions,
        regressed_fraction,
        cases: case_comparisons,
        accepted: mean_ndcg_delta >= gate.min_mean_ndcg_delta
            && regressed_fraction <= gate.max_regressed_fraction,
    })
}

/// Validate fixture schema, provenance, case identities, judgments, and metric
/// cutoffs before executing a strategy or accepting a persisted report.
pub fn validate_suite(
    suite: &EvaluationSuite,
    config: &EvaluationConfig,
) -> Result<(), SearchError> {
    if suite.schema_version != EVALUATION_SUITE_SCHEMA_VERSION {
        return Err(SearchError::InvalidRequest(format!(
            "unsupported evaluation suite schema version {}",
            suite.schema_version
        )));
    }
    if suite.id.trim().is_empty() || suite.revision.trim().is_empty() || suite.cases.is_empty() {
        return Err(SearchError::InvalidRequest(
            "evaluation suite id, revision, and cases cannot be empty".to_owned(),
        ));
    }
    let provenance = &suite.provenance;
    if provenance.corpus_source.trim().is_empty()
        || provenance.corpus_revision.trim().is_empty()
        || provenance.judgments_source.trim().is_empty()
        || provenance.judgments_method.trim().is_empty()
        || provenance.corpus_sha256.len() != 64
        || !provenance
            .corpus_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SearchError::InvalidRequest(
            "evaluation suite provenance is incomplete or invalid".to_owned(),
        ));
    }
    if config.cutoffs.is_empty() || config.cutoffs.contains(&0) {
        return Err(SearchError::InvalidRequest(
            "evaluation cutoffs must be non-empty and greater than zero".to_owned(),
        ));
    }
    let mut cutoffs = HashSet::new();
    if config.cutoffs.iter().any(|cutoff| !cutoffs.insert(cutoff)) {
        return Err(SearchError::InvalidRequest(
            "evaluation cutoffs cannot contain duplicates".to_owned(),
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
        if !case.judgments.iter().any(|judgment| judgment.relevance > 0) {
            return Err(SearchError::InvalidRequest(format!(
                "evaluation case {:?} has no relevant judgment",
                case.id
            )));
        }
    }
    Ok(())
}

fn report_case_map(report: &EvaluationReport) -> Result<HashMap<&str, &CaseReport>, SearchError> {
    let mut cases = HashMap::with_capacity(report.cases.len());
    for case in &report.cases {
        if case.case_id.trim().is_empty() || cases.insert(case.case_id.as_str(), case).is_some() {
            return Err(SearchError::InvalidRequest(
                "evaluation report contains an empty or duplicate case id".to_owned(),
            ));
        }
    }
    Ok(cases)
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

    fn suite() -> EvaluationSuite {
        EvaluationSuite {
            schema_version: EVALUATION_SUITE_SCHEMA_VERSION,
            id: "suite".to_owned(),
            revision: "r1".to_owned(),
            provenance: EvaluationProvenance {
                corpus_source: "fixture://unit-test".to_owned(),
                corpus_revision: "1".to_owned(),
                corpus_sha256: "0".repeat(64),
                judgments_source: "unit test".to_owned(),
                judgments_method: "Explicit graded labels for deterministic rankings".to_owned(),
            },
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
                judgments: vec![
                    RelevanceJudgment {
                        document_id: id("best"),
                        relevance: 3,
                    },
                    RelevanceJudgment {
                        document_id: id("good"),
                        relevance: 1,
                    },
                ],
            }],
        }
    }

    fn report(suite: &EvaluationSuite, strategy: &str, order: &[&str]) -> EvaluationReport {
        let returned = order.iter().map(|value| id(value)).collect::<Vec<_>>();
        let case = CaseReport {
            case_id: "q1".to_owned(),
            metrics: metrics_at_cutoffs(&returned, &suite.cases[0].judgments, &[1, 3]),
            returned,
            elapsed_ms: 1.0,
        };
        let aggregate = aggregate(std::slice::from_ref(&case), &[1, 3]);
        EvaluationReport {
            suite_id: "suite".to_owned(),
            suite_revision: "r1".to_owned(),
            suite_sha256: suite_sha256(suite).unwrap(),
            strategy: StrategyRef {
                id: strategy.to_owned(),
                version: "1".to_owned(),
            },
            cutoffs: vec![1, 3],
            cases: vec![case],
            aggregate,
            mean_elapsed_ms: 1.0,
        }
    }

    #[test]
    fn paired_gate_accepts_a_material_improvement() {
        let suite = suite();
        let baseline = report(&suite, "baseline", &["noise", "good", "best"]);
        let candidate = report(&suite, "candidate", &["best", "good", "noise"]);
        let comparison = compare(
            &suite,
            &baseline,
            &candidate,
            &QualityGate {
                cutoff: 3,
                min_cases: 1,
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

    #[test]
    fn report_verification_rejects_cached_metric_edits() {
        let suite = suite();
        let mut report = report(&suite, "candidate", &["best", "good", "noise"]);
        report.aggregate.get_mut(&3).unwrap().mean_ndcg = 0.0;

        let error = verify_report(&suite, &report).unwrap_err();
        assert!(error
            .to_string()
            .contains("aggregate metrics do not reproduce"));
    }

    #[test]
    fn suite_fingerprint_binds_judgments_even_without_revision_change() {
        let suite = suite();
        let mut changed = suite.clone();
        changed.cases[0].judgments[0].relevance = 2;

        assert_ne!(
            suite_sha256(&suite).unwrap(),
            suite_sha256(&changed).unwrap()
        );
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
                items: ["best", "good", "noise"]
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
        let suite = suite();
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
