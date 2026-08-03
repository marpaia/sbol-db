#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command awk
prepare_state_dir

template="$SCRIPT_DIR/fly.toml.tmpl"
temporary="$(mktemp "$FLY_STATE_DIR/fly.toml.XXXXXX")"
trap 'rm -f "$temporary"' EXIT

awk \
  -v app="$FLY_APP" \
  -v region="$FLY_PRIMARY_REGION" \
  -v hostname="$SBOL_DB_HOSTNAME" \
  -v contact="$SBOL_DB_ACME_CONTACT" \
  -v bucket="$SBOL_DB_BACKUP_BUCKET" \
  -v prefix="$SBOL_DB_BACKUP_PREFIX" \
  -v recipient="$SBOL_DB_BACKUP_RECOVERY_RECIPIENT" \
  -v interval="$SBOL_DB_BACKUP_INTERVAL_SECS" \
  -v reserve="$SBOL_DB_MIN_FREE_BYTES" \
  -v volume="$FLY_VOLUME_NAME" \
  -v volume_size="$FLY_VOLUME_SIZE_GB" \
  -v snapshots="$FLY_VOLUME_SNAPSHOT_RETENTION_DAYS" \
  -v extend_threshold="$FLY_VOLUME_AUTO_EXTEND_THRESHOLD" \
  -v extend_increment="$FLY_VOLUME_AUTO_EXTEND_INCREMENT_GB" \
  -v extend_limit="$FLY_VOLUME_AUTO_EXTEND_LIMIT_GB" \
  -v vm_size="$FLY_VM_SIZE" \
  -v vm_memory="$FLY_VM_MEMORY" '
  {
    gsub(/@@FLY_APP@@/, app)
    gsub(/@@FLY_PRIMARY_REGION@@/, region)
    gsub(/@@SBOL_DB_HOSTNAME@@/, hostname)
    gsub(/@@SBOL_DB_ACME_CONTACT@@/, contact)
    gsub(/@@SBOL_DB_BACKUP_BUCKET@@/, bucket)
    gsub(/@@SBOL_DB_BACKUP_PREFIX@@/, prefix)
    gsub(/@@SBOL_DB_BACKUP_RECOVERY_RECIPIENT@@/, recipient)
    gsub(/@@SBOL_DB_BACKUP_INTERVAL_SECS@@/, interval)
    gsub(/@@SBOL_DB_MIN_FREE_BYTES@@/, reserve)
    gsub(/@@FLY_VOLUME_NAME@@/, volume)
    gsub(/@@FLY_VOLUME_SIZE_GB@@/, volume_size)
    gsub(/@@FLY_VOLUME_SNAPSHOT_RETENTION_DAYS@@/, snapshots)
    gsub(/@@FLY_VOLUME_AUTO_EXTEND_THRESHOLD@@/, extend_threshold)
    gsub(/@@FLY_VOLUME_AUTO_EXTEND_INCREMENT_GB@@/, extend_increment)
    gsub(/@@FLY_VOLUME_AUTO_EXTEND_LIMIT_GB@@/, extend_limit)
    gsub(/@@FLY_VM_SIZE@@/, vm_size)
    gsub(/@@FLY_VM_MEMORY@@/, vm_memory)
    print
  }
' "$template" >"$temporary"

if grep -q '@@' "$temporary"; then
  die "unresolved placeholder remains in rendered fly.toml"
fi

mv "$temporary" "$FLY_TOML"
trap - EXIT
chmod 600 "$FLY_TOML"

note "rendered $FLY_TOML"
