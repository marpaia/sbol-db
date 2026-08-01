# RocksDB HA testing

The HA test contract is not "the cluster elected another leader." It is:

> Every write that a client observed as successful remains present exactly
> once after any failure within the advertised fault model, while a minority
> cannot acknowledge a write and successful reads remain linearizable.

That contract needs several complementary test systems. An in-process Raft
simulation gives fast, replayable coverage of long fault schedules. It cannot
prove operating-system process isolation, RocksDB durability after abrupt
power loss, filesystem behavior, or the public API's concurrent history. Those
claims require separate subprocess, disk-fault, and external-history gates.

## The first corpus-backed simulator

`sbol-db-ha-sim` starts three real OpenRaft nodes. Every node uses the production
`RocksLogStore` and `RocksStateMachine` against its own temporary RocksDB
directories. Only the RPC network is replaced by an in-process fault fabric.
The fabric can partition directed links, isolate a node, add latency, remove a
node, and reconnect a restarted node to its durable directory.

The workload reads the existing
`sbol-test-suite-integration-v1.json` manifest and refuses to run unless the
external SBOLTestSuite checkout is:

- a Git checkout at the exact pinned commit;
- clean with respect to tracked and staged files;
- complete across every declared import group; and
- still producing the exact expected import/parse-failure counts.

Each recognized source document goes through the normal pure SBOL import
derivation before entering the workload. At the pinned revision the simulator
accepts 447 documents: 27 SBOL1, 179 SBOL2, 11 SBOL2 best-practice, 71 SBOL2
incomplete-compliant, 155 SBOL3, and 4 RDF documents. The five expected SBOL3
parse failures remain visible in the inventory rather than silently
disappearing.

For the current protocol milestone, every accepted document's raw body,
format, content hash, object count, and triple count is stored through a
replicated `SetConfig` command under a test-only key. This is deliberately a
transport/durability workload: it exercises real corpus sizes, Raft log writes,
sync application batches, idempotency, snapshots, catch-up, and restart. It
does **not** yet prove that graph registries, triples, typed projections, jobs,
or derived indexes replicate correctly. Once `ImportDocument` is a normalized
replicated command, the simulator must replace this payload adapter with that
production command and compare the full semantic store on every node. The
current result must not be described as application-level SBOL HA.

Run the principal local gate with:

```sh
make ha/chaos-sbol-test-suite \
  SBOL_TEST_SUITE_ROOT=/absolute/path/to/SBOLTestSuite
```

Reproduce a particular fault schedule and retain its JSON trace with:

```sh
make ha/chaos-sbol-test-suite \
  SBOL_TEST_SUITE_ROOT=/absolute/path/to/SBOLTestSuite \
  HA_CHAOS_SEED=0x5b01db0000000001 \
  HA_CHAOS_TRACE=target/ha-chaos-0x5b01db0000000001.json
```

The seed fixes corpus ordering, request IDs, and fault choices. The current
Tokio/OpenRaft test runtime is not a fully deterministic scheduler: thread
interleavings, election timing, and therefore leader IDs can still differ.
The trace is a replayable fault plan and evidence record, not a promise of
FoundationDB-style instruction-for-instruction replay.

An always-on 60-document synthetic test exercises the same scenario without
requiring the external checkout:

```sh
cargo test -p sbol-db-ha-sim
```

## Single-host real-process systems lab

`sbol-db-ha-test` crosses the process boundary that the simulator deliberately
does not model. It launches three standalone `sbol-db-ha-node` voters, each
with an independent RocksDB directory and authenticated peer and test-client
listeners. A controller-owned TCP fabric creates six directed source-to-target
links, so a partition closes existing keep-alive connections as well as
rejecting new traffic without changing canonical Raft membership.

The synthetic gate runs on every workspace test invocation:

```sh
make ha/test-process
```

It exercises concurrent linearizable reads and writes, leader `SIGKILL`, a
leader crash after application but before its client response, a 1-versus-2
partition, a lagging follower, snapshot catch-up, exact idempotent retries, and
a full three-process restart from the original RocksDB directories. After a
final barrier, every acknowledged key is read through the elected leader and
all nodes must produce the same canonical per-column-family state audit.

