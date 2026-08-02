# Self-contained edge deployment

This topology runs one `sbol-db` process on one durable Linux server. The
process owns RocksDB, attachment blobs, search state, the durable job queue,
native HTTPS, ACME renewal, scheduled complete backups, and the loopback
operations surface. It does not require Caddy, Postgres, Redis, or a separate
worker.

It is intentionally a single-node appliance, not an HA cluster. Recovery is a
verified restore from S3 or GCS onto a replacement server.

## Network and storage contract

Open inbound TCP 443 for HTTPS and ACME TLS-ALPN-01. TCP 80 is optional and is
used only for the canonical HTTP-to-HTTPS redirect. Keep the operations
listener (`127.0.0.1:9090` by default) unreachable from the network; a local
Prometheus or Grafana Alloy process can scrape it.

Use an absolute data path on a durable local filesystem. Production creates:

```text
<data-dir>/
  LAYOUT_VERSION
  LOCK
  CURRENT
  PREVIOUS                         # after a reversible restore
  generations/<uuid>/
    rocksdb/
    blobs/
    search/
    acme/
  backups/                         # encrypted local artifacts and proof sidecars
  restore/                         # private restore staging and journal
    history/                       # bounded recovery-event records
```

`CURRENT` is the only activation pointer. RocksDB, blobs, search state, and
ACME account/certificate state always move together as one generation.

## Required first-launch configuration

```bash
export SBOL_DB_PROFILE=production
export SBOL_DB_DATA_DIR=/var/lib/sbol-db
export SBOL_DB_HOSTNAME=registry.example.org
export SBOL_DB_ACME_CONTACT=ops@example.org

# Public age X25519 recipient only. Store its secret recovery identity offline.
export SBOL_DB_BACKUP_RECOVERY_RECIPIENT=age1...

# Exactly one external complete-backup repository.
export SBOL_DB_BACKUP_REPOSITORY_URL=s3://my-backup-bucket/registry/production
# or: gs://my-backup-bucket/registry/production

# Required only until the first administrator exists; use at least 32 characters.
export SBOL_DB_SETUP_TOKEN='replace-with-a-long-random-bootstrap-secret'

exec /usr/local/bin/sbol-db server
```

S3 and GCS credentials come from the object-store provider's standard
environment or workload identity. Do not put credentials in the repository
URL. The production runtime rejects URL credentials and insecure S3 HTTP.

On first launch, these arguments and environment variables bootstrap a
validated settings document in the RocksDB configuration store. On later
launches, the durable document is authoritative and is applied before the
listener, ACME client, scheduler, object-store client, and disk policy are
constructed. This prevents a partial runtime reconfiguration. The document is
inside the RocksDB checkpoint and therefore participates in the same complete
backup and restore contract as the rest of the generation.

Useful bounded settings are:

| Environment variable | Default | Contract |
|---|---:|---|
| `SBOL_DB_BACKUP_INTERVAL_SECS` | `86400` | 15 minutes to 30 days. The scheduler immediately covers the current wall-clock bucket and durably deduplicates restarts. |
| `SBOL_DB_MIN_FREE_BYTES` | `2147483648` | Readiness fails below this reserve. Backup admission additionally requires a conservative three-times-live-data working estimate. |
| `SBOL_DB_BACKUP_LOCAL_RETENTION` | `2` | 1 to 30 newest local artifacts. An older artifact is pruned only after its object-store readback proof is durable and its local SHA-256 still matches. |
| `SBOL_DB_OPERATIONS_BIND` | `127.0.0.1:9090` | Must remain loopback in production. |
| `SBOL_DB_HTTP_REDIRECT_DISABLED` | `false` | Set only when port 80 must remain closed. ACME still uses port 443. |

The data directory is exclusively locked. A second server process, an offline
restore, or a rollback fails while the active process owns it.

## HTTPS and ACME lifecycle

Production always terminates TLS in-process. The ACME account and certificate
cache is private generation state and is included in every complete backup.
The public listener becomes ready only after a currently valid certificate is
installed in rustls. Renewal events and expiration are exported as metrics.

The hostname must be a DNS name, not an IP address or wildcard. Point its A/AAAA
records at the server before first launch and make TCP 443 reachable from the
public internet.

## Administrator operations

The embedded administrator workspace exposes the production controls without
turning secret material or live listener replacement into browser operations:

- `/admin/settings/edge` reads the active and pending settings, validates and
  persists changes, shows TLS/ACME/disk health, and marks when a process restart
  is required. It manages the hostname, ACME contact and directory, redirect
  policy, TLS handshake timeout, public age recipient, credential-free object
  repository URL, backup cadence and retention, and disk reserve.
- `/admin/operations/backup` triggers the one complete-backup job, shows its
  local and remote verification evidence, links to durable job attempts and
  logs, reports active and previous generations, and supplies copyable offline
  verify/restore/rollback commands.
