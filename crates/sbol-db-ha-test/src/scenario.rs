use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{TimeZone, Utc};
use sbol_db_ha_sim::Corpus;
use sbol_db_raft::{CommandEnvelope, CommandOutcome, ReplicatedCommand, StateAudit};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::process::config_command;
use crate::{
    check_register_linearizable, ArtifactBundle, CommandReply, Completion, FailpointAction,
    HistoryRecorder, LinearizabilityReport, Operation, ProcessClient, ProcessCluster,
    ProcessClusterConfig, RunManifest,
};

const CONCURRENT_KEY_PREFIX: &str = "ha-process/concurrent/";

#[derive(Clone, Debug)]
pub struct ProcessChaosConfig {
    pub seed: u64,
    pub retry_every: usize,
    pub node_binary: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessChaosReport {
    pub schema_version: u16,
    pub seed: u64,
    pub cluster_id: Uuid,
    pub corpus_commit: String,
    pub corpus_fingerprint: String,
    pub document_count: usize,
    pub acknowledged_document_writes: usize,
    pub exact_retries: usize,
    pub ambiguous_retries: usize,
    pub linearizability: LinearizabilityReport,
    pub node_ids: Vec<u64>,
    pub final_state_sha256: String,
    pub duration_ms: u128,
}

pub async fn run_process_chaos(
    corpus: &Corpus,
    config: ProcessChaosConfig,
) -> Result<ProcessChaosReport> {
    if corpus.documents.len() < 60 {
        bail!(
            "the process fault schedule needs at least 60 documents, found {}",
            corpus.documents.len()
        );
    }
    let started = Instant::now();
    let cluster_id = deterministic_uuid(
        b"process-cluster",
        config.seed,
        corpus.fingerprint.as_bytes(),
    );
    let client_id = deterministic_uuid(
        b"process-client",
        config.seed,
        corpus.fingerprint.as_bytes(),
    );
    let run_id = deterministic_uuid(b"process-run", config.seed, corpus.fingerprint.as_bytes());
    let manifest = RunManifest {
        schema_version: 1,
        run_id,
        scenario: "single-host-process-chaos".to_owned(),
        seed: config.seed,
        driver: "process".to_owned(),
        cluster_id,
        node_count: 3,
        corpus_commit: Some(corpus.manifest.source.commit.clone()),
        corpus_fingerprint: Some(corpus.fingerprint.clone()),
    };
    let artifacts = ArtifactBundle::create(&config.artifact_root, &manifest)?;
    let history = HistoryRecorder::default();
    let cluster_config = ProcessClusterConfig {
        node_binary: config.node_binary,
        root: config.artifact_root.clone(),
        cluster_id,
        bearer_token: format!("ha-test-{run_id}"),
        node_count: 3,
    };
    let mut cluster = ProcessCluster::start(cluster_config).await?;
    let result = run_schedule(
        &mut cluster,
        corpus,
        config.seed,
        config.retry_every,
        client_id,
        &history,
        started,
    )
    .await;
    artifacts.write_history(&history.snapshot())?;
    match result {
        Ok(report) => {
            artifacts.write_json("report.json", &report)?;
            artifacts.write_json(
                "checker.json",
                &json!({"status": "valid", "state_sha256": report.final_state_sha256}),
            )?;
            cluster.shutdown().await?;
            Ok(report)
        }
        Err(error) => {
            artifacts.write_json(
                "checker.json",
                &json!({"status": "invalid", "error": format!("{error:#}")}),
            )?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_schedule(
    cluster: &mut ProcessCluster,
    corpus: &Corpus,
    seed: u64,
    retry_every: usize,
    client_id: Uuid,
    history: &HistoryRecorder,
    started: Instant,
) -> Result<ProcessChaosReport> {
    let count = corpus.documents.len();
    let lag_start = count / 10;
    let lag_end = count / 3;
    let leader_stop = count * 2 / 5;
    let ambiguous = count / 2;
    let partition = count * 3 / 5;
    let follower_stop = count * 7 / 10;
    let follower_restart = count * 9 / 10;
    let linearizability = run_concurrent_register(cluster, seed, history).await?;
    let mut leader = cluster
        .wait_for_writable_leader(None, Duration::from_secs(15))
        .await?;
    let mut isolated_follower = None;
    let mut stopped_follower = None;
    let mut expected = BTreeMap::<String, Value>::new();
    let mut exact_retries = 0;
    let mut ambiguous_retries = 0;
    let mut last_index = 0;

    for document in &corpus.documents {
        if document.ordinal == lag_start {
            let follower = follower_other_than(cluster, leader, None)?;
            cluster.isolate(follower);
            isolated_follower = Some(follower);
        }
        if document.ordinal == lag_end {
            cluster.heal();
            let barrier = barrier(client_id, seed, b"lag-healed");
            let (new_leader, index) =
                submit(cluster, leader, barrier, Operation::Barrier, history).await?;
            leader = new_leader;
            last_index = last_index.max(index);
            cluster
                .wait_for_applied(&cluster.node_ids(), index, Duration::from_secs(180))
                .await?;
            isolated_follower = None;
        }

        let key = format!(
            "ha-process/corpus/{:04}-{}",
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
        let request_id = deterministic_uuid(b"document", seed, document.relative_path.as_bytes());
        let updated_at = Utc
            .timestamp_opt(1_700_000_000 + document.ordinal as i64, 0)
            .single()
            .expect("fixed process test timestamp is valid");
        let envelope = CommandEnvelope::new(
            client_id,
            request_id,
            ReplicatedCommand::SetConfig {
                key: key.clone(),
                value: value.clone(),
                updated_at,
            },
        );
        let operation = Operation::Set {
            key: key.clone(),
            // Keep histories useful and bounded without duplicating entire SBOL
            // documents. The replicated value and the end-state verifier still
            // retain and compare the complete payload.
            value: json!({
                "relative_path": document.relative_path,
                "sha256": document.sha256,
                "bytes": document.body.len(),
                "object_count": document.object_count,
                "triple_count": document.triple_count,
            }),
        };
        let (new_leader, index) = submit(
            cluster,
            leader,
            envelope.clone(),
            operation.clone(),
            history,
        )
        .await?;
        leader = new_leader;
        last_index = last_index.max(index);
        expected.insert(key, value);

        if retry_every != 0 && (document.ordinal + 1) % retry_every == 0 {
            let (new_leader, retry_index) =
                submit(cluster, leader, envelope, operation, history).await?;
            leader = new_leader;
            last_index = last_index.max(retry_index);
            exact_retries += 1;
        }

        if document.ordinal == leader_stop {
            let stopped = leader;
            cluster.kill(stopped).await?;
            let allowed = cluster
                .node_ids()
                .into_iter()
                .filter(|node_id| *node_id != stopped)
                .collect::<BTreeSet<_>>();
            leader = cluster
                .wait_for_writable_leader(Some(&allowed), Duration::from_secs(15))
                .await?;
            cluster.restart(stopped).await?;
        }

        if document.ordinal == ambiguous {
            let key = "ha-process/ambiguous-response".to_owned();
            let value = json!({"seed": seed});
            let request_id = deterministic_uuid(b"ambiguous", seed, b"response");
            let envelope = config_command(client_id, request_id, &key, value.clone());
            let operation = Operation::Set {
                key: key.clone(),
                value: value.clone(),
            };
            cluster.set_failpoint(leader, FailpointAction::Arm).await?;
            let operation_id = Uuid::new_v4();
            history.invoke(
                operation_id,
                client_id,
                request_id,
                leader,
                operation.clone(),
            );
            let task = cluster.spawn_command(leader, envelope.clone())?;
            cluster
                .wait_for_failpoint_hit(leader, Duration::from_secs(10))
                .await?;
            let stopped = leader;
            cluster.kill(stopped).await?;
            history.complete(
                operation_id,
                Completion::Indeterminate {
                    message: "leader killed after apply before response".to_owned(),
                },
            );
            let _ = task.await;
            let allowed = cluster
                .node_ids()
                .into_iter()
                .filter(|node_id| *node_id != stopped)
                .collect::<BTreeSet<_>>();
            leader = cluster
                .wait_for_writable_leader(Some(&allowed), Duration::from_secs(15))
                .await?;
            let (new_leader, index) = submit(cluster, leader, envelope, operation, history).await?;
            leader = new_leader;
            last_index = last_index.max(index);
            expected.insert(key, value);
            ambiguous_retries += 1;
            cluster.restart(stopped).await?;
        }

        if document.ordinal == partition {
            let isolated = leader;
            let majority = cluster
                .node_ids()
                .into_iter()
                .filter(|node_id| *node_id != isolated)
                .collect::<BTreeSet<_>>();
            cluster.partition(&[BTreeSet::from([isolated]), majority.clone()]);
            let key = "ha-process/minority-resolution".to_owned();
            let value = json!({"isolated_leader": isolated});
            let request_id = deterministic_uuid(b"partition", seed, b"minority");
            let envelope = config_command(client_id, request_id, &key, value.clone());
            let operation = Operation::Set {
                key: key.clone(),
                value: value.clone(),
            };
            let operation_id = Uuid::new_v4();
            history.invoke(
                operation_id,
                client_id,
                request_id,
                isolated,
                operation.clone(),
            );
            let task = cluster.spawn_command(isolated, envelope.clone())?;
            leader = cluster
                .wait_for_writable_leader(Some(&majority), Duration::from_secs(15))
                .await?;
            match tokio::time::timeout(Duration::from_millis(750), task).await {
                Ok(Ok(Ok(CommandReply::Applied { .. }))) => {
                    bail!("the isolated minority leader acknowledged a write")
                }
                result => {
                    history.complete(
                        operation_id,
                        Completion::Indeterminate {
                            message: format!("minority request unresolved: {result:?}"),
                        },
                    );
                }
            }
            let (new_leader, index) = submit(cluster, leader, envelope, operation, history).await?;
            leader = new_leader;
            last_index = last_index.max(index);
            expected.insert(key, value);
            cluster.heal();
        }

        if document.ordinal == follower_stop {
            let follower = follower_other_than(cluster, leader, isolated_follower)?;
            cluster.kill(follower).await?;
            stopped_follower = Some(follower);
        }
        if document.ordinal == follower_restart {
            if let Some(follower) = stopped_follower.take() {
                cluster.restart(follower).await?;
            }
        }
    }

    cluster.heal();
    if let Some(follower) = stopped_follower.take() {
        cluster.restart(follower).await?;
    }
    let final_barrier = barrier(client_id, seed, b"pre-snapshot");
    let (new_leader, index) =
        submit(cluster, leader, final_barrier, Operation::Barrier, history).await?;
    leader = new_leader;
    last_index = last_index.max(index);
    cluster
        .wait_for_applied(&cluster.node_ids(), last_index, Duration::from_secs(180))
        .await?;
    cluster.trigger_snapshot(leader).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    cluster.full_restart().await?;
    leader = cluster
        .wait_for_writable_leader(None, Duration::from_secs(15))
        .await?;
    let barrier = barrier(client_id, seed, b"post-restart");
    let (new_leader, final_index) =
        submit(cluster, leader, barrier, Operation::Barrier, history).await?;
    leader = new_leader;
    cluster
        .wait_for_applied(&cluster.node_ids(), final_index, Duration::from_secs(180))
        .await?;

    for (key, value) in &expected {
        let actual = cluster
            .read_config(leader, key)
            .await
            .with_context(|| format!("reading acknowledged key {key}"))?;
        if actual.value.as_ref() != Some(value) {
            bail!("acknowledged key {key} was lost or changed after recovery");
        }
    }
    let mut audits = BTreeMap::<u64, StateAudit>::new();
    for node_id in cluster.node_ids() {
        audits.insert(node_id, cluster.audit(node_id).await?);
    }
    let first = audits
        .get(&1)
        .ok_or_else(|| anyhow!("node 1 audit missing"))?;
    for (node_id, audit) in &audits {
        if audit != first {
            bail!("node {node_id} state audit differs after recovery");
        }
    }

    Ok(ProcessChaosReport {
        schema_version: 1,
        seed,
        cluster_id: cluster.cluster_id(),
        corpus_commit: corpus.manifest.source.commit.clone(),
        corpus_fingerprint: corpus.fingerprint.clone(),
        document_count: count,
        acknowledged_document_writes: count,
        exact_retries,
        ambiguous_retries,
        linearizability,
        node_ids: cluster.node_ids().into_iter().collect(),
        final_state_sha256: hex::encode(first.sha256),
        duration_ms: started.elapsed().as_millis(),
    })
}

async fn run_concurrent_register(
    cluster: &mut ProcessCluster,
    seed: u64,
    history: &HistoryRecorder,
) -> Result<LinearizabilityReport> {
    let client = cluster.client_handle();
    let mut workloads = Vec::new();
    for client_ordinal in 0..4_u64 {
        workloads.push(tokio::spawn(concurrent_client(
            client.clone(),
            history.clone(),
            seed,
            client_ordinal,
        )));
    }

    tokio::time::sleep(Duration::from_millis(55)).await;
    let stopped_leader = cluster
        .wait_for_writable_leader(None, Duration::from_secs(10))
        .await?;
    cluster.kill(stopped_leader).await?;
    let majority = cluster
        .node_ids()
        .into_iter()
        .filter(|node_id| *node_id != stopped_leader)
        .collect::<BTreeSet<_>>();
    cluster
        .wait_for_writable_leader(Some(&majority), Duration::from_secs(10))
        .await?;
    cluster.restart(stopped_leader).await?;

    tokio::time::sleep(Duration::from_millis(55)).await;
    let isolated_leader = cluster
        .wait_for_writable_leader(None, Duration::from_secs(10))
        .await?;
    let majority = cluster
        .node_ids()
        .into_iter()
        .filter(|node_id| *node_id != isolated_leader)
        .collect::<BTreeSet<_>>();
    cluster.partition(&[BTreeSet::from([isolated_leader]), majority.clone()]);
    cluster
        .wait_for_writable_leader(Some(&majority), Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(75)).await;
    cluster.heal();

    for workload in workloads {
        workload
            .await
            .context("concurrent client task panicked")??;
    }
    let report = check_register_linearizable(&history.snapshot(), CONCURRENT_KEY_PREFIX);
    if !report.valid {
        bail!(
            "concurrent register history is not linearizable: {}",
            report.error.as_deref().unwrap_or("unknown checker error")
        );
    }
    Ok(report)
}

async fn concurrent_client(
    client: ProcessClient,
    history: HistoryRecorder,
    seed: u64,
    client_ordinal: u64,
) -> Result<()> {
    let client_id = deterministic_uuid(b"concurrent-client", seed, client_ordinal.to_be_bytes());
    for step in 0..12_u64 {
        let key = format!("{CONCURRENT_KEY_PREFIX}{}", step % 2);
        let request_id = deterministic_uuid(
            b"concurrent-request",
            seed,
            [client_ordinal.to_be_bytes(), step.to_be_bytes()].concat(),
        );
        let operation_id = deterministic_uuid(
            b"concurrent-operation",
            seed,
            [client_ordinal.to_be_bytes(), step.to_be_bytes()].concat(),
        );
        let mut target = (client_ordinal + step) % 3 + 1;
        let deadline = Instant::now() + Duration::from_secs(15);
        if step % 3 != 2 {
            let value = json!({"client": client_ordinal, "step": step});
            let updated_at = Utc
                .timestamp_opt(1_800_000_000 + (client_ordinal * 100 + step) as i64, 0)
                .single()
                .expect("fixed concurrent timestamp is valid");
            let request = CommandEnvelope::new(
                client_id,
                request_id,
                ReplicatedCommand::SetConfig {
                    key: key.clone(),
                    value: value.clone(),
                    updated_at,
                },
            );
            history.invoke(
                operation_id,
                client_id,
                request_id,
                target,
                Operation::Set { key, value },
            );
            loop {
                match client.command(target, &request).await {
                    Ok(CommandReply::Applied {
                        log_index,
                        response,
                    }) if response.outcome == CommandOutcome::Applied => {
                        history.complete(
                            operation_id,
                            Completion::Applied {
                                log_index,
                                value: None,
                            },
                        );
                        break;
                    }
                    Ok(CommandReply::NotLeader { leader_id, .. }) => {
                        target = leader_id.unwrap_or(target % 3 + 1);
                    }
                    Ok(CommandReply::Unavailable { .. }) | Err(_) => {
                        target = target % 3 + 1;
                    }
                    Ok(CommandReply::Applied { response, .. }) => {
                        bail!("concurrent write rejected: {:?}", response.outcome)
                    }
                }
                if Instant::now() >= deadline {
                    bail!("concurrent write {request_id} did not complete");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } else {
            history.invoke(
                operation_id,
                client_id,
                request_id,
                target,
                Operation::Get { key: key.clone() },
            );
            loop {
                match client.read_config(target, &key).await {
                    Ok(Ok(reply)) => {
                        history.complete(
                            operation_id,
                            Completion::Applied {
                                log_index: reply.applied_index.unwrap_or(0),
                                value: reply.value,
                            },
                        );
                        break;
                    }
                    Ok(Err(CommandReply::NotLeader { leader_id, .. })) => {
                        target = leader_id.unwrap_or(target % 3 + 1);
                    }
                    Ok(Err(_)) | Err(_) => target = target % 3 + 1,
                }
                if Instant::now() >= deadline {
                    bail!("concurrent read {request_id} did not complete");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(18)).await;
    }
    Ok(())
}

async fn submit(
    cluster: &ProcessCluster,
    mut node_id: u64,
    request: CommandEnvelope,
    operation: Operation,
    history: &HistoryRecorder,
) -> Result<(u64, u64)> {
    let request_body = serde_json::to_vec(&request).context("encoding replicated command")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let operation_id = Uuid::new_v4();
        history.invoke(
            operation_id,
            request.client_id,
            request.request_id,
            node_id,
            operation.clone(),
        );
        match cluster.command_bytes(node_id, &request_body).await {
            Ok(CommandReply::Applied {
                log_index,
                response,
            }) if response.outcome == CommandOutcome::Applied => {
                history.complete(
                    operation_id,
                    Completion::Applied {
                        log_index,
                        value: None,
                    },
                );
                return Ok((node_id, log_index));
            }
            Ok(CommandReply::Applied { response, .. }) => {
                let message = format!("replicated command rejected: {:?}", response.outcome);
                history.complete(
                    operation_id,
                    Completion::Rejected {
                        message: message.clone(),
                    },
                );
                bail!(message)
            }
            Ok(CommandReply::NotLeader { message, .. }) => {
                history.complete(
                    operation_id,
                    Completion::Rejected {
                        message: message.clone(),
                    },
                );
                // A redirect is only a hint and can lag an election. Verify a
                // node can commit a barrier before retrying the user command.
                node_id = cluster
                    .wait_for_writable_leader(None, Duration::from_secs(15))
                    .await?;
            }
            Ok(CommandReply::Unavailable { message }) => {
                history.complete(operation_id, Completion::Indeterminate { message });
                node_id = cluster
                    .wait_for_writable_leader(None, Duration::from_secs(5))
                    .await?;
            }
            Err(error) => {
                history.complete(
                    operation_id,
                    Completion::Indeterminate {
                        message: error.to_string(),
                    },
                );
                node_id = cluster
                    .wait_for_writable_leader(None, Duration::from_secs(5))
                    .await?;
            }
        }
        if Instant::now() >= deadline {
            bail!("command {} did not complete", request.request_id);
        }
    }
}

fn follower_other_than(
    cluster: &ProcessCluster,
    leader: u64,
    excluded: Option<u64>,
) -> Result<u64> {
    cluster
        .node_ids()
        .into_iter()
        .find(|node_id| *node_id != leader && Some(*node_id) != excluded)
        .ok_or_else(|| anyhow!("no eligible follower"))
}

fn barrier(client_id: Uuid, seed: u64, label: &[u8]) -> CommandEnvelope {
    CommandEnvelope::new(
        client_id,
        deterministic_uuid(b"barrier", seed, label),
        ReplicatedCommand::Barrier,
    )
}

fn deterministic_uuid(domain: &[u8], seed: u64, value: impl AsRef<[u8]>) -> Uuid {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_be_bytes());
    digest.update(domain);
    digest.update(seed.to_be_bytes());
    let value = value.as_ref();
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
    let bytes: [u8; 32] = digest.finalize().into();
    Uuid::from_bytes(bytes[..16].try_into().expect("SHA-256 prefix is 16 bytes"))
}