The principal real-corpus gate uses the same pinned manifest and loader as the
in-process simulator:

```sh
make ha/process-sbol-test-suite \
  SBOL_TEST_SUITE_ROOT=/absolute/path/to/SBOLTestSuite
```

Every local run retains `manifest.json`, `history.jsonl`, `checker.json`, the
final report, node stdout/stderr, and each RocksDB directory below a new
`target/ha-runs/process-*` directory. A timed-out request is recorded as
indeterminate and retried with its original `(client_id, request_id)`. CI runs
the same workload but does not export corpus-bearing histories or data
directories as artifacts.

This process lab still shares one kernel, clock, physical disk, and power
domain. It validates real process death and real TCP behavior, but it is not
evidence for physical host or availability-zone independence. Its histories,
workload protocol, state audits, and artifact schema are environment-neutral
so a future VM driver can reuse them.

## Fault schedule and oracle

One full-corpus run performs these phases while writes continue:

1. Healthy three-voter replication.
2. Isolation of one follower long enough to cross snapshot and log-purge
   thresholds, followed by healing and catch-up.
3. Exact retries of previously acknowledged request IDs, proving that the
   stored result is returned rather than applying the mutation twice.
4. Clean leader shutdown after an acknowledged write, replacement election,
   and restart of the old leader from disk.
5. Delayed quorum traffic causing an ambiguous client timeout, followed by an
   exact retry of the same request ID.
6. A 1-versus-2 partition. The isolated former leader must not acknowledge;
   the majority must elect a leader and acknowledge a barrier.
7. A follower kept offline while new corpus writes commit, then restarted and
   caught up.
8. Healing, a linearizable barrier, snapshot creation, complete shutdown of all
   three nodes, reconstruction from the same RocksDB directories, and another
   barrier.
9. Exact comparison of every acknowledged key/value on all three nodes plus a
   common canonical SHA-256 state digest.

The oracle records a mutation only after the client receives success. A timed
out request is **indeterminate**, not failed: it may have committed. Retrying
the identical `(client_id, request_id, logical request)` is the only safe way
to resolve that ambiguity. Tests must never assert that an unacknowledged
request is absent merely because its response was lost.

The fixed full-corpus run on 2026-08-01 produced:

| Field | Result |
| --- | --- |
| SBOLTestSuite commit | `0044284331b2f915a6e4b9d50e1cbf3ea2f62dcd` |
| Corpus fingerprint | `ad3edcfc390407a4cb79548f61897f636cad916cc92739d185202d53836a487f` |
| Acknowledged corpus writes | 447 / 447 |
| Exact acknowledged retries | 12 |
| Ambiguous timeout resolved by exact retry | 1 |
| Recovered nodes matching the oracle | 3 / 3 |
| Final logical state SHA-256 | `95ce2972d6e534c8f7ddb67843b0b98eaf3903ae718b0444c366115adfd8af8a` |

This is evidence for the implemented in-process fault model, not a production
durability certification.

## Required test layers

| Layer | Runs | Primary invariant | Status |
| --- | --- | --- | --- |
| Storage and state-machine tests | Every PR | OpenRaft storage contract, sync batches, snapshot integrity and activation recovery | Implemented |
| Synthetic in-process chaos | Every PR, fixed seed | No acknowledged loss, minority fail-closed, idempotent retry, convergence after restart | Implemented |
| Full SBOLTestSuite in-process chaos | Nightly and before release, seed matrix | Same oracle over all 447 real importable documents and realistic payload sizes | Implemented for one replayable schedule |
| Real HTTP three-node tests | Every PR | Peer authentication, body limits, serialization, routing, and failover | Initial fixed scenario implemented |
| Multi-process crash harness | Every PR and nightly | `SIGKILL`, after-apply response loss, automatic process restart, client retry behavior | Implemented |
| Filesystem fault harness | Nightly on Linux | `ENOSPC`, `EIO`, WAL/snapshot corruption, lost/torn writes, fsync semantics | Required |
| External concurrent-history test | Every PR, nightly, and release | Linearizable reads/writes and no lost acknowledged operations under process and partition faults | Implemented for replicated config registers |
| Rolling-version test | Release | Protocol compatibility, snapshot compatibility, membership changes, safe downgrade refusal | Required |
| Application semantic chaos | Release once import commands exist | Graphs, triples, objects, auth, jobs, and declared derived state match after recovery | Required |