- `/admin/observability` adds TLS lifetime, ACME lifecycle, disk reserve,
  complete-backup freshness, object-store readback, and pending-restart signals
  to the application metrics already shown there.

Every settings mutation is administrator-authenticated and audited. Cloud
credentials continue to come only from workload identity or provider
environment variables. The private age recovery identity is deliberately not
accepted, stored, or returned by the API. Settings changes become active only
after a deliberate restart; an interrupted request cannot half-reconfigure the
running process.

## The one complete-backup path

Manual requests from the admin UI, scheduled requests, and pre-deploy requests
all enqueue the same durable `complete_backup` job. There is no graph-only
backup mode.

Each `.sbolbackup.age` contains:

- a native consistent RocksDB checkpoint;
- every attachment blob, with referenced SBOL blob hashes checked;
- search state; and
- ACME account and certificate state.

Before success, sbol-db decrypts the artifact, checks every manifest size and
SHA-256, opens RocksDB read-only with the exact column-family set, checks blob
gzip/SHA-1 integrity and RDF references, uploads it, downloads it from the
object store, and runs the same semantic verifier again. Only then does the job
report success and write the local remote-verification sidecar.

For a deployment gate, POST `/api/v2/admin/backup` as an administrator with a
`pre_deploy` trigger and a release-specific idempotency key, then wait for the
returned durable job to succeed. The admin UI's backup button uses the same
endpoint and artifact contract.

## Disaster recovery

Download the selected encrypted object from S3 or GCS to the replacement
server. Keep the server stopped throughout verification and activation.

First inspect and fully verify it:

```bash
sbol-db backup verify \
  --artifact /srv/recovery/selected.sbolbackup.age \
  --identity-file /srv/recovery/recovery.agekey \
  --staging-dir /srv/recovery/verify
```

The JSON report prints an exact `restore_confirmation`. Restore into an empty
or existing managed data directory with that value:

```bash
sbol-db backup restore \
  --artifact /srv/recovery/selected.sbolbackup.age \
  --identity-file /srv/recovery/recovery.agekey \
  --data-dir /var/lib/sbol-db \
  --confirmation 'RESTORE <backup-uuid>'
```

Restore decrypts into private staging under the target filesystem, verifies it,
renames the complete payload into `generations/<backup-uuid>`, verifies it
again, journals the transition, and atomically replaces `CURRENT`. Repeating
the command after a crash resumes or confirms the same generation.

When replacing a valid existing generation, the output includes an exact
`rollback_confirmation` and `PREVIOUS` retains the old generation:

```bash
sbol-db backup rollback \
  --data-dir /var/lib/sbol-db \
  --confirmation 'ROLLBACK <current-generation-uuid>'
```

A pristine disaster-recovery target has no prior valid generation, so rollback
is deliberately unavailable.

Recovery remains an offline, server-stopped operation. The administrator UI
may report the bounded recovery history and generate commands, but it never
uploads an encrypted artifact, accepts a recovery identity, or activates a
generation from the running web process.

## Grafana and alerts

Scrape `http://127.0.0.1:9090/metrics` from a collector on the same host and
forward it to the chosen Prometheus-compatible backend. Import
[`sbol-db-edge-dashboard.json`](../ops/grafana/sbol-db-edge-dashboard.json) into
Grafana. Load [`sbol-db-edge-alerts.yml`](../ops/prometheus/sbol-db-edge-alerts.yml)
as Prometheus-compatible recording/alert rules.

The external dashboard covers disk reserve and backup working space, local and
remote backup freshness, backup outcomes/duration, TLS expiration,
ACME/scheduler events, HTTP rate/latency/errors, and durable queue state. The
alert rules treat stale remote verification, low disk, and missing TLS as
production-critical.
The built-in `/admin/observability` view exposes the most important edge health
signals for an administrator without making the loopback listener public.

The three loopback endpoints have distinct meanings:

- `/healthz` is static process liveness;
- `/readyz` requires a valid TLS certificate, sufficient free disk, and a
  successful storage ping; and
- `/metrics` is the Prometheus exposition surface.

## Release acceptance

At minimum, a production release should pass:

```bash
cargo test -p sbol-db --test backup_restore_smoke
cargo test -p sbol-db-backup -p sbol-db-jobs -p sbol-db-server -p sbol-db
cargo clippy -p sbol-db-backup -p sbol-db-jobs -p sbol-db-server -p sbol-db \
  --all-targets -- -D warnings
```

The black-box restore test creates a real RocksDB checkpoint, encrypted complete
artifact, recovery identity, invokes `sbol-db backup verify` and `backup
restore`, then reopens the restored RocksDB and ACME state from the activated
generation.
