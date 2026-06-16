# Deploying sbol-db

This is the operator's reference: architecture, install, configuration,
day-2 operations, capacity planning, and troubleshooting. It does
**not** cover the request shape of individual REST endpoints — see
`/docs` (Scalar UI) or
[openapi.json](../crates/sbol-db-server/src/openapi.json) for that,
and the [crate guide](crate-guide.md) for codebase architecture (Rust
modules, traits, repositories).

## Audience

You're standing up sbol-db in a real environment (k8s cluster, managed
Postgres, observability stack). If you're just trying the CLI locally,
the [README quickstart](../README.md#installation) is faster.

## Architecture

The conceptual model. Read this section once to understand what
you're deploying; reach for the later sections to do the actual work.

### Topology

`sbol-db server` is a single stateless binary that talks to one
Postgres instance. Each pod runs both an HTTP listener **and** an
embedded async-job worker by default — the worker subscribes to every
registered queue and shares the database (but not the connection
pool) with the HTTP routes. Everything — the typed objects, the RDF
triplestore, the typed projections, the k-mer index, ontology
closures, the job queue itself — lives in Postgres. Multiple pods can
share a database safely; work is distributed across the cluster via
`FOR UPDATE SKIP LOCKED` against `sbol_jobs`, with no leader election
or external broker. Migrations are idempotent and run before the
serve pods roll.

The HTTP surface exposes the SBOL query API, the async-job operator
surface (`POST /jobs`, etc.), and three operational endpoints
(`/healthz`, `/readyz`, `/metrics`) on a single port.

### Async job runtime

`sbol-db` ships a Postgres-backed async job runtime for work that
shouldn't block an HTTP request: corpus-scale imports, ontology
fetches, future projection workers, index rebuilds. The job system is
intentionally narrow — infrastructure for sbol-db's own async work,
not a general-purpose workflow engine.

#### Deployment shapes

| Shape | When | How |
|---|---|---|
| **Single-node** | Dev / small prod | `sbol-db server` on one pod. HTTP and worker share a process. |
| **Two-node HA** | Most production | `sbol-db server` on two pods behind an L7 load balancer, both pointed at the same Postgres. SKIP LOCKED splits work; a dead pod's leases expire and the other picks up. |
| **Dedicated worker fleet** | When async capacity has to scale independently of API throughput | `sbol-db server --no-worker` on API nodes; a separate `sbol-db worker` Deployment for workers. |

There is no leader election, no external broker, and no in-process
state shared between pods.

#### Why two connection pools

A long-running job handler holds its Postgres transaction for the
handler's full duration. If that ran on the HTTP pool, a handful of
slow imports would starve inbound API requests of connections. So
each pod opens **two** pools:

- The **API pool** sized for request throughput (default 8, env
  `DATABASE_MAX_CONNECTIONS`).
- The **worker pool** sized to `worker_concurrency + 4` (override
  via `SBOL_DB_WORKER_POOL_MAX`).

When `--no-worker` is set, the worker pool isn't opened at all.

#### At-least-once delivery

The runtime guarantees at-least-once execution of every enqueued
job. Handlers must therefore be idempotent or use `idempotency_key`;
`sbol_graphs.content_hash` gives natural idempotency for imports.

- **Lease.** A worker takes a lease (default 60s) on dequeue and
  renews it while the handler runs. The lease in Postgres is what
  makes work safely partitionable across nodes — see
  [Process lifecycle](#process-lifecycle) for what happens when
  leases lapse.
- **Exponential backoff on failure.** Each failed attempt re-queues
  the row with `available_at = now() + min(60s × 2^attempts, 1h)`.
  After `max_attempts` (default 5) the row lands in `dead`.
- **LISTEN/NOTIFY for low-latency wake.** Workers `LISTEN
  sbol_jobs_enqueued`; a trigger fires `NOTIFY` on insert. The
  poll-interval fallback (default 5s) covers transient listener
  disconnects.

#### Registered job kinds

Built-in handlers ship in `sbol-db-jobs::handlers`. Today:

| `kind` | Purpose |
|---|---|
| `import_document` | Async equivalent of `POST /graphs`. Payload is the inline import body, format (`turtle`, `jsonld`, `rdfxml`, `ntriples`, `genbank`, or `fasta`), optional namespace, and metadata; `result` is the `ImportReport`. |
| `import_remote_document` | Worker-side public HTTPS fetch followed by the same import pipeline. Payload is `{url, format, namespace?, document_iri?, name?, description?, created_by?}`; the worker rejects non-HTTPS, local, and private-address URLs before fetching. |

Future handlers (projection worker, ontology fetch, index rebuild)
will land here as new modules without schema changes.

#### Operator surfaces

REST (mirrors the CLI):

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/jobs` | Enqueue. Body: `{kind, payload, queue?, priority?, max_attempts?, idempotency_key?, correlation_id?}`. |
| `GET` | `/jobs` | List. Filters: `kind`, `status`, `queue`, `correlation_id`, `limit` (max 1000). |
| `GET` | `/jobs/{id}` | One job. |
| `POST` | `/jobs/{id}/cancel` | Cancel a queued or running job. |

CLI:

```sh
sbol-db jobs enqueue import_document @payload.json --idempotency-key=doc:42
sbol-db jobs enqueue import_remote_document @remote-payload.json
sbol-db jobs status <uuid>
sbol-db jobs list --kind import_document --status failed --limit 50
sbol-db jobs cancel <uuid>

# Dedicated worker fleet (alternative to `server --no-worker` + standalone API):
sbol-db worker --concurrency 8 --queues default
```

The `/jobs` routes inherit the same `SBOL_DB_MAX_BODY_BYTES` and
`SBOL_DB_REQUEST_TIMEOUT_SECS` limits as the rest of the HTTP surface;
keep that in mind when enqueueing large `import_document` bodies (split
into multiple jobs, or use synchronous `POST /graphs` with a higher
body cap if you really need a single 200 MB import). Remote imports keep
large source bodies out of the enqueue request, but the fetched response
still runs inside the worker's request and shutdown budgets.

### Process lifecycle

What happens when a pod boots, terminates gracefully, or crashes.

#### Startup

1. Resolve config; build the job handler registry.
2. Connect the API pool, run migrations (idempotent), ping the
   database.
3. Open the worker pool (skipped under `--no-worker`).
4. Spawn the worker, lease reaper, and `LISTEN sbol_jobs_enqueued`
   task.
5. Bind the HTTP listener last; `/readyz` returns 200 only once every
   step above is up.

Startup honors `DATABASE_STARTUP_TIMEOUT_SECS` (default 30s) on every
connect path, so the boot retries Postgres with capped exponential
backoff before declaring failure.

#### Graceful shutdown

SIGTERM (and SIGINT) drive a unified shutdown of HTTP and worker
concurrently:

1. The HTTP listener stops accepting new connections; in-flight
   requests run to completion (bounded by
   `SBOL_DB_REQUEST_TIMEOUT_SECS`).
2. The worker stops dequeuing immediately; in-flight handlers get the
   configured grace window (default 30s) to finish.
3. Past the grace deadline, the worker exits and any still-running
   handlers are abandoned. Their leases expire shortly after and
   another node (or the same node on restart) picks them up — the
   at-least-once contract covers the rest.
4. Once both halves drain, the process exits 0.

Kubernetes pod termination sequence:

1. Pod marked Terminating; removed from Service endpoints; SIGTERM
   sent to PID 1.
2. sbol-db server enters drain mode.
3. After `terminationGracePeriodSeconds` (default 65s in the chart),
   kubelet sends SIGKILL.

Match `terminationGracePeriodSeconds` ≥ `SBOL_DB_REQUEST_TIMEOUT_SECS`
+ worker grace + a few seconds of headroom, otherwise SIGKILL will
arrive mid-request or mid-handler.

#### Crash semantics

When a worker dies hard (OOM, node failure, SIGKILL), no graceful
drain runs. The safety net is the lease itself:

- The dead worker's `sbol_jobs.lease_expires_at` is fixed in the
  past.
- Every running worker periodically reaps expired leases —
  `UPDATE sbol_jobs SET status='queued', leased_by=NULL WHERE
  status='running' AND lease_expires_at < now()`.
- Another worker dequeues the row on its next tick.

Hard-kill and graceful-shutdown produce the same observable end state
in Postgres. The cost difference is latency: a hard kill leaks at
most one lease duration of stalled work per affected job.

## Container image

### Where it's published

| | |
|---|---|
| Registry | `ghcr.io/marpaia/sbol-db` |
| Source | [`Dockerfile`](../Dockerfile) at the repo root |
| Base | `gcr.io/distroless/static-debian12:nonroot` |
| Build | `make container` (locally) or [`.github/workflows/container.yml`](../.github/workflows/container.yml) (CI) |

### Tag scheme

Mirrors the `Makefile`:

- If the commit is on an annotated tag → the tag (e.g. `v0.2.0`).
- Otherwise → the short Git SHA, with `-dirty` appended when the working
  tree has uncommitted changes (only seen on local builds; CI checkouts
  are clean).

The CI workflow (`container.yml`) runs on `v*` tag pushes and on manual
dispatch. It does **not** run on every push to `master`; uncomment the
`branches: [master]` line in the workflow when you want that.

### Image properties

- Statically linked `sbol-db` binary at `/usr/local/bin/sbol-db`, built
  with musl libc. Zero shared-library surface.
- Runs as `nonroot:nonroot` (UID/GID 65532). Compatible with strict
  pod security: `runAsNonRoot: true`, `readOnlyRootFilesystem: true`,
  `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`.
- `ENTRYPOINT ["/usr/local/bin/sbol-db"]`, default `CMD ["server",
  "--bind", "0.0.0.0:8080"]`. Subcommands are passed via Kubernetes
  `args:`.
- Exposes port `8080`.

### Building locally

```sh
make container          # builds ghcr.io/marpaia/sbol-db:<version>, --load to local docker
docker run --rm -it ghcr.io/marpaia/sbol-db:<version> --help
```

## CI workflows

| File | Trigger | Purpose |
|---|---|---|
| [`ci.yml`](../.github/workflows/ci.yml) | push, PR | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `helm lint` + `helm template` against three value profiles |
| [`container.yml`](../.github/workflows/container.yml) | `v*` tag push, `workflow_dispatch` | Build and push the container image to GHCR |

There is no chart-publish workflow yet; the chart is installed from the
working tree (`helm install … ./charts/sbol-db`).

## Helm chart

Located at [`charts/sbol-db/`](../charts/sbol-db) in this repo. See its
own [README](../charts/sbol-db/README.md) for the chart-specific tour;
the rest of this section captures the operational story.

### Quickstart

```sh
helm dependency update charts/sbol-db
helm install sbol-db ./charts/sbol-db \
  --set postgresql.enabled=true \
  --set postgresql.auth.password=changeme
```

This brings up the bundled bitnami/postgresql subchart, runs the
migration Job, and starts sbol-db. Connect through a port-forward:

```sh
kubectl port-forward svc/sbol-db 8080:80
open http://127.0.0.1:8080/docs    # Scalar API explorer
```

### Production install

Don't use the bitnami subchart in production — operate Postgres
separately (RDS, Cloud SQL, etc.) and feed sbol-db a Secret:

```sh
kubectl create secret generic sbol-db-database \
  --from-literal=url='postgres://user:pass@db.example.com:5432/sbol?sslmode=require'

helm install sbol-db ./charts/sbol-db \
  --set externalDatabase.existingSecret.name=sbol-db-database \
  --set externalDatabase.existingSecret.key=url \
  --set ingress.enabled=true \
  --set ingress.hosts[0].host=sbol-db.example.com \
  --set serviceMonitor.enabled=true \
  --set podDisruptionBudget.enabled=true
```

### DSN resolution (three modes)

Exactly one of these must be set; the chart fails to render otherwise.

1. **`externalDatabase.existingSecret.{name,key}`** — the Deployment and
   migration Job both read `DATABASE_URL` directly from the named
   Secret. Nothing chart-managed touches the DSN. Preferred for
   production.
2. **`postgresql.enabled=true`** + `postgresql.auth.password` — the
   chart provisions the bitnami subchart and renders a chart-managed
   Secret carrying the DSN. Quickstart/dev only; the password lands in
   the rendered manifest and rotation needs a separate workflow.
3. **`externalDatabase.url`** — literal DSN string, materialized into a
   chart-managed Secret. Useful when migrating from another runner;
   prefer mode 1 once the Secret can live independently.

### Migration semantics

`sbol-db db migrate` runs as a Helm hook:

| | |
|---|---|
| Hooks | `pre-install,pre-upgrade` |
| Weight | `-5` (runs before the Deployment rolls) |
| Delete policy | `before-hook-creation,hook-succeeded` |
| `restartPolicy` | `OnFailure` |
| `backoffLimit` | `migrate.backoffLimit` (default 3) |
| `ttlSecondsAfterFinished` | `migrate.ttlSecondsAfterFinished` (default 600) |

Migrations are append-only (the `sqlx_migrations` table records each
applied version) and idempotent, so the hook is safe to re-run on
every upgrade. On failure the Job records its error and the install or
upgrade halts before the Deployment changes.

The Job inherits `DATABASE_STARTUP_TIMEOUT_SECS` (default 30s), so the
hook can survive a slow Postgres cold-start — the migration container
retries `connect()` with exponential backoff before declaring failure.

## Configuration reference

This is the operator's single source of truth for tunable knobs. The
Helm chart materializes each value from `values.yaml`; the binary
falls back to the env var directly when running outside the chart. The
chart's own [values.yaml](../charts/sbol-db/values.yaml) carries
inline comments for every key.

### Application config (env-driven)

#### Connection

| Chart value | Env | Default | Purpose |
|---|---|---|---|
| `externalDatabase.*` or `postgresql.*` | `DATABASE_URL` | — | Postgres DSN. Resolved via one of the three DSN modes above. |
| `config.bind` | `SBOL_DB_BIND` | `0.0.0.0:8080` | Listen address. The Service and probes assume port 8080. |

#### Server

| Chart value | Env | Default | Purpose |
|---|---|---|---|
| `config.server.requestTimeoutSecs` | `SBOL_DB_REQUEST_TIMEOUT_SECS` | `60` | Outer wall-clock timeout per HTTP request; returns 408. |
| `config.server.maxBodyBytes` | `SBOL_DB_MAX_BODY_BYTES` | `33554432` (32 MiB) | Max request body; oversize returns 413. |

#### Database pool

These knobs apply to the API pool. They also apply to the worker pool
*except* `max_connections`, which the worker pool sizes from
`worker_concurrency` (override with `SBOL_DB_WORKER_POOL_MAX`).

| Chart value | Env | Default | Purpose |
|---|---|---|---|
| `config.database.maxConnections` | `DATABASE_MAX_CONNECTIONS` | `8` | API pool ceiling. |
| `config.database.minConnections` | `DATABASE_MIN_CONNECTIONS` | `0` | Idle floor (warm pool). |
| `config.database.acquireTimeoutSecs` | `DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | Per-request wait when the pool is saturated. |
| `config.database.idleTimeoutSecs` | `DATABASE_IDLE_TIMEOUT_SECS` | `300` | Idle eviction. `0` disables. |
| `config.database.maxLifetimeSecs` | `DATABASE_MAX_LIFETIME_SECS` | `1800` | Hard connection lifetime. `0` disables. Useful behind PgBouncer. |
| `config.database.connectTimeoutSecs` | `DATABASE_CONNECT_TIMEOUT_SECS` | `5` | Per-attempt connect cap. |
| `config.database.startupTimeoutSecs` | `DATABASE_STARTUP_TIMEOUT_SECS` | `30` (chart-set on serve / migrate) or `0` (other CLI commands) | Total budget for the boot-time retry loop. |

#### Async-job worker

These knobs apply to the embedded worker spawned by `sbol-db server`
and to the standalone `sbol-db worker` process. All take effect at
process start; there is no live-reload.

| Chart value | Env / CLI flag | Default | Purpose |
|---|---|---|---|
| `config.worker.disabled` | `SBOL_DB_WORKER_DISABLED` / `--no-worker` | `false` | Skip the embedded worker entirely. Set on API-only pods when running a dedicated worker fleet. |
| `config.worker.concurrency` | `SBOL_DB_WORKER_CONCURRENCY` / `--worker-concurrency` | `num_cpus()` | Max in-flight handler tasks per worker. Also sizes the worker connection pool floor (`concurrency + 4`). |
| `config.worker.queues` | `SBOL_DB_WORKER_QUEUES` / `--worker-queues` | `default` | Comma-separated queue allowlist. A worker only dequeues rows whose `queue` matches. |
| `config.worker.id` | `SBOL_DB_WORKER_ID` / `--worker-id` | `<hostname>-<pid>-<rand>` | Stable identity persisted as `sbol_jobs.leased_by`; the label on the `sbol_db_worker_*` metric series. Set explicitly when running multiple workers per pod. |
| `config.worker.poolMax` | `SBOL_DB_WORKER_POOL_MAX` | `concurrency + 4` | Override for the worker pool's `max_connections`. The other pool knobs (`DATABASE_*`) apply to both pools. |

#### Logging

| Chart value | Env | Default | Purpose |
|---|---|---|---|
| `config.rustLog` | `RUST_LOG` | `info` | `tracing-subscriber` env-filter directive. Common: `info,sbol_db=debug,sqlx=warn`. |
| `config.logFormat` | `LOG_FORMAT` | `json` (chart default) / auto (binary default) | `json` for structured output; `text`/`plain`/`human` for the human-readable formatter; `auto` (binary only) picks JSON when stdout isn't a TTY. |

`extraEnv:` is appended after the chart-managed env block and can
override any of the above, for one-off cases that don't warrant a new
top-level value.

### Workload knobs (chart-only)

These shape the Deployment, Service, and ServiceAccount; they don't
correspond to env vars on the binary.

| Chart value | Default | Purpose |
|---|---|---|
| `replicaCount` | `1` | sbol-db server replicas. Set `≥ 2` for production. |
| `strategy` | `RollingUpdate{maxSurge: 1, maxUnavailable: 0}` | Deployment update strategy. |
| `terminationGracePeriodSeconds` | `65` | SIGTERM-to-SIGKILL window. Must be ≥ `config.server.requestTimeoutSecs` + worker grace + headroom or in-flight work gets killed on rollouts. |
| `resources` | `requests: {100m, 128Mi}, limits: {1, 512Mi}` | Container resource requests/limits. |
| `image.repository` / `image.tag` / `image.pullPolicy` | `ghcr.io/marpaia/sbol-db` / `appVersion` / `IfNotPresent` | Container image coordinates. |
| `imagePullSecrets` | `[]` | Attached to the Deployment, migration Job, and (when created) the ServiceAccount. |
| `serviceAccount.{create,name,annotations}` | `{true, "", {}}` | Per-release SA with `automountServiceAccountToken: false`. |
| `podSecurityContext`, `securityContext` | distroless-compatible strict defaults | `runAsUser: 65532`, `runAsNonRoot: true`, `readOnlyRootFilesystem: true`, `drop: [ALL]`. |
| `service.{type,port,portName}` | `{ClusterIP, 80, http}` | Service shape. |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints` | `{}` / `[]` | Scheduling controls. |
| `podLabels`, `podAnnotations` | `{}` | Pod metadata. The chart adds `prometheus.io/scrape` annotations automatically when `serviceMonitor.enabled=false`. |
| `extraVolumes`, `extraVolumeMounts` | `[]` | Escape hatch for volumes (e.g. mounting a CA bundle). |

### Probe timings

The probe **paths** are hardcoded (`/healthz`, `/readyz`); only the
timing knobs are exposed. Defaults match the
[probe-tuning guidance](#probe-tuning) under Operating sbol-db.

| Chart value | Default | Purpose |
|---|---|---|
| `livenessProbe.enabled` | `true` | |
| `livenessProbe.initialDelaySeconds` | `5` | |
| `livenessProbe.periodSeconds` | `10` | |
| `livenessProbe.timeoutSeconds` | `2` | |
| `livenessProbe.failureThreshold` | `3` | |
| `readinessProbe.enabled` | `true` | |
| `readinessProbe.initialDelaySeconds` | `3` | |
| `readinessProbe.periodSeconds` | `5` | |
| `readinessProbe.timeoutSeconds` | `2` | Must be > 1s (the `/readyz` internal DB-probe timeout). |
| `readinessProbe.failureThreshold` | `3` | |
| `startupProbe.enabled` | `true` | |
| `startupProbe.periodSeconds` | `5` | |
| `startupProbe.failureThreshold` | `30` | Gives the pod ≈ `periodSeconds × failureThreshold = 150s` to become ready. |

### Migration Job knobs

| Chart value | Default | Purpose |
|---|---|---|
| `migrate.enabled` | `true` | Whether to run the pre-install/pre-upgrade Job at all. |
| `migrate.backoffLimit` | `3` | Retries before the hook is treated as failed. |
| `migrate.ttlSecondsAfterFinished` | `600` | Lingering time for inspection after success. |
| `migrate.resources` | `requests: {50m, 64Mi}, limits: {500m, 256Mi}` | Resource bounds for the short-lived migrate container. |
| `migrate.podLabels` | `{}` | Extra labels on the Job pod. |

### Optional resources (each gated by `.enabled`)

| Chart subtree | Resource emitted | Notes |
|---|---|---|
| `ingress.{enabled,className,annotations,hosts,tls}` | `Ingress` | Multi-host + TLS supported. |
| `autoscaling.{enabled,minReplicas,maxReplicas,targetCPUUtilizationPercentage,targetMemoryUtilizationPercentage}` | `HorizontalPodAutoscaler` v2 | Set memory target to `0` to disable that rule. |
| `podDisruptionBudget.{enabled,minAvailable,maxUnavailable}` | `PodDisruptionBudget` | Exactly one of `minAvailable` / `maxUnavailable`. |
| `serviceMonitor.{enabled,namespace,labels,interval,scrapeTimeout,honorLabels,relabelings,metricRelabelings}` | Prometheus Operator `ServiceMonitor` | Scrapes `/metrics` on the main port. When disabled, fallback `prometheus.io/scrape` annotations are added to the pod. |
| `networkPolicy.{enabled,allowOntologyFetch,extraIngress,extraEgress}` | `NetworkPolicy` | Default-deny; allows ingress on 8080, egress to DNS + Postgres + (optional) HTTPS to OBO Foundry. |

## Operating sbol-db

The cluster-facing surfaces: probes, metrics, and logs.

### Probes and discovery endpoints

| Path | Method | Description |
|---|---|---|
| `/healthz` | `GET` | Static liveness. Returns `200 ok` if the process is running. Does **not** touch Postgres. Wire to `livenessProbe`. |
| `/readyz` | `GET` | Readiness. Issues a 1s-timeout `SELECT 1` against the pool. `200 {"status":"ready"}` or `503 {"status":"not_ready","reason":"…"}`. Wire to `readinessProbe` and `startupProbe`. |
| `/metrics` | `GET` | Prometheus exposition format. See [Metrics](#metrics) below. |
| `/docs` | `GET` | Scalar-rendered API explorer. |
| `/openapi.json` | `GET` | OpenAPI 3.1 schema. |

#### Probe tuning

The chart sets reasonable defaults (see
[Probe timings](#probe-timings)) but a few relationships are worth
understanding:

- **Liveness** failing should be rare and recovery-by-restart. Don't
  point it at `/readyz` — a transient Postgres hiccup would restart the
  pod instead of marking it temporarily unready.
- **Readiness `timeoutSeconds`** must be > `/readyz` internal timeout
  (1s) + `DATABASE_ACQUIRE_TIMEOUT_SECS` worst case. The default chart
  value (`2s`) assumes the pool isn't saturated. Bump it if you see
  readiness flapping under load.
- **Startup probe** uses `/readyz`. With the default `periodSeconds: 5`
  and `failureThreshold: 30`, the pod has ~150s to become ready before
  Kubernetes gives up. Match `DATABASE_STARTUP_TIMEOUT_SECS` to this
  budget (default 30s is well under the 150s ceiling).

### Metrics

Exposed in Prometheus text format at `/metrics` on the main HTTP port
(no separate metrics port — sbol-db has no auth, so cluster-internal
scraping is the assumed model). Add the `monitoring.coreos.com/v1`
ServiceMonitor with `serviceMonitor.enabled=true`.

#### Metric catalog

##### HTTP

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `http_requests_total` | counter | `method`, `route`, `status` | `route` is the **templated** path (e.g. `/objects/:id`), so cardinality stays bounded. Unmatched paths are bucketed as `unmatched`. |
| `http_request_duration_seconds` | histogram | `method`, `route`, `status` | Buckets: 5 ms → 10 s (standard HTTP histogram set). |

##### Connection pools

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_pool_connections` | gauge | `state` ∈ {`open`, `idle`, `in_use`} | API pool snapshot. Taken on each scrape. |
| `sbol_db_worker_pool_connections` | gauge | `state` ∈ {`open`, `idle`, `in_use`} | Worker pool snapshot. Absent when the worker is disabled. |

##### Worker process

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_worker_concurrency` | gauge | `worker_id` | Static at startup. The max simultaneous in-flight handlers for this worker. |
| `sbol_db_worker_inflight` | gauge | `worker_id` | Current in-flight handler count. Increments at dequeue, decrements on completion. |
| `sbol_db_worker_heartbeat_timestamp_seconds` | gauge | `worker_id` | Unix seconds, updated every 5s by the worker. Alert on `time() - this > N` to detect dead/stuck workers. |

##### Job lifecycle

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_jobs_started_total` | counter | `kind`, `queue`, `worker_id` | Incremented on successful dequeue. |
| `sbol_db_jobs_completed_total` | counter | `kind`, `queue`, `status` ∈ {`succeeded`, `failed`, `dead`, `handler_missing`} | Terminal outcome of each handler invocation. `failed` rows re-queue with backoff; `dead` is terminal. |
| `sbol_db_jobs_duration_seconds` | histogram | `kind`, `status` | Handler runtime. Buckets: 10 ms → 1 h. |
| `sbol_db_jobs_wait_seconds` | histogram | `kind`, `queue` | Time from enqueue to first attempt. Recorded only on `attempts = 1` (retries have their own backoff). Buckets: 10 ms → 1 h. |
| `sbol_db_jobs_dequeue_errors_total` | counter | — | Dequeue query failures (DB unreachable, etc.). |

##### Queue state (scrape-time)

These are computed on each `/metrics` call from `sbol_jobs`. The
queries are cheap given the partial dequeue index.

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_jobs_queue_depth` | gauge | `status` ∈ {`queued`, `running`, `failed`, `dead`}, `queue` | Row count per `(status, queue)` bucket. |
| `sbol_db_jobs_oldest_queued_age_seconds` | gauge | `queue` | Age of the oldest still-queued (and available) row per queue. Drives stuck-queue alerts. |
| `sbol_db_jobs_status_enum` | gauge | `status` | Always emitted (value `1`) for every status value. Anchors dashboards so `sum by (status) (queue_depth)` queries don't go blank when a status is absent. |
| `sbol_db_jobs_scrape_errors_total` | counter | `scope` ∈ {`queue_depth`, `oldest_age`} | Increments when the scrape query itself fails. |

##### Lease, reaper, listener

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_jobs_lease_renewals_total` | counter | `result` ∈ {`ok`, `lost`, `error`} | `lost` means the row's status or `leased_by` changed under us — usually because another node reaped an expired lease. |
| `sbol_db_jobs_reaped_total` | counter | — | Total rows transitioned back from `running` → `queued` by the lease reaper. A non-zero rate means workers are losing leases (crashes, network partitions, oversubscribed CPU). |
| `sbol_db_jobs_reap_errors_total` | counter | — | Reaper query failures. |
| `sbol_db_jobs_reaper_last_run_timestamp_seconds` | gauge | — | Unix seconds; the reaper writes it on every tick. Alert on staleness alongside the worker heartbeat. |
| `sbol_db_jobs_listener_connected` | gauge | — | `1` when the worker's `LISTEN` connection is up. `0` means the worker is in poll-only fallback (still correct, just higher tail latency). |
| `sbol_db_jobs_listener_reconnects_total` | counter | `reason` ∈ {`connect_failed`, `listen_failed`, `stream_error`, `stream_closed`} | Listener reconnect attempts. |
| `sbol_db_jobs_notifications_received_total` | counter | `queue` | Successful `NOTIFY sbol_jobs_enqueued` deliveries matched to a subscribed queue. |

##### Build info

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `sbol_db_build_info` | gauge | `version` | Constant `1`; the label carries the binary version. |

#### Useful PromQL

```promql
# --- HTTP -----------------------------------------------------------

# Request rate by route
sum by (route) (rate(http_requests_total[5m]))

# 99th-percentile latency, per route
histogram_quantile(0.99,
  sum by (route, le) (rate(http_request_duration_seconds_bucket[5m])))

# 5xx rate as a fraction
sum(rate(http_requests_total{status=~"5.."}[5m]))
  / sum(rate(http_requests_total[5m]))

# --- Pools ----------------------------------------------------------

# API pool saturation (0..1)
sbol_db_pool_connections{state="in_use"}
  / on() group_left
  (sbol_db_pool_connections{state="open"} + sbol_db_pool_connections{state="idle"})

# Worker pool saturation
sbol_db_worker_pool_connections{state="in_use"}
  / on() group_left
  (sbol_db_worker_pool_connections{state="open"} + sbol_db_worker_pool_connections{state="idle"})

# --- Worker liveness -----------------------------------------------

# Workers whose heartbeat is older than 30s (alert)
time() - sbol_db_worker_heartbeat_timestamp_seconds > 30

# Reaper hasn't run in 2 minutes (every worker runs one — this firing
# means *no* worker is healthy)
time() - sbol_db_jobs_reaper_last_run_timestamp_seconds > 120

# Listener flapping
changes(sbol_db_jobs_listener_connected[5m]) > 2

# --- Job throughput / success --------------------------------------

# Per-kind throughput
sum by (kind) (rate(sbol_db_jobs_completed_total[5m]))

# Per-kind success rate
sum by (kind) (rate(sbol_db_jobs_completed_total{status="succeeded"}[5m]))
  / sum by (kind) (rate(sbol_db_jobs_completed_total[5m]))

# Per-kind p99 handler duration
histogram_quantile(0.99,
  sum by (kind, le) (rate(sbol_db_jobs_duration_seconds_bucket[5m])))

# p99 enqueue→start wait, by queue
histogram_quantile(0.99,
  sum by (queue, le) (rate(sbol_db_jobs_wait_seconds_bucket[5m])))

# --- Stuck-queue / dead-letter alerts ------------------------------

# Queued backlog by queue
sum by (queue) (sbol_db_jobs_queue_depth{status="queued"})

# Oldest queued job in any queue is older than 5 minutes
max(sbol_db_jobs_oldest_queued_age_seconds) > 300

# Jobs landing in dead-letter
sum by (kind) (rate(sbol_db_jobs_completed_total{status="dead"}[15m])) > 0

# Lease churn (workers losing leases under us — investigate CPU starvation,
# network partitions, or under-sized lease intervals)
rate(sbol_db_jobs_reaped_total[5m]) > 0.1
```

### Logging

`tracing-subscriber` is the logger. Two output formats:

- **JSON** (default in containers) — one JSON object per line. Parsed
  cleanly by Loki, Stackdriver, Datadog, etc.
- **Text** (default in interactive shells) — human-readable, with ANSI
  color when stdout is a TTY.

Forced selection via `LOG_FORMAT=json` or `LOG_FORMAT=text`. Verbosity
via `RUST_LOG` (standard `tracing-subscriber` directives).

The `/metrics` and `/healthz` endpoints are noisy when scraped at
typical intervals; if you don't want them in logs add
`tower_http=off,axum=off` to your `RUST_LOG` filter (we don't currently
mount a per-route trace layer; this is preparatory for that change).

#### Notable log events

- `database connect failed; retrying` (warn) — emitted by the
  boot-time retry loop. Each retry includes the next backoff in
  seconds. If you see this repeatedly without a `connected` follow-up,
  `DATABASE_STARTUP_TIMEOUT_SECS` is too low or the DSN is wrong.
- `starting sbol-db worker` (info) — emitted once per worker on
  startup; the structured fields (`worker_id`, `concurrency`,
  `queues`, `kinds`) capture the worker's full subscription state.
  Match `worker_id` against the `sbol_db_worker_*` metric labels.
- `job started` / `job succeeded` / `job failed` (info / warn) — per
  handler invocation, inside a span carrying `job_id`, `kind`, and
  `attempt`. `elapsed_secs` shows handler runtime on completion.
- `lease lost` (warn) — the row's lease was reaped from under a
  running handler. Either the lease was too short for the workload or
  the worker pod was CPU-starved and missed renewals.
- `reaped expired job leases` (warn, with `reclaimed = N`) — the
  reaper re-queued N rows. Investigate alongside `lease lost`.
- `shutdown signal received` (info) — emitted when SIGTERM or SIGINT
  arrives. Followed by `sbol-db server loop exited cleanly` when the
  drain completes; if you don't see that, in-flight work didn't finish
  before `terminationGracePeriodSeconds` elapsed.

## Capacity planning

`sbol-db server` is stateless and CPU-light for typical query
workloads; Postgres does the work. Defaults:

| Resource | Default request | Default limit |
|---|---|---|
| CPU | 100m | 1 |
| Memory | 128Mi | 512Mi |

There are three independent scaling axes — HTTP request throughput,
async-worker throughput, and Postgres capacity.

### Scaling the HTTP path

Three signals to watch:

1. **API pool saturation** — `sbol_db_pool_connections{state="in_use"}`
   near `max_connections`. Either bump `DATABASE_MAX_CONNECTIONS` (and
   the Postgres `max_connections` it consumes) or add replicas.
2. **Request latency p99 drifting past your SLO** — usually pool
   saturation or a hot SPARQL query. Inspect histograms by route.
3. **CPU near limit** — almost always SPARQL or a sequence k-mer
   scan. Scale horizontally; multiple replicas share Postgres without
   coordination.

### Scaling the worker fleet

Three signals to watch:

1. **Queued backlog growing**
   (`sum by (queue) (sbol_db_jobs_queue_depth{status="queued"})`) or
   **oldest-queued age rising** (`sbol_db_jobs_oldest_queued_age_seconds`).
   Increase `SBOL_DB_WORKER_CONCURRENCY`, add pod replicas, or both.
2. **Worker pool saturation**
   (`sbol_db_worker_pool_connections{state="in_use"}`). Raise
   `SBOL_DB_WORKER_POOL_MAX` *or* `DATABASE_MAX_CONNECTIONS` on the
   underlying Postgres if the pool ceiling is the bottleneck rather
   than handler concurrency itself.
3. **Lease churn** (`rate(sbol_db_jobs_reaped_total[5m]) > 0`).
   Workers are losing leases under load — either CPU is starving the
   renewer or jobs run longer than the lease. Scale CPU or increase
   the lease duration in the WorkerConfig.

Workers are stateless. To shift load from API pods to dedicated
worker pods, set `config.worker.disabled=true` on the API Deployment
and run a second Deployment using `sbol-db worker`. Either fleet
can scale horizontally without coordinating with the other; SKIP
LOCKED handles distribution.

### Postgres sizing

The dominant tables grow with the imported design corpus:

- `sbol_triples` — one row per RDF triple, ~10× the typed-object count.
  Plan for tens of millions of rows on a serious deployment.
- `sbol_sequence_kmers` — one row per 8-mer position per nucleotide
  Sequence. Long sequences blow up here; e.g. a 10 kb plasmid yields
  ~10 k rows.
- `sbol_objects.data` — JSON-LD slice per object; bounded by document
  size.
- `sbol_jobs` / `sbol_job_attempts` — one row per enqueued job and per
  attempt. Stays small if traffic is bursty; can grow large if you run
  millions of jobs without pruning terminal (`succeeded`, `dead`,
  `cancelled`) rows. There is no built-in retention sweeper yet — a
  `pg_cron` job that periodically `DELETE`s old terminal rows is the
  recommended pattern.

A managed Postgres with 2 vCPU / 8 GiB RAM handles ≪1 M designs.
Beyond that, the typed projections + GIN indexes start mattering.
Check `EXPLAIN ANALYZE` on slow SPARQL patterns before chasing scale.

## Backup and restore

The chart provisions no backup tooling. Production deployments should:

- Use a managed Postgres with point-in-time recovery (RDS, Cloud SQL,
  Aiven, etc.).
- Or, run `pg_dump` as a sidecar CronJob outside this chart.

The stored triples (`sbol_triples`) are the canonical record, and every
derived projection is deterministically rebuildable from them.
Replaying the triples is a fallback restore path, not a primary backup
strategy.

## Troubleshooting

### Helm install fails with "set externalDatabase.existingSecret.name…"

The chart's DSN resolver requires exactly one of the three modes. Set
`externalDatabase.existingSecret.name`, or `externalDatabase.url`, or
`postgresql.enabled=true` (with `postgresql.auth.password`).

### Migration Job stays Pending forever

The pre-install hook can't schedule. Check:

- ImagePullBackOff — the image tag doesn't exist in GHCR. The chart
  defaults `image.tag` to `appVersion`; supply `--set image.tag=<sha>`
  when running against a dev build.
- The Job's pod sees the same resource constraints as the Deployment;
  if your cluster has a per-namespace quota set tightly, the Job's
  modest request (50m CPU, 64Mi RAM) may still bump it.

### Pod boots, then crash-loops with "connecting to …"

The pod can't reach Postgres before `DATABASE_STARTUP_TIMEOUT_SECS`
expires. Check:

```sh
kubectl logs deploy/sbol-db | grep "database connect failed"
```

If you see retries but no eventual connect, the DSN host/credentials
are wrong, the Postgres pod isn't running, or a NetworkPolicy is
blocking egress. Raise the timeout only if Postgres reliably takes >30s
to become ready — the root cause is usually the DSN.

### `/readyz` flaps between 200 and 503

Most likely the Postgres pool is saturated. Symptoms:

- `sbol_db_pool_connections{state="in_use"}` ≈ `max_connections`.
- `/readyz` 503 body reads `pool timed out while waiting for an open
  connection`.

Fixes (in increasing impact order): raise
`DATABASE_ACQUIRE_TIMEOUT_SECS`, raise `DATABASE_MAX_CONNECTIONS`, add
sbol-db replicas, scale Postgres.

### Imports return 413 Payload Too Large

A single SBOL document exceeds `SBOL_DB_MAX_BODY_BYTES` (default 32
MiB). Raise the limit via `config.server.maxBodyBytes`, or split the
upload across multiple documents.

### Requests return 408 Request Timeout

Server-side wall-clock cap hit. Either the request is genuinely slow
(SPARQL or large neighborhood walk) or the pool is starved. Inspect
`http_request_duration_seconds` histograms by route to localize.
Raising `SBOL_DB_REQUEST_TIMEOUT_SECS` only papers over the symptom.

### `sbol-db ontology fetch so` fails in-cluster

The container has no shell, so you'd run it via:

```sh
kubectl exec deploy/sbol-db -- /usr/local/bin/sbol-db ontology fetch so
```

If `networkPolicy.enabled=true`, the policy must allow egress to OBO
Foundry. Set `networkPolicy.allowOntologyFetch=true` (default) — this
opens 443/TCP egress.

### Jobs stay `queued` and never run

In order of likelihood:

1. **No worker is subscribed.** Check the metric series
   `sbol_db_worker_concurrency{worker_id=...}` exists for at least one
   pod. If absent, every pod is running `--no-worker` (or the worker
   pool failed to open at startup — look for `opening worker connection
   pool` in the logs).
2. **Wrong queue.** The job's `queue` column doesn't match any
   worker's allowlist. `sbol-db jobs status <id>` shows the queue;
   check the startup log line `starting sbol-db worker { … queues:
   [...] }` for what each worker is subscribed to.
3. **Backoff in effect.** A failed retry pins `available_at` up to 1
   hour out. Inspect the row: is `available_at` past `now()`? If not,
   either wait or `UPDATE sbol_jobs SET available_at = now() WHERE id
   = …` for an operator-issued "retry now".

### Worker heartbeat is stale

`time() - sbol_db_worker_heartbeat_timestamp_seconds` is high. The
heartbeat task ticks every 5s, so anything past ~30s is broken. Check:

- The worker pod isn't crash-looping (`kubectl logs --previous`).
- The worker pod isn't CPU-starved (other heavy tasks on the same
  node). The heartbeat is a tokio task; if the runtime can't schedule
  it, the gauge stalls even though the process is "alive."
- Lease churn (`rate(sbol_db_jobs_reaped_total[5m]) > 0`) is the
  symptom: jobs get reaped because lease renewal can't run. Scale up
  CPU or shorten the renewal interval.

### Dead-letter is filling up

`sbol_db_jobs_completed_total{status="dead"}` is incrementing. Each
row hit `max_attempts` (default 5) and stopped retrying. List them:

```sh
sbol-db jobs list --status dead --limit 50
```

The `error` column carries the last failure. Common causes: malformed
payload (deserialisation rejection), a handler that mis-classifies a
permanent failure as retryable, or a downstream system that's been
down longer than the backoff envelope (`60s × 2^4 = 16m`).

## Production checklist

Before declaring a deployment production-ready:

- [ ] Image is built from a tagged commit (`vX.Y.Z`), not a dev SHA.
- [ ] `externalDatabase.existingSecret` points at a managed Postgres
      with point-in-time recovery enabled.
- [ ] `serviceMonitor.enabled=true` and the metrics are visible in your
      Prometheus.
- [ ] Dashboards alert on:
      - 5xx rate > X% for N minutes
      - Pool saturation > 80% for N minutes (both `sbol_db_pool_connections`
        and `sbol_db_worker_pool_connections`)
      - `/readyz` failing for any single pod
      - Worker heartbeat staleness: `time() -
        sbol_db_worker_heartbeat_timestamp_seconds > 60`
      - Stuck queue: `max(sbol_db_jobs_oldest_queued_age_seconds) > 300`
      - Dead-letter rate: `rate(sbol_db_jobs_completed_total{status="dead"}[15m]) > 0`
      - Listener disconnected: `sbol_db_jobs_listener_connected == 0` for > 5 min
- [ ] `config.worker.disabled` set consistently across the fleet (either
      every pod embeds a worker, or API-only pods set it and a dedicated
      worker Deployment runs `sbol-db worker`).
- [ ] `podDisruptionBudget.enabled=true` with `minAvailable: 1` (or
      `maxUnavailable: 25%` for `replicaCount > 1`).
- [ ] `replicaCount ≥ 2` for redundancy.
- [ ] `ingress.tls` configured (or TLS terminated at a managed L7).
- [ ] `terminationGracePeriodSeconds` ≥ `SBOL_DB_REQUEST_TIMEOUT_SECS`
      + worker grace + headroom.
- [ ] `RUST_LOG=info` (not debug) in steady state.

## What this doc intentionally doesn't cover

- **AuthN/Z.** sbol-db has no auth; enforce at the Ingress or via a
  sidecar proxy. The crate guide explains this is by design.
- **Multi-tenancy.** No `organization_id` columns; if you need
  isolation, run separate deployments.
- **Distributed tracing / OpenTelemetry.** JSON logs + request IDs (a
  near-term addition) cover most needs.
- **Async projection consumer.** The job runtime is in place (see
  [Async job runtime](#async-job-runtime)) but there's no built-in
  handler yet that drains `sbol_rdf_projection_events`. Today, all
  projections are still written synchronously inside the import
  transaction; the event log exists for a future projection handler to
  tail deterministically.
