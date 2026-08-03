# Fly.io production deployment

This directory manages the single-Machine SBOL DB production appliance. It
uses raw TCP 80/443 because SBOL DB owns TLS and ACME, one encrypted Fly Volume
for the managed generation layout, and a private Tigris bucket for complete
age-encrypted backups.

The scripts never source the repository-root `.env`. They read only
`FLY_API_TOKEN` from it, so Fly's token encoding cannot be interpreted as shell
syntax. In CI, provide `FLY_API_TOKEN` directly.

## Local setup

1. Copy `config.env.example` to ignored `config.env` and fill in the app,
   region, custom hostname, ACME contact, bucket, and immutable image.
2. Generate the offline recovery identity:

   ```bash
   deploy/fly/seed.sh keygen
   ```

   Put the printed public recipient in `config.env`. Move a protected copy of
   `.state/recovery.agekey` to the production recovery vault before cutover.
3. Create the app only, if DNS and region choices are not final yet:

   ```bash
   deploy/fly/bootstrap.sh --app-only
   ```

4. Create the IPs, volume, Tigris bucket, and staged setup token:

   ```bash
   deploy/fly/bootstrap.sh
   ```

   Before first use, initialize the fresh offline volume for the production
   image's nonroot UID with the pinned one-off helper:

   ```bash
   SBOL_DB_VOLUME_INIT_CONFIRM='INITIALIZE sbol-db-production' \
     deploy/fly/init-volume.sh
   ```

5. Build the current source into a uniquely tagged candidate in Fly's private
   registry, then put the printed `FLY_IMAGE` value in `config.env`:

   ```bash
   deploy/fly/build.sh
   ```

6. If production begins from an existing RocksDB, complete the exact seed
   procedure below while the volume is offline and no Machine exists.
7. Configure the hostname's A and AAAA records to the printed Fly addresses.
   Wait for public DNS before deploying; the first boot must complete the
   application's TLS-ALPN-01 challenge.
8. Deploy exactly one Machine:

   ```bash
   deploy/fly/deploy.sh "$FLY_IMAGE"
   deploy/fly/verify.sh "$FLY_IMAGE"
   ```

   Fly gates raw TCP routing on a TCP listener check so the application's
   first TLS-ALPN-01 challenge can reach it. `verify.sh` is the stricter
   post-deploy gate: it requires the public, hostname-validated HTTPS instance
   endpoint to succeed.

## Exact local RocksDB seed

The local `.sbol-db` was populated from the production SynBioHub source. Do not
copy its live directory or individual SST files. Stop every local process that
can own `.sbol-db/rocksdb`, then create one consistent native checkpoint plus
the matching blobs and text index:

```bash
deploy/fly/seed.sh create
```

The command creates an age-encrypted artifact, verifies the archive, opens the
decrypted RocksDB checkpoint read-only, checks the content-addressed blob tree,
and records its SHA-256 and backup UUID in ignored `.state/seed.json`.

With no production Machine running, stage and restore the seed onto the offline
volume:

```bash
deploy/fly/seed.sh upload
SBOL_DB_SEED_CONFIRM='RESTORE sbol-db-production' deploy/fly/seed.sh restore
```

`upload` temporarily places the encrypted artifact and owner-only recovery key
on the volume using a private upload-holder Machine. `restore` attaches the
volume to a one-off offline recovery Machine, atomically activates the verified
generation, confirms a clean process exit, and removes both staged files after
success. Both temporary Machines are removed before cutover. Configure DNS,
then run `deploy.sh` and `verify.sh` for the first public boot. The offline
recovery identity must still be kept outside Fly for future disaster recovery.
The volume initializer is idempotent at the mount root, but it is an initial
provisioning operation and requires zero Machines plus an explicit confirmation.

Do not run `restore` against an instance that has accepted production writes.
That command is an initial cutover operation, not a merge.

## Deployments after cutover

`deploy.sh` refuses to replace an existing Machine unless a pre-deploy backup
has been completed for the exact image or the operator explicitly sets
`SBOL_DB_SKIP_PREDEPLOY_BACKUP=1`. CI should call the authenticated admin backup
endpoint through the included gate and only then invoke `deploy.sh`:

```bash
SBOL_DB_ADMIN_TOKEN=... deploy/fly/predeploy-backup.sh "$FLY_IMAGE"
deploy/fly/deploy.sh "$FLY_IMAGE"
```

The gate requires a succeeded job with both a valid SHA-256 and a remotely
verified Tigris object key. The skip exists for the initial restored deployment
and recovery, not normal releases.

## Offline sole-administrator recovery

Use the recovery workflow only when no current administrator can authenticate
to the normal admin API. It requires an immutable image containing the `users`
recovery command and identifies the target with both its exact username and
email. The workflow:

1. gracefully stops the sole production Machine;
2. waits for a new offline Fly volume snapshot to reach `created`;
3. destroys only the stopped Machine so the encrypted volume can be attached to
   a private one-off Machine;
4. exclusively locks the managed data layout and atomically promotes the target
   while demoting every other administrator;
5. requires a clean helper exit, removes it, deploys the same immutable image,
   and runs the complete public verification gate.

```bash
export SBOL_DB_ADMIN_USERNAME=marpaia
export SBOL_DB_ADMIN_EMAIL=mike@arpaia.co
export SBOL_DB_ADMIN_RECOVERY_CONFIRM="SET SOLE ADMIN $SBOL_DB_ADMIN_USERNAME $SBOL_DB_ADMIN_EMAIL ON $FLY_APP"
deploy/fly/set-sole-admin.sh \
  "$FLY_IMAGE" "$SBOL_DB_ADMIN_USERNAME" "$SBOL_DB_ADMIN_EMAIL"
```

The script records its snapshot and phase in ignored `.state` metadata and can
resume after a failed local process. A second confirmation can explicitly
accept one exact pending snapshot, but normal operations should let the default
30-minute bounded wait complete. Keep a remotely verified application backup
as the primary recovery gate; Fly volume snapshots are an additional recovery
point, not a replacement for application-consistent backups.

The generated `.state/fly.toml`, Fly CLI state, recovery identity, seed
artifact, and seed metadata are ignored by Git. `config.env` is also ignored so
local choices can be rehearsed before the public deployment configuration is
promoted into repository or GitHub Environment variables.