### Process crash points

The subprocess harness should expose failpoints and pause immediately:

- before and after the local Raft WAL sync;
- after follower persistence but before its RPC response;
- after quorum replication but before the leader responds to the client;
- before and after state-machine batch sync;
- during snapshot construction, transfer, verification, activation journal
  creation, directory swap, and cleanup; and
- during learner promotion, voter removal, and rolling restart.

The controller then sends `SIGKILL`, restarts from the same directory, heals
the cluster, and evaluates the acknowledged-write oracle. A graceful
`Raft::shutdown` test does not cover these crash windows.

### Disk faults

Linux CI should add a small privileged test job using a faulting block device
or filesystem shim. At minimum it must inject disk-full and I/O errors into the
Raft log, application state, snapshot staging directory, and directory-sync
operations. Corruption tests must distinguish three safe outcomes: reject the
node and require replacement, install a verified snapshot from the leader, or
recover an activation journal. Silent acceptance or startup with partial state
is always a failure.

### External history and linearizability

The current oracle is deliberately single-client and state-oriented. A
production claim needs several concurrent clients making writes and
linearizable reads through arbitrary nodes while a separate controller injects
faults. Record invocation and completion separately, preserve timed-out
operations as unknown, and validate the history against a simple reference
model. This is the same essential shape used by etcd robustness testing: run
traffic and failpoints, retain requests/responses and server data directories,
then validate the history and generate a failure report.

Start with a register/map model over configuration keys and a set model over
idempotency IDs. Add multi-object SBOL import/delete semantics only when the
public transaction boundary is defined. Linearizability is a real-time
property, so merely comparing final state is necessary but not sufficient.

## CI cadence and evidence

- **Per PR:** storage suites, RocksDB conformance, HTTP cluster test, one fixed
  in-process seed, and the real-process 60-document systems gate.
- **Nightly:** the full 447-document corpus through the external-process
  controller and an expanding seed/failpoint matrix.
- **Weekly:** longer leader churn, rolling upgrade/downgrade, disk-full and I/O
  fault jobs, snapshot corruption, and hours-long mixed read/write soak.
- **Release:** replay every retained regression seed, run the full semantic
  corpus after the replicated import path lands, and archive traces, process
  logs, node data-directory manifests, corpus commit/fingerprint, build commit,
  and checker output.

Any discovered bug becomes a permanent named regression with its minimal
trace, fault point, and seed. The harness is not considered improved if it can
no longer reproduce bugs it previously found.

## Database-testing perspective

This layering follows mature distributed-database practice:

- [FoundationDB simulation](https://apple.github.io/foundationdb/testing.html)
  emphasizes deterministic, seed-replayable whole-cluster simulation, but
  explicitly combines it with live performance and hardware failure testing.
  That combination is the model here; an in-process simulator is not allowed
  to stand in for disks or processes.
- [FoundationDB client testing](https://apple.github.io/foundationdb/client-testing.html)
  uses the same workload shape in simulation and on real clusters and keeps a
  local model updated by successful writes. The sbol-db oracle follows that
  successful-write discipline.
- [TiKV's testing guide](https://tikv.org/deep-dive/testing/introduction/)
  calls out node replacement, lost messages, partitions, overloaded links,
  disappearing dependencies, and scheduling bugs as distinct failure classes.
- [etcd robustness testing](https://github.com/etcd-io/etcd/tree/main/tests/robustness)
  combines traffic, crash/network/disk failpoints, operation histories, model
  validation, retained data directories, and reproducible bug scenarios.
- [Jepsen's linearizability model](https://jepsen.io/consistency/models/linearizable)
  makes the key availability consequence explicit: during a partition a
  linearizable system cannot let every side continue accepting operations.
  The simulator's minority-no-ack assertion is therefore a safety requirement,
  not an availability defect.

The practical conclusion is that "no acknowledged data loss" is not one test
result. It is an evidence chain spanning deterministic plans, real concurrent
histories, abrupt process death, storage faults, snapshot recovery, and the
complete application mutation inventory.
