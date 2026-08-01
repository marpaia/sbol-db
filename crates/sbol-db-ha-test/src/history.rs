use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Operation {
    Set { key: String, value: Value },
    Get { key: String },
    Delete { key: String },
    Barrier,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Completion {
    Applied {
        log_index: u64,
        value: Option<Value>,
    },
    Rejected {
        message: String,
    },
    Indeterminate {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HistoryEvent {
    Invoke {
        sequence: u64,
        monotonic_ns: u128,
        operation_id: Uuid,
        client_id: Uuid,
        request_id: Uuid,
        target_node: u64,
        operation: Operation,
    },
    Complete {
        sequence: u64,
        monotonic_ns: u128,
        operation_id: Uuid,
        completion: Completion,
    },
}

pub type History = Vec<HistoryEvent>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinearizabilityReport {
    pub valid: bool,
    pub keys_checked: usize,
    pub operations_checked: usize,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct HistoryRecorder {
    started: Instant,
    next_sequence: Arc<AtomicU64>,
    events: Arc<Mutex<History>>,
}

pub fn check_register_linearizable(history: &History, key_prefix: &str) -> LinearizabilityReport {
    match collect_completed(history, key_prefix) {
        Ok(by_key) => {
            let operations_checked = by_key.values().map(Vec::len).sum();
            for (key, operations) in &by_key {
                if operations.len() > 63 {
                    return invalid(format!(
                        "key {key} has {} operations; checker limit is 63",
                        operations.len()
                    ));
                }
                if !linearize_register(operations) {
                    return invalid(format!("no legal linearization for key {key}"));
                }
            }
            LinearizabilityReport {
                valid: true,
                keys_checked: by_key.len(),
                operations_checked,
                error: None,
            }
        }
        Err(error) => invalid(error),
    }
}

#[derive(Clone, Debug)]
struct CompletedOperation {
    invoke_ns: u128,
    complete_ns: u128,
    operation: Operation,
    value: Option<Value>,
}

fn collect_completed(
    history: &History,
    key_prefix: &str,
) -> Result<BTreeMap<String, Vec<CompletedOperation>>, String> {
    let mut invocations = HashMap::<Uuid, (u128, Operation)>::new();
    let mut by_key = BTreeMap::<String, Vec<CompletedOperation>>::new();
    for event in history {
        match event {
            HistoryEvent::Invoke {
                monotonic_ns,
                operation_id,
                operation,
                ..
            } => {
                invocations.insert(*operation_id, (*monotonic_ns, operation.clone()));
            }
            HistoryEvent::Complete {
                monotonic_ns,
                operation_id,
                completion: Completion::Applied { value, .. },
                ..
            } => {
                let Some((invoke_ns, operation)) = invocations.remove(operation_id) else {
                    return Err(format!("completion {operation_id} has no invocation"));
                };
                let key = match &operation {
                    Operation::Set { key, .. }
                    | Operation::Get { key }
                    | Operation::Delete { key } => key,
                    Operation::Barrier => continue,
                };
                if key.starts_with(key_prefix) {
                    by_key
                        .entry(key.clone())
                        .or_default()
                        .push(CompletedOperation {
                            invoke_ns,
                            complete_ns: *monotonic_ns,
                            operation,
                            value: value.clone(),
                        });
                }
            }
            HistoryEvent::Complete { .. } => {}
        }
    }
    Ok(by_key)
}

fn linearize_register(operations: &[CompletedOperation]) -> bool {
    let mut predecessors = vec![0_u64; operations.len()];
    for (later, operation) in operations.iter().enumerate() {
        for (earlier, candidate) in operations.iter().enumerate() {
            if candidate.complete_ns < operation.invoke_ns {
                predecessors[later] |= 1_u64 << earlier;
            }
        }
    }
    let complete = if operations.is_empty() {
        0
    } else {
        (1_u64 << operations.len()) - 1
    };
    let mut failed = HashSet::<(u64, Option<String>)>::new();
    linearize_from(operations, &predecessors, complete, 0, None, &mut failed)
}

fn linearize_from(
    operations: &[CompletedOperation],
    predecessors: &[u64],
    complete: u64,
    placed: u64,
    state: Option<Value>,
    failed: &mut HashSet<(u64, Option<String>)>,
) -> bool {
    if placed == complete {
        return true;
    }
    let state_key = state
        .as_ref()
        .map(|value| serde_json::to_string(value).expect("JSON values serialize"));
    if failed.contains(&(placed, state_key.clone())) {
        return false;
    }
    for index in 0..operations.len() {
        let bit = 1_u64 << index;
        if placed & bit != 0 || predecessors[index] & !placed != 0 {
            continue;
        }
        let operation = &operations[index];
        let next_state = match &operation.operation {
            Operation::Set { value, .. } => Some(value.clone()),
            Operation::Delete { .. } => None,
            Operation::Get { .. } if operation.value == state => state.clone(),
            Operation::Get { .. } => continue,
            Operation::Barrier => state.clone(),
        };
        if linearize_from(
            operations,
            predecessors,
            complete,
            placed | bit,
            next_state,
            failed,
        ) {
            return true;
        }
    }
    failed.insert((placed, state_key));
    false
}

fn invalid(error: String) -> LinearizabilityReport {
    LinearizabilityReport {
        valid: false,
        keys_checked: 0,
        operations_checked: 0,
        error: Some(error),
    }
}

impl Default for HistoryRecorder {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            next_sequence: Arc::new(AtomicU64::new(0)),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl HistoryRecorder {
    pub fn invoke(
        &self,
        operation_id: Uuid,
        client_id: Uuid,
        request_id: Uuid,
        target_node: u64,
        operation: Operation,
    ) {
        self.push(HistoryEvent::Invoke {
            sequence: self.sequence(),
            monotonic_ns: self.started.elapsed().as_nanos(),
            operation_id,
            client_id,
            request_id,
            target_node,
            operation,
        });
    }

    pub fn complete(&self, operation_id: Uuid, completion: Completion) {
        self.push(HistoryEvent::Complete {
            sequence: self.sequence(),
            monotonic_ns: self.started.elapsed().as_nanos(),
            operation_id,
            completion,
        });
    }

    pub fn snapshot(&self) -> History {
        self.events.lock().expect("history lock poisoned").clone()
    }

    fn sequence(&self) -> u64 {
        self.next_sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn push(&self, event: HistoryEvent) {
        self.events
            .lock()
            .expect("history lock poisoned")
            .push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_checker_accepts_legal_and_rejects_stale_reads() {
        let client = Uuid::from_u128(1);
        let request = Uuid::from_u128(2);
        let write = Uuid::from_u128(3);
        let read = Uuid::from_u128(4);
        let mut history = vec![
            HistoryEvent::Invoke {
                sequence: 0,
                monotonic_ns: 0,
                operation_id: write,
                client_id: client,
                request_id: request,
                target_node: 1,
                operation: Operation::Set {
                    key: "register/a".to_owned(),
                    value: Value::from(1),
                },
            },
            HistoryEvent::Complete {
                sequence: 1,
                monotonic_ns: 10,
                operation_id: write,
                completion: Completion::Applied {
                    log_index: 1,
                    value: None,
                },
            },
            HistoryEvent::Invoke {
                sequence: 2,
                monotonic_ns: 20,
                operation_id: read,
                client_id: client,
                request_id: Uuid::from_u128(5),
                target_node: 1,
                operation: Operation::Get {
                    key: "register/a".to_owned(),
                },
            },
            HistoryEvent::Complete {
                sequence: 3,
                monotonic_ns: 30,
                operation_id: read,
                completion: Completion::Applied {
                    log_index: 1,
                    value: Some(Value::from(1)),
                },
            },
        ];
        assert!(check_register_linearizable(&history, "register/").valid);
        if let HistoryEvent::Complete {
            completion: Completion::Applied { value, .. },
            ..
        } = &mut history[3]
        {
            *value = None;
        }
        assert!(!check_register_linearizable(&history, "register/").valid);
    }
}
