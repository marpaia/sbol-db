# sbol-db Helm chart

Deploys the [sbol-db](https://github.com/marpaia/sbol-db) HTTP server on
Kubernetes. Wraps the `ghcr.io/marpaia/sbol-db` container image with:

- A `Deployment` running `sbol-db serve --bind 0.0.0.0:8080`.
- A pre-install / pre-upgrade `Job` running `sbol-db migrate up` so the
  schema is current before the Deployment rolls.
- Probes wired to `/healthz` (liveness) and `/readyz` (readiness — hits
  Postgres).
- Optional `ServiceMonitor` for Prometheus Operator to scrape `/metrics`.
- Optional `Ingress`, `HorizontalPodAutoscaler`, `PodDisruptionBudget`,
  `NetworkPolicy`.

The chart deliberately exposes only `DATABASE_URL` as configuration; all
other tuning is via Helm-level knobs in `values.yaml` (replicas,
resources, probes, …).

## Quick-start (development)

```sh
helm dependency update charts/sbol-db
helm install sbol-db ./charts/sbol-db \
  --set postgresql.enabled=true \
  --set postgresql.auth.password=changeme
```

This pulls the bitnami/postgresql subchart, runs the migration Job, and
brings up sbol-db. Forward and import:

```sh
kubectl port-forward svc/sbol-db 8080:80
sbol-db --database-url postgres://sbol:changeme@localhost:5432/sbol \
  import path/to/design.ttl
```

## Production

Point at an existing Postgres via a pre-created Secret:

```sh
kubectl create secret generic sbol-db-database \
  --from-literal=url='postgres://user:pass@db.example.com:5432/sbol?sslmode=require'

helm install sbol-db ./charts/sbol-db \
  --set externalDatabase.existingSecret.name=sbol-db-database \
  --set externalDatabase.existingSecret.key=url \
  --set ingress.enabled=true \
  --set ingress.hosts[0].host=sbol-db.example.com \
  --set serviceMonitor.enabled=true
```

## DSN resolution

Exactly one of these three sources is used, in priority order:

1. `externalDatabase.existingSecret.name` — Deployment reads the DSN
   directly from the named Secret/key. Preferred for production.
2. `postgresql.enabled=true` — chart provisions the bitnami subchart;
   you must set `postgresql.auth.password`. The DSN is rendered into a
   chart-managed Secret.
3. `externalDatabase.url` — literal DSN, materialized into a
   chart-managed Secret. Development only.

If none of these are set the chart fails to render with a clear error.

## Operational endpoints

Provided by the binary (see `docs/crate-guide.md`):

| Path       | Used by chart for             |
|------------|-------------------------------|
| `/healthz` | `livenessProbe`               |
| `/readyz`  | `readinessProbe`, `startupProbe` |
| `/metrics` | `ServiceMonitor` (when enabled) |

## Tunable configuration (`config:`)

Everything under `config:` becomes an environment variable on the
sbol-db container; CLI flags aren't used so the chart owns one source
of truth. Sensible defaults work for most clusters; the table below
covers when you'd change them.

| Key                                 | Env var                          | Default     | When to tune |
|-------------------------------------|----------------------------------|-------------|--------------|
| `config.logFormat`                  | `LOG_FORMAT`                     | `json`      | `text` for human-readable logs when running locally. |
| `config.rustLog`                    | `RUST_LOG`                       | `info`      | Set `info,sbol_db=debug,sqlx=warn` to debug a specific crate. |
| `config.bind`                       | `SBOL_DB_BIND`                   | `0.0.0.0:8080` | Change only if you also rebuild the container on a different port. |
| `config.server.requestTimeoutSecs`  | `SBOL_DB_REQUEST_TIMEOUT_SECS`   | `60`        | Lower behind a fast load balancer; raise for huge imports. SPARQL has its own 30s timeout that fires first. |
| `config.server.maxBodyBytes`        | `SBOL_DB_MAX_BODY_BYTES`         | `33554432`  | Raise if you import very large SBOL documents; lower for safety on public endpoints. |
| `config.database.maxConnections`    | `DATABASE_MAX_CONNECTIONS`       | `8`         | Match to `replicaCount * pool` ≤ Postgres `max_connections`. |
| `config.database.minConnections`    | `DATABASE_MIN_CONNECTIONS`       | `0`         | Raise to keep warm connections for spiky workloads. |
| `config.database.acquireTimeoutSecs`| `DATABASE_ACQUIRE_TIMEOUT_SECS`  | `5`         | Bound how long requests wait for a connection. |
| `config.database.idleTimeoutSecs`   | `DATABASE_IDLE_TIMEOUT_SECS`     | `300`       | `0` to disable idle eviction. |
| `config.database.maxLifetimeSecs`   | `DATABASE_MAX_LIFETIME_SECS`     | `1800`      | `0` to disable connection rotation. Useful when behind PgBouncer. |
| `config.database.connectTimeoutSecs`| `DATABASE_CONNECT_TIMEOUT_SECS`  | `5`         | Per-attempt connect cap. |
| `config.database.startupTimeoutSecs`| `DATABASE_STARTUP_TIMEOUT_SECS`  | `30`        | Total budget for the boot-time retry loop. Raise when the migration Job races with a slow Postgres cold-start. |

`extraEnv:` is appended after these and can override any of them, for
the cases where a deployment needs to one-off a value without forking
the chart.

## Things this chart does not do

- **No multi-tenancy.** sbol-db itself has no auth; the chart assumes
  cluster-internal or ingress-level access control.
- **No StatefulSet for sbol-db.** All state lives in Postgres; pods are
  fungible.
- **No vertical pod autoscaler.** HPA only.
- **No automatic ontology preload.** `sbol-db ontology fetch so` is an
  imperative admin step; the chart provides no Job for it (and the
  ontology data goes into Postgres, not a chart-managed volume).
