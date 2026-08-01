# RocksDB high availability

This document is the implementation contract for a two- or three-node
sbol-db cluster whose database state is stored in RocksDB. The design uses one
OpenRaft consensus group. Each voter owns a complete copy of the application
state and a separate durable Raft log.

The design follows the same broad boundaries used by mature systems while
staying deliberately smaller than a sharded database:

- [OpenRaft's storage-v2 contract](https://docs.rs/openraft/latest/openraft/storage/)
  separates durable log storage from the applied state machine and snapshots.
- [TiKV's Rust/Raft architecture](https://tikv.org/deep-dive/consensus-algorithm/raft/)
  applies quorum-committed log actions to RocksDB; its documented layout also
  uses [separate data and Raft-log engines](https://tikv.org/docs/7.1/deploy/configure/rocksdb/).
- [CockroachDB's replication layer](https://www.cockroachlabs.com/docs/stable/architecture/replication-layer)
  reinforces quorum writes, leader/leaseholder consistent reads, and explicit
  redirects from non-leaders.
- [etcd's membership procedure](https://etcd.io/docs/v3.5/op-guide/runtime-configuration/)
  adds replacements as learners and promotes them only after catch-up.

sbol-db needs one Raft group for the complete local database, not TiKV or
CockroachDB's many range-level groups, distributed transactions, or placement
driver. Adopting those layers would add complexity without improving the stated
two-to-three-node availability goal.

## Guarantee

A successful mutation response means:

1. the versioned logical command was durably appended on a voting majority;
2. the entry was committed by Raft; and
3. the leader durably applied it to its local RocksDB state machine.

Any subsequently elected leader must contain every acknowledged command. This
is the cluster's zero acknowledged-write-loss guarantee. It does not mean every
node has applied the command when the response is returned, and it does not
make non-RocksDB sidecars durable automatically.

The default production topology is three data-bearing voters in independent
failure domains. Two voters can provide the same majority durability while
both are healthy, but loss or isolation of either node removes the majority.
A two-node cluster therefore fails closed for writes; it cannot provide both
automatic single-node failover and split-brain safety.

## Node storage layout

Each node uses independent directories:

```text
node-root/
  raft-log.rocksdb/       vote, last-purged id, ordered log entries
  state.rocksdb/          SBOL state plus replicated state-machine metadata
  snapshots/              staged and verified snapshot generations
```

The Raft log and application state must not share one RocksDB instance. They
have different recovery and compaction lifecycles. The log store synchronizes
every vote, append, truncate, and purge before reporting success. The replicated
application state opens with `Durability::Sync`; its mutation and
`last_applied_log_id` must be committed in one RocksDB `WriteBatch`.

Stable `node_id` values belong to data directories, not network addresses.
Reusing a node id with a different data directory is forbidden.

## Replicated protocol

Clients send mutations to any node. Followers return or proxy an explicit
leader redirect. The leader validates and normalizes the request, chooses all
nondeterministic inputs such as UUIDs and timestamps, and proposes a
`CommandEnvelope` containing:

- a command protocol version;
- a stable client id;
- a request id used for idempotency; and
- a deterministic logical command;
- a canonical payload hash; and
- a logical request hash that excludes leader-selected fields.

The state machine stores recent `(client_id, request_id)` results as replicated
state. A retry after a timeout or leader change therefore returns the prior
result instead of applying the command twice. Raw RocksDB batches are not the
wire protocol: they would freeze physical indexes into the cluster protocol and
make rolling schema upgrades unsafe.

The two hashes serve different purposes. The canonical payload hash binds the
exact normalized entry and is independent of JSON object insertion order. The
request hash captures client-visible semantics, so two leaders may choose
different tentative timestamps for the same retry while the state machine still
recognizes it and returns the first durable result. Reusing a request id for a
different logical mutation is rejected.

All voters must advertise support for a command version before the leader may
emit it. Schema migration is itself a versioned replicated command or a startup
compatibility gate; a mixed cluster must never interpret the same log entry in
two ways.

## Reads and failover

Leader reads use OpenRaft's linearizable-read barrier before accessing the
local state machine. Followers do not serve ordinary API reads initially.
Bounded-stale follower reads can be added later as an opt-in API with the
applied index and staleness visible to the caller.

Only a majority partition can elect a leader. The minority remains read-only
or unavailable and never accepts writes. Client retry safety comes from the
request id, not from guessing whether the old leader committed a timed-out
request.

## Snapshots and node replacement

A snapshot is built from a RocksDB checkpoint and a manifest containing at
least the cluster id, state-machine schema version, command protocol version,
last applied log id, membership, file sizes, and checksums. The receiver writes
to a staging directory, verifies the complete manifest and a logical state
digest, closes the current state database, atomically activates the generation,
and only then reports installation success.

Snapshot installation must never import into a live RocksDB handle. A new node
joins as a learner, installs and verifies a snapshot, catches up through the
log, and is promoted to a voter only after it is current.

Activation uses a synchronized recovery journal around the two directory
renames. On restart, a node either restores the closed previous generation or
opens and verifies the already-activated generation before deleting its
backup. There is no restart state in which a missing canonical state directory
is silently treated as a fresh database.

## Application integration boundary

The consensus crate must own the application RocksDB handle. The ordinary
RocksDB backend currently clones `Db` into every repository; those clones would
keep the old generation open across a snapshot install. HA-backed trait
adapters must instead acquire the current generation through the state-machine
handle for each operation. Enabling an HA CLI flag before that refactor would
create an apparently clustered server with unsafe snapshot replacement.

Every mutation also has to enter the replicated command protocol. The current
inventory is:

| State | Mutation families | Required treatment |
| --- | --- | --- |
| Source of truth | document import/replace/merge, graph delete/clear/write, SPARQL Update, ontology load | leader derives a normalized domain plan containing every UUID and timestamp; followers stage that plan plus `last_applied` in one batch |
| Identity | users and password/reset changes; API tokens | implemented as explicit replicated transitions; only password hashes and token hashes enter the log |
| Work scheduling | enqueue, dequeue/lease, renewal, completion/failure, reap, cancel, logs | run workers only on the leader initially; replicate leader-selected times, ids, counters, and transition results |
| Rebuildable indexes | PageRank, sequence clusters, sketches, in-memory/vector search | either replicate their compact deterministic outputs or mark/rebuild them before the promoted node advertises readiness |
| Instance configuration | config set/delete | replicate the leader-selected timestamp |
| External bytes | content-addressed attachment blob store | place behind independently HA object storage or add a separate replicated-blob protocol; it is not covered by RocksDB Raft |

Repository staging helpers are already the right shape for much of the SBOL
write path, but they currently choose some clocks, UUIDs, and counters locally.
Those choices move to the leader-facing planner. The wire protocol remains a
versioned logical/domain plan, not a serialized RocksDB `WriteBatch`, so a
rolling upgrade can change physical indexes without changing command meaning.

Public request idempotency must carry an API request id through to
`CommandEnvelope`; adapters that mint a fresh internal request id are safe only
for operations already idempotent by value. Creation, enqueue, and other
non-idempotent routes may not be exposed until that request context is wired.

## Networking and operations

The implemented transport uses an internal Axum router and OpenRaft HTTP RPC.
Requests are rejected by bearer authentication before JSON body buffering or
deserialization, and the server retains only the token digest. This authenticates
peers but plain HTTP does not encrypt the token or snapshot bytes. Production
deployments must use mTLS/TLS (directly or through a private service mesh),
restrict the listener at the network layer, and rotate the shared credential.
The router sets an explicit 64 MiB encoded-body limit because OpenRaft's default
3 MiB raw snapshot chunk expands substantially in JSON and exceeds Axum's 2 MiB
default. Streaming/binary snapshot transport should replace JSON before large
production datasets are certified.

Bootstrap is an explicit one-time `initialize` with the full initial voter map.
Subsequent nodes join as learners and use OpenRaft membership changes; a second
bootstrap is never an availability fallback. Node identity and cluster identity
are persisted before the node participates. A load balancer may send clients to
any node only after the API has a leader redirect/proxy contract; followers may
not accept local mutations.

## Implementation sequence

1. **Durable primitives (implemented).** Explicit synchronous RocksDB writes,
   physical checkpoints, a separate OpenRaft crate, a versioned command
   envelope, durable vote/log storage, and disk-bound node/cluster identities.
2. **State-machine seam (in progress).** Idempotency results, membership,
   `last_applied_log_id`, configuration, user/account transitions, and hashed
   API-token state commit in one synchronous application batch. The remaining
   mutation inventory above still needs deterministic command variants and
   HA-backed trait adapters.
3. **Single-node Raft mode (not exposed yet).** Route all writes through `client_write`, enforce
   leader-only linearizable reads, and pass the existing RocksDB conformance
   suite without changing public semantics.
4. **Networking and membership (transport implemented).** Authenticated HTTP
   Raft RPC and the reusable node/storage bootstrap exist and have run as a
   three-node cluster. Production still needs TLS, explicit bootstrap/join
   commands, leader discovery, joint-consensus membership changes, health
   status, and metrics for term/role/commit/applied indexes.
5. **Snapshots and replacement (storage implemented).** Checkpoint manifests,
   file checksums, cluster/protocol binding, corruption rejection, staged
   install, and journaled activation exist. Learner catch-up and operational
   promotion/removal workflows remain.
6. **Failure validation (started).** OpenRaft's complete storage suite, real
   three-node HTTP failover, two-voter fail-closed behavior, snapshot
   corruption, activation-gap recovery, and full-cluster durable restart pass.
   Continue with process-kill,
   restart, disk-fault, delay, partition, leader-churn, and snapshot-corruption
   tests. Every acknowledged request id must exist exactly once after recovery,
   and all nodes at one applied index must produce the same logical digest.

The concrete test layers, corpus workload, current evidence, and remaining
production claim gates are specified in [RocksDB HA testing](ha-testing.md).

The HA claim covers only state included in this protocol and snapshot contract.
Before production, any file-backed upload, search index, or other sidecar must
be classified as replicated source-of-truth state or explicitly rebuildable
derived state. A RocksDB-only consensus path cannot by itself guarantee those
external bytes.
