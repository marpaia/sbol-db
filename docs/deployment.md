# Deploying sbol-db

This is the operator's reference: container image, Helm chart,
configuration knobs, probes, metrics, logging, shutdown semantics, and
troubleshooting. It does **not** cover the API surface — see the
[crate guide](crate-guide.md) for architecture and `/docs` (or
[openapi.json](../crates/sbol-db-server/src/openapi.json)) for the REST
API.

## Audience

You're standing up sbol-db in a real environment (k8s cluster, managed
Postgres, observability stack). If you're just trying the CLI locally,
the [README quickstart](../README.md#installation) is faster.

## Topology in one paragraph

`sbol-db serve` is a single stateless HTTP binary that talks to one
Postgres instance. Everything — the typed objects, the RDF quad store,
the typed projections, the k-mer index, ontology closures — lives in
Postgres. Multiple `sbol-db` pods can share a database safely; there's
no in-process state to coordinate. Migrations are idempotent and run
before the serve pods roll. The HTTP surface exposes the SBOL query
API plus three operational endpoints (`/healthz`, `/readyz`, `/metrics`)
on a single port.

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
- `ENTRYPOINT ["/usr/local/bin/sbol-db"]`, default `CMD ["serve",
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

`sbol-db migrate up` runs as a Helm hook:

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

| Chart value | Env | Default | Purpose |
|---|---|---|---|
| `config.database.maxConnections` | `DATABASE_MAX_CONNECTIONS` | `8` | Pool ceiling. |
| `config.database.minConnections` | `DATABASE_MIN_CONNECTIONS` | `0` | Idle floor (warm pool). |
| `config.database.acquireTimeoutSecs` | `DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | Per-request wait when the pool is saturated. |
| `config.database.idleTimeoutSecs` | `DATABASE_IDLE_TIMEOUT_SECS` | `300` | Idle eviction. `0` disables. |
| `config.database.maxLifetimeSecs` | `DATABASE_MAX_LIFETIME_SECS` | `1800` | Hard connection lifetime. `0` disables. Useful behind PgBouncer. |
| `config.database.connectTimeoutSecs` | `DATABASE_CONNECT_TIMEOUT_SECS` | `5` | Per-attempt connect cap. |
| `config.database.startupTimeoutSecs` | `DATABASE_STARTUP_TIMEOUT_SECS` | `30` (chart-set on serve / migrate) or `0` (other CLI commands) | Total budget for the boot-time retry loop. |

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
| `replicaCount` | `1` | sbol-db serve replicas. Set `≥ 2` for production. |
| `strategy` | `RollingUpdate{maxSurge: 1, maxUnavailable: 0}` | Deployment update strategy. |
| `terminationGracePeriodSeconds` | `65` | SIGTERM-to-SIGKILL window. Must be ≥ `config.server.requestTimeoutSecs` + headroom or in-flight requests get killed on rollouts. |
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
[probe-tuning guidance](#probe-tuning) above.

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

## Operational endpoints

| Path | Method | Description |
|---|---|---|
| `/healthz` | `GET` | Static liveness. Returns `200 ok` if the process is running. Does **not** touch Postgres. Wire to `livenessProbe`. |
| `/readyz` | `GET` | Readiness. Issues a 1s-timeout `SELECT 1` against the pool. `200 {"status":"ready"}` or `503 {"status":"not_ready","reason":"…"}`. Wire to `readinessProbe` and `startupProbe`. |
| `/metrics` | `GET` | Prometheus exposition format. |
| `/docs` | `GET` | Scalar-rendered API explorer. |
| `/openapi.json` | `GET` | OpenAPI 3.1 schema. |

### Probe tuning

The chart sets reasonable defaults but a few relationships are worth
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

## Metrics

Exposed in Prometheus text format at `/metrics` on the main HTTP port
(no separate metrics port — sbol-db has no auth, so cluster-internal
scraping is the assumed model). Add the `monitoring.coreos.com/v1`
ServiceMonitor with `serviceMonitor.enabled=true`.

### Metric catalog

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `http_requests_total` | counter | `method`, `route`, `status` | `route` is the **templated** path (e.g. `/objects/:id`), so cardinality stays bounded. Unmatched paths are bucketed as `unmatched`. |
| `http_request_duration_seconds` | histogram | `method`, `route`, `status` | Buckets: 5 ms → 10 s (standard HTTP histogram set). |
| `sbol_db_pool_connections` | gauge | `state` ∈ {`open`, `idle`, `in_use`} | Snapshot taken on each scrape. |
| `sbol_db_build_info` | gauge | `version` | Constant 1; the label carries the binary version. |

### Useful PromQL

```promql
# Request rate by route
sum by (route) (rate(http_requests_total[5m]))

# 99th-percentile latency, per route
histogram_quantile(0.99, sum by (route, le) (rate(http_request_duration_seconds_bucket[5m])))

# Error rate as a fraction
sum(rate(http_requests_total{status=~"5.."}[5m]))
  / sum(rate(http_requests_total[5m]))

# Pool saturation
sbol_db_pool_connections{state="in_use"} / on() group_left
  (sbol_db_pool_connections{state="open"} + sbol_db_pool_connections{state="idle"})
```

Domain metrics (imports, sparql queries, document/quad gauges) are on
the [operability roadmap](operations-roadmap) but not yet implemented.

## Logging

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

### Notable log events

- `database connect failed; retrying` (warn) — emitted by the
  boot-time retry loop. Each retry includes the next backoff in
  seconds. If you see this repeatedly without a `connected` follow-up,
  `DATABASE_STARTUP_TIMEOUT_SECS` is too low or the DSN is wrong.
- `shutdown signal received` (info) — emitted when SIGTERM or SIGINT
  arrives. Followed by `sbol-db serve loop exited cleanly` when the
  drain completes; if you don't see that, in-flight requests didn't
  finish before `terminationGracePeriodSeconds` elapsed.

## Graceful shutdown

The `serve` subcommand installs a SIGTERM + SIGINT handler and routes
the signal into `axum::serve(...).with_graceful_shutdown(...)`. On
signal:

1. The listener stops accepting new connections.
2. In-flight requests run to completion (bounded by their per-request
   timeout).
3. Once the last response is sent, the process exits 0.

Kubernetes pod termination sequence:

1. Pod marked Terminating; removed from Service endpoints; SIGTERM
   sent to PID 1.
2. sbol-db enters drain mode.
3. After `terminationGracePeriodSeconds` (default 30s), kubelet sends
   SIGKILL.

Match `terminationGracePeriodSeconds` ≥
`SBOL_DB_REQUEST_TIMEOUT_SECS` + a few seconds of headroom, otherwise
SIGKILL will arrive mid-request.

## Capacity planning

`sbol-db serve` is stateless and CPU-light for typical query workloads;
Postgres does the work. Defaults:

| Resource | Default request | Default limit |
|---|---|---|
| CPU | 100m | 1 |
| Memory | 128Mi | 512Mi |

Three signals to scale on:

1. **Pool saturation** (`sbol_db_pool_connections{state="in_use"}` near
   `max_connections`). Either bump `DATABASE_MAX_CONNECTIONS` (and the
   Postgres `max_connections` it consumes) or add replicas.
2. **Request latency p99** drifting past your SLO — usually pool
   saturation or a hot SPARQL query. Inspect histograms by route.
3. **CPU near limit** — almost always SPARQL or a sequence k-mer scan.
   Scale horizontally; multiple replicas share Postgres without
   coordination.

### Postgres sizing

The dominant tables grow with the imported design corpus:

- `sbol_quads` — one row per RDF triple, ~10× the typed-object count.
  Plan for tens of millions of rows on a serious deployment.
- `sequence_kmers` — one row per 8-mer position per nucleotide
  Sequence. Long sequences blow up here; e.g. a 10 kb plasmid yields
  ~10 k rows.
- `sbol_objects.data` — JSON-LD slice per object; bounded by document
  size.

A managed Postgres with 2 vCPU / 8 GiB RAM handles ≪1 M designs.
Beyond that, the typed projections + GIN indexes start mattering.
Check `EXPLAIN ANALYZE` on slow SPARQL patterns before chasing scale.

## Backup and restore

The chart provisions no backup tooling. Production deployments should:

- Use a managed Postgres with point-in-time recovery (RDS, Cloud SQL,
  Aiven, etc.).
- Or, run `pg_dump` as a sidecar CronJob outside this chart.

Since every projection is deterministically rebuildable from the
imported documents, a full restore from the raw payloads
(`sbol_documents.raw_payload`) is a fallback option — but it's a
last-resort path, not a primary backup strategy.

## Troubleshooting

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

### Migration Job stays Pending forever

The pre-install hook can't schedule. Check:

- ImagePullBackOff — the image tag doesn't exist in GHCR. The chart
  defaults `image.tag` to `appVersion`; supply `--set image.tag=<sha>`
  when running against a dev build.
- The Job's pod sees the same resource constraints as the Deployment;
  if your cluster has a per-namespace quota set tightly, the Job's
  modest request (50m CPU, 64Mi RAM) may still bump it.

### `/readyz` flaps between 200 and 503

Most likely the Postgres pool is saturated. Symptoms:

- `sbol_db_pool_connections{state="in_use"}` ≈ `max_connections`.
- `/readyz` 503 body reads `pool timed out while waiting for an open
  connection`.

Fixes (in increasing impact order): raise
`DATABASE_ACQUIRE_TIMEOUT_SECS`, raise `DATABASE_MAX_CONNECTIONS`, add
sbol-db replicas, scale Postgres.

### `sbol-db ontology fetch so` fails in-cluster

The container has no shell, so you'd run it via:

```sh
kubectl exec deploy/sbol-db -- /usr/local/bin/sbol-db ontology fetch so
```

If `networkPolicy.enabled=true`, the policy must allow egress to OBO
Foundry. Set `networkPolicy.allowOntologyFetch=true` (default) — this
opens 443/TCP egress.

### Imports return 413 Payload Too Large

A single SBOL document exceeds `SBOL_DB_MAX_BODY_BYTES` (default 32
MiB). Raise the limit via `config.server.maxBodyBytes`, or split the
upload across multiple documents.

### Requests return 408 Request Timeout

Server-side wall-clock cap hit. Either the request is genuinely slow
(SPARQL or large neighborhood walk) or the pool is starved. Inspect
`http_request_duration_seconds` histograms by route to localize.
Raising `SBOL_DB_REQUEST_TIMEOUT_SECS` only papers over the symptom.

### Helm install fails with "set externalDatabase.existingSecret.name…"

The chart's DSN resolver requires exactly one of the three modes. Set
`externalDatabase.existingSecret.name`, or `externalDatabase.url`, or
`postgresql.enabled=true` (with `postgresql.auth.password`).

## Production checklist

Before declaring a deployment production-ready:

- [ ] Image is built from a tagged commit (`vX.Y.Z`), not a dev SHA.
- [ ] `externalDatabase.existingSecret` points at a managed Postgres
      with point-in-time recovery enabled.
- [ ] `serviceMonitor.enabled=true` and the metrics are visible in your
      Prometheus.
- [ ] Dashboards alert on:
      - 5xx rate > X% for N minutes
      - Pool saturation > 80% for N minutes
      - `/readyz` failing for any single pod
- [ ] `podDisruptionBudget.enabled=true` with `minAvailable: 1` (or
      `maxUnavailable: 25%` for `replicaCount > 1`).
- [ ] `replicaCount ≥ 2` for redundancy.
- [ ] `ingress.tls` configured (or TLS terminated at a managed L7).
- [ ] `terminationGracePeriodSeconds` ≥ `SBOL_DB_REQUEST_TIMEOUT_SECS`
      with headroom.
- [ ] `RUST_LOG=info` (not debug) in steady state.

## What this doc intentionally doesn't cover

- **AuthN/Z.** sbol-db has no auth; enforce at the Ingress or via a
  sidecar proxy. The crate guide explains this is by design.
- **Multi-tenancy.** No `organization_id` columns; if you need
  isolation, run separate deployments.
- **Distributed tracing / OpenTelemetry.** JSON logs + request IDs (a
  near-term addition) cover most needs.
- **Background workers.** The `rdf_projection_events` table exists for
  a future async consumer; today, all projections are written
  synchronously inside the import transaction.
