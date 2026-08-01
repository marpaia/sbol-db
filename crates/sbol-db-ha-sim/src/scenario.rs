use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{TimeZone, Utc};
use sbol_db_core::ConfigEntry;
use sbol_db_raft::{CommandEnvelope, CommandOutcome, ReplicatedCommand};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::cluster::{SimulatedCluster, WriteAck};
use crate::corpus::Corpus;

#[derive(Clone, Debug)]
pub struct ScenarioConfig {
    pub seed: u64,
    pub retry_every: usize,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            seed: 0x5b01_db00_0000_0001,
            retry_every: 37,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SimulationReport {
    pub schema_version: u16,
    pub seed: u64,
    pub corpus_manifest_id: String,
    pub corpus_manifest_revision: String,
    pub corpus_commit: String,
    pub corpus_fingerprint: String,
    pub document_count: usize,
    pub acknowledged_document_writes: usize,
    pub final_state_sha256: String,
    pub duration_ms: u128,
    pub events: Vec<TraceEvent>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceEvent {
    pub sequence: usize,
    #[serde(flatten)]
    pub kind: TraceEventKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TraceEventKind {
    ClusterStarted {
        cluster_id: Uuid,
        leader_id: u64,
    },
    FaultSchedule {
        follower_isolation_after: usize,
        follower_heal_after: usize,
        leader_stop_after: usize,
        ambiguous_retry_after: usize,
        minority_partition_after: usize,
        follower_stop_after: usize,
        follower_restart_after: usize,
    },
    CorpusWriteAcknowledged {
        ordinal: usize,
        relative_path: String,
        request_id: Uuid,
        leader_id: u64,
        log_index: u64,
    },
    ExactRetryAcknowledged {
        ordinal: usize,
        request_id: Uuid,
        original_applied_log_index: u64,
        retry_log_index: u64,
    },
    FollowerIsolated {
        node_id: u64,
    },
    NetworkHealed,
    NetworkDelayInjected {
        leader_id: u64,
        delay_ms: u64,
    },
    LeaderStopped {
        node_id: u64,
    },
    NodeRestarted {
        node_id: u64,
    },
    MinorityPartition {
        isolated_leader: u64,
        majority: Vec<u64>,
    },
    MinorityWriteNotAcknowledged {
        request_id: Uuid,
    },
    MajorityWriteAcknowledged {
        request_id: Uuid,
        leader_id: u64,
        log_index: u64,
    },
    AmbiguousWriteTimedOut {
        request_id: Uuid,
    },
    AmbiguousRetryAcknowledged {
        request_id: Uuid,
        original_applied_log_index: u64,
        retry_log_index: u64,
    },
    FollowerStopped {
        node_id: u64,
    },
    SnapshotTriggered {
        leader_id: u64,
    },
    FullClusterRestarted {
        leader_id: u64,
    },
    OracleVerified {
        node_ids: Vec<u64>,
        acknowledged_writes: usize,
        state_sha256: String,
    },
}

pub async fn run_corpus_chaos(corpus: &Corpus, config: ScenarioConfig) -> Result<SimulationReport> {
    if corpus.documents.len() < 60 {
        bail!(
            "the full fault schedule needs at least 60 documents, found {}",
            corpus.documents.len()
        );
    }
    let started = Instant::now();
    let cluster_id = deterministic_uuid(b"cluster", config.seed, corpus.fingerprint.as_bytes());
    let client_id = deterministic_uuid(b"client", config.seed, corpus.fingerprint.as_bytes());
    let mut cluster = SimulatedCluster::start(3, cluster_id)
        .await
        .context("starting three-node simulation")?;
    let mut trace = Trace::default();
    let initial_leader = cluster.wait_for_leader(None).await?;
    trace.push(TraceEventKind::ClusterStarted {
        cluster_id,
        leader_id: initial_leader,
    });

    let document_count = corpus.documents.len();
    let schedule = FaultSchedule::for_seed(document_count, config.seed);
    trace.push(TraceEventKind::FaultSchedule {
        follower_isolation_after: schedule.lag_start,
        follower_heal_after: schedule.lag_end,
        leader_stop_after: schedule.leader_stop,
        ambiguous_retry_after: schedule.ambiguous_retry,
        minority_partition_after: schedule.partition,
        follower_stop_after: schedule.offline_start,
        follower_restart_after: schedule.offline_end,
    });
    let mut isolated_follower = None;
    let mut stopped_follower = None;
    let mut expected = BTreeMap::<String, ConfigEntry>::new();
    let mut last_log_index = 0;

    for document in &corpus.documents {
        let request_id =
            deterministic_uuid(b"document", config.seed, document.relative_path.as_bytes());
        let key = format!(
            "ha-sim/corpus/{:04}-{}",
            document.ordinal,
            &document.sha256[..16]
        );
        let value = json!({
            "relative_path": document.relative_path,
            "format": document.format.as_db_str(),
            "sha256": document.sha256,
            "body": document.body,
            "object_count": document.object_count,
            "triple_count": document.triple_count,
        });
        let updated_at = Utc
            .timestamp_opt(1_700_000_000 + document.ordinal as i64, 0)
            .single()
            .expect("fixed simulation timestamp is valid");
        let command = CommandEnvelope::new(
            client_id,
            request_id,
            ReplicatedCommand::SetConfig {
                key: key.clone(),
                value: value.clone(),
                updated_at,
            },
        );
        let ack = submit_with_retry(&cluster, command.clone(), None).await?;
        verify_applied(&ack, request_id)?;
        last_log_index = last_log_index.max(ack.log_index);
        expected.insert(
            key.clone(),
            ConfigEntry {
                key,
                value,
                updated_at,
            },
        );
        trace.push(TraceEventKind::CorpusWriteAcknowledged {
            ordinal: document.ordinal,
            relative_path: document.relative_path.clone(),
            request_id,
            leader_id: ack.leader_id,
            log_index: ack.log_index,
        });

        if config.retry_every != 0 && (document.ordinal + 1) % config.retry_every == 0 {
            let retry = submit_with_retry(&cluster, command, None).await?;
            verify_applied(&retry, request_id)?;
            if retry.response.applied_log_index != ack.response.applied_log_index {
                bail!(
                    "exact retry for {} applied twice: original {}, retry {}",
                    document.relative_path,
                    ack.response.applied_log_index,
                    retry.response.applied_log_index
                );
            }
            if retry.response.applied_log_index >= retry.log_index {
                bail!(
                    "exact retry for {} did not append after the original result",
                    document.relative_path
                );
            }
            last_log_index = last_log_index.max(retry.log_index);
            trace.push(TraceEventKind::ExactRetryAcknowledged {
                ordinal: document.ordinal,
                request_id,
                original_applied_log_index: ack.response.applied_log_index,
                retry_log_index: retry.log_index,
            });
        }

        let completed = document.ordinal + 1;
        if completed == schedule.lag_start {
            let leader = cluster.wait_for_leader(None).await?;
            let follower = choose_other(&cluster.live_node_ids(), leader, config.seed);
            cluster.isolate(follower);
            isolated_follower = Some(follower);
            trace.push(TraceEventKind::FollowerIsolated { node_id: follower });
        }
        if completed == schedule.lag_end && isolated_follower.take().is_some() {
            cluster.heal();
            trace.push(TraceEventKind::NetworkHealed);
            cluster
                .wait_for_all_applied(last_log_index)
                .await
                .context("catching isolated follower up after healing")?;
        }
        if completed == schedule.leader_stop {
            let leader = cluster.wait_for_leader(None).await?;
            cluster.stop_node(leader).await?;
            trace.push(TraceEventKind::LeaderStopped { node_id: leader });
            cluster
                .wait_for_leader(None)
                .await
                .context("electing replacement after stopping leader")?;
            cluster.restart_node(leader).await?;
            trace.push(TraceEventKind::NodeRestarted { node_id: leader });
        }
        if completed == schedule.ambiguous_retry {
            inject_ambiguous_retry(
                &cluster,
                client_id,
                config.seed,
                &mut last_log_index,
                &mut trace,
            )
            .await?;
        }
        if completed == schedule.partition {
            inject_minority_partition(
                &mut cluster,
                client_id,
                config.seed,
                &mut last_log_index,
                &mut trace,
            )
            .await?;
        }
        if completed == schedule.offline_start {
            let leader = cluster.wait_for_leader(None).await?;
            let follower =
                choose_other(&cluster.live_node_ids(), leader, config.seed.rotate_left(7));
            cluster.stop_node(follower).await?;
            stopped_follower = Some(follower);
            trace.push(TraceEventKind::FollowerStopped { node_id: follower });
        }
        if completed == schedule.offline_end {
            if let Some(follower) = stopped_follower.take() {
                cluster.restart_node(follower).await?;
                trace.push(TraceEventKind::NodeRestarted { node_id: follower });
            }
        }
    }

    cluster.heal();
    if isolated_follower.take().is_some() {
        trace.push(TraceEventKind::NetworkHealed);
    }
    if let Some(follower) = stopped_follower.take() {
        cluster.restart_node(follower).await?;
        trace.push(TraceEventKind::NodeRestarted { node_id: follower });
    }
    let leader = cluster.wait_for_leader(None).await?;
    let final_barrier = barrier(client_id, config.seed, b"pre-snapshot");
    let barrier_ack = submit_with_retry(&cluster, final_barrier, None).await?;
    last_log_index = last_log_index.max(barrier_ack.log_index);
    cluster.wait_for_all_applied(last_log_index).await?;
    cluster.trigger_snapshot(leader).await?;
    trace.push(TraceEventKind::SnapshotTriggered { leader_id: leader });

    cluster
        .full_restart()
        .await
        .context("restarting the complete cluster from disk")?;
    let restarted_leader = cluster.wait_for_leader(None).await?;
    trace.push(TraceEventKind::FullClusterRestarted {
        leader_id: restarted_leader,
    });
    let restart_barrier = barrier(client_id, config.seed, b"post-restart");
    let restart_ack = submit_with_retry(&cluster, restart_barrier, None).await?;
    cluster.wait_for_all_applied(restart_ack.log_index).await?;

    let expected_digest = digest_expected(&expected)?;
    let live_nodes = cluster.live_node_ids().into_iter().collect::<Vec<_>>();
    for node_id in &live_nodes {
        let actual = cluster
            .config_entries(*node_id)?
            .into_iter()
            .map(|entry| (entry.key.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        if actual != expected {
            let missing = expected
                .keys()
                .filter(|key| !actual.contains_key(*key))
                .take(10)
                .cloned()
                .collect::<Vec<_>>();
            bail!(
                "node {node_id} diverged after recovery: expected {} values, found {}; first missing keys: {missing:?}",
                expected.len(),
                actual.len()
            );
        }
        let actual_digest = digest_expected(&actual)?;
        if actual_digest != expected_digest {
            bail!("node {node_id} state digest differs despite equal entry count");
        }
    }
    trace.push(TraceEventKind::OracleVerified {
        node_ids: live_nodes,
        acknowledged_writes: expected.len(),
        state_sha256: expected_digest.clone(),
    });

    let report = SimulationReport {
        schema_version: 1,
        seed: config.seed,
        corpus_manifest_id: corpus.manifest.id.clone(),
        corpus_manifest_revision: corpus.manifest.revision.clone(),
        corpus_commit: corpus.manifest.source.commit.clone(),
        corpus_fingerprint: corpus.fingerprint.clone(),
        document_count,
        acknowledged_document_writes: expected.len(),
        final_state_sha256: expected_digest,
        duration_ms: started.elapsed().as_millis(),
        events: trace.events,
    };
    cluster.shutdown().await?;
    Ok(report)
}

async fn inject_ambiguous_retry(
    cluster: &SimulatedCluster,
    client_id: Uuid,
    seed: u64,
    last_log_index: &mut u64,
    trace: &mut Trace,
) -> Result<()> {
    let leader = cluster.wait_for_leader(None).await?;
    let delay = Duration::from_millis(200);
    for node_id in cluster.live_node_ids() {
        if node_id != leader {
            cluster.set_delay(leader, node_id, delay);
            cluster.set_delay(node_id, leader, delay);
        }
    }
    trace.push(TraceEventKind::NetworkDelayInjected {
        leader_id: leader,
        delay_ms: delay.as_millis() as u64,
    });

    let command = barrier(client_id, seed, b"ambiguous-response");
    let request_id = command.request_id;
    let first = tokio::time::timeout(
        Duration::from_millis(25),
        cluster.write(leader, command.clone()),
    )
    .await;
    if first.is_ok() {
        bail!("delayed request {request_id} did not produce an ambiguous client timeout");
    }
    trace.push(TraceEventKind::AmbiguousWriteTimedOut { request_id });
    cluster.heal();
    trace.push(TraceEventKind::NetworkHealed);

    let retry = submit_with_retry(cluster, command, None).await?;
    verify_applied(&retry, request_id)?;
    if retry.response.applied_log_index >= retry.log_index {
        bail!("timed-out request {request_id} did not commit before its exact retry");
    }
    *last_log_index = (*last_log_index).max(retry.log_index);
    trace.push(TraceEventKind::AmbiguousRetryAcknowledged {
        request_id,
        original_applied_log_index: retry.response.applied_log_index,
        retry_log_index: retry.log_index,
    });
    Ok(())
}

async fn inject_minority_partition(
    cluster: &mut SimulatedCluster,
    client_id: Uuid,
    seed: u64,
    last_log_index: &mut u64,
    trace: &mut Trace,
) -> Result<()> {
    let isolated_leader = cluster.wait_for_leader(None).await?;
    let majority = cluster
        .live_node_ids()
        .into_iter()
        .filter(|node_id| *node_id != isolated_leader)
        .collect::<BTreeSet<_>>();
    cluster.partition(&[BTreeSet::from([isolated_leader]), majority.clone()]);
    trace.push(TraceEventKind::MinorityPartition {
        isolated_leader,
        majority: majority.iter().copied().collect(),
    });

    let minority_request = barrier(client_id, seed, b"minority-must-not-ack");
    let minority_request_id = minority_request.request_id;
    let minority_result = tokio::time::timeout(
        Duration::from_millis(900),
        cluster.write(isolated_leader, minority_request),
    )
    .await;
    if matches!(minority_result, Ok(Ok(_))) {
        bail!("a minority leader acknowledged request {minority_request_id}");
    }
    trace.push(TraceEventKind::MinorityWriteNotAcknowledged {
        request_id: minority_request_id,
    });

    let majority_request = barrier(client_id, seed, b"majority-progress");
    let majority_request_id = majority_request.request_id;
    let ack = submit_with_retry(cluster, majority_request, Some(&majority)).await?;
    verify_applied(&ack, majority_request_id)?;
    *last_log_index = (*last_log_index).max(ack.log_index);
    trace.push(TraceEventKind::MajorityWriteAcknowledged {
        request_id: majority_request_id,
        leader_id: ack.leader_id,
        log_index: ack.log_index,
    });
    cluster.heal();
    trace.push(TraceEventKind::NetworkHealed);
    cluster
        .wait_for_leader(None)
        .await
        .context("stabilizing cluster after healing minority partition")?;
    Ok(())
}

async fn submit_with_retry(
    cluster: &SimulatedCluster,
    command: CommandEnvelope,
    allowed: Option<&BTreeSet<u64>>,
) -> Result<WriteAck> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let leader = match cluster.wait_for_leader(allowed).await {
                Ok(leader) => leader,
                Err(_) => continue,
            };
            match tokio::time::timeout(
                Duration::from_secs(3),
                cluster.write(leader, command.clone()),
            )
            .await
            {
                Ok(Ok(ack)) => return Ok(ack),
                Ok(Err(_)) | Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    })
    .await
    .map_err(|_| anyhow!("request {} was not acknowledged", command.request_id))?
}

fn verify_applied(ack: &WriteAck, request_id: Uuid) -> Result<()> {
    if ack.response.request_id != Some(request_id)
        || ack.response.outcome != CommandOutcome::Applied
    {
        bail!(
            "request {request_id} returned unexpected response {:?}",
            ack.response
        );
    }
    Ok(())
}

fn barrier(client_id: Uuid, seed: u64, label: &[u8]) -> CommandEnvelope {
    CommandEnvelope::new(
        client_id,
        deterministic_uuid(b"barrier", seed, label),
        ReplicatedCommand::Barrier,
    )
}

fn choose_other(node_ids: &BTreeSet<u64>, excluded: u64, seed: u64) -> u64 {
    let candidates = node_ids
        .iter()
        .copied()
        .filter(|node_id| *node_id != excluded)
        .collect::<Vec<_>>();
    candidates[(seed as usize) % candidates.len()]
}

fn deterministic_uuid(domain: &[u8], seed: u64, input: &[u8]) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(seed.to_be_bytes());
    hash.update(input);
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(Clone, Copy, Debug)]
struct FaultSchedule {
    lag_start: usize,
    lag_end: usize,
    leader_stop: usize,
    ambiguous_retry: usize,
    partition: usize,
    offline_start: usize,
    offline_end: usize,
}

impl FaultSchedule {
    fn for_seed(total: usize, seed: u64) -> Self {
        let mut random = SplitMix64::new(seed ^ 0xa076_1d64_78bd_642f);
        Self {
            lag_start: position_in_percent_range(total, 8, 12, &mut random),
            lag_end: position_in_percent_range(total, 30, 35, &mut random),
            leader_stop: position_in_percent_range(total, 39, 44, &mut random),
            ambiguous_retry: position_in_percent_range(total, 48, 54, &mut random),
            partition: position_in_percent_range(total, 58, 64, &mut random),
            offline_start: position_in_percent_range(total, 68, 74, &mut random),
            offline_end: position_in_percent_range(total, 88, 92, &mut random),
        }
    }
}

fn position_in_percent_range(
    total: usize,
    lower_percent: usize,
    upper_percent: usize,
    random: &mut SplitMix64,
) -> usize {
    let lower = (total * lower_percent / 100).max(1);
    let upper = (total * upper_percent / 100).max(lower);
    lower + (random.next() as usize % (upper - lower + 1))
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

fn digest_expected(entries: &BTreeMap<String, ConfigEntry>) -> Result<String> {
    let encoded = serde_json::to_vec(entries).context("serializing state oracle")?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

#[derive(Default)]
struct Trace {
    events: Vec<TraceEvent>,
}

impl Trace {
    fn push(&mut self, kind: TraceEventKind) {
        self.events.push(TraceEvent {
            sequence: self.events.len(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ids_are_stable_and_domain_separated() {
        assert_eq!(
            deterministic_uuid(b"document", 7, b"SBOL2/a.xml"),
            deterministic_uuid(b"document", 7, b"SBOL2/a.xml")
        );
        assert_ne!(
            deterministic_uuid(b"document", 7, b"SBOL2/a.xml"),
            deterministic_uuid(b"barrier", 7, b"SBOL2/a.xml")
        );
    }

    #[test]
    fn seeded_fault_schedules_vary_without_overlapping_phases() {
        let first = FaultSchedule::for_seed(447, 1);
        let second = FaultSchedule::for_seed(447, 2);
        assert!(first.lag_start < first.lag_end);
        assert!(first.lag_end < first.leader_stop);
        assert!(first.leader_stop < first.ambiguous_retry);
        assert!(first.ambiguous_retry < first.partition);
        assert!(first.partition < first.offline_start);
        assert!(first.offline_start < first.offline_end);
        assert_ne!(
            (
                first.lag_start,
                first.lag_end,
                first.leader_stop,
                first.partition
            ),
            (
                second.lag_start,
                second.lag_end,
                second.leader_stop,
                second.partition
            )
        );
    }
}
