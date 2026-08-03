#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
prepare_state_dir
require_command cargo
require_command jq

run_sbol_db() {
  cargo run --quiet --locked -p sbol-db -- "$@"
}

seed_keygen() {
  mkdir -p "$(dirname "$SBOL_DB_RECOVERY_IDENTITY_FILE")"
  chmod 700 "$(dirname "$SBOL_DB_RECOVERY_IDENTITY_FILE")"
  run_sbol_db backup keygen \
    --identity-file "$SBOL_DB_RECOVERY_IDENTITY_FILE"
}

seed_create() {
  local source="$SBOL_DB_LOCAL_DATA_DIR"
  local staging
  local temporary_json

  [[ -d "$source/rocksdb" ]] || die "missing local RocksDB: $source/rocksdb"
  [[ -d "$source/blobs" ]] || die "missing local blobs: $source/blobs"
  [[ -d "$source/text-index" ]] || die "missing local text index: $source/text-index"

  seed_keygen >/dev/null
  mkdir -p "$SBOL_DB_SEED_BACKUP_DIR"
  chmod 700 "$SBOL_DB_SEED_BACKUP_DIR"
  staging="$(mktemp -d "$FLY_STATE_DIR/seed-source.XXXXXX")"
  temporary_json="$(mktemp "$FLY_STATE_DIR/seed.json.XXXXXX")"
  trap 'rm -rf "$staging"; rm -f "$temporary_json"' EXIT

  mkdir -p "$staging/search" "$staging/acme"
  note "staging the exact local text index at the managed search root"
  cp -a "$source/text-index/." "$staging/search/"

  note "creating and read-back-verifying the encrypted seed artifact"
  run_sbol_db backup create \
    --database-root "$source/rocksdb" \
    --blobs-root "$source/blobs" \
    --search-root "$staging/search" \
    --acme-root "$staging/acme" \
    --backup-root "$SBOL_DB_SEED_BACKUP_DIR" \
    --identity-file "$SBOL_DB_RECOVERY_IDENTITY_FILE" >"$temporary_json"

  mv "$temporary_json" "$SBOL_DB_SEED_JSON"
  chmod 600 "$SBOL_DB_SEED_JSON"
  trap - EXIT
  rm -rf "$staging"
  note "seed artifact ready"
  jq . "$SBOL_DB_SEED_JSON"
  note "set SBOL_DB_BACKUP_RECOVERY_RECIPIENT to the recipient printed by: $0 keygen"
}

seed_fix_permissions() {
  require_full_config
  [[ -f "$SBOL_DB_SEED_JSON" ]] || die "missing $SBOL_DB_SEED_JSON; run $0 create"

  local artifact
  local backup_id
  local fixer_id
  local fixer_name
  local fixer_status
  local machines_json
  local volume_id
  artifact="$(jq -r .path "$SBOL_DB_SEED_JSON")"
  backup_id="$(jq -r .backup_id "$SBOL_DB_SEED_JSON")"
  machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
  [[ "$(jq 'length' <<<"$machines_json")" == "0" ]] || die \
    "seed permission normalization requires zero Machines so the production volume is offline"
  volume_id="$(fly_cmd volumes list --app "$FLY_APP" --json | jq -r --arg name "$FLY_VOLUME_NAME" '[.[] | select((.Name // .name) == $name and (.State // .state) == "created")][0] | (.ID // .id) // empty')"
  [[ -n "$volume_id" ]] || die "could not resolve the production volume"
  fixer_name="seed-permissions-${backup_id%%-*}"

  note "normalizing staged seed ownership for the production UID with a pinned helper image"
  # The quoted loop is evaluated inside the pinned helper image, not locally.
  # shellcheck disable=SC2016
  if ! fly_cmd machine run \
    --app "$FLY_APP" \
    --region "$FLY_PRIMARY_REGION" \
    --volume "$volume_id:/var/lib/sbol-db" \
    --vm-size shared-cpu-1x \
    --name "$fixer_name" \
    --restart no \
    -- \
    "$FLY_VOLUME_INIT_IMAGE" \
    /bin/sh -ceu \
    'for path do
       test -f "$path"
       test ! -L "$path"
       chown 65532:65532 "$path"
       chmod 0600 "$path"
       stat -c "%u:%g %a %n" "$path"
     done' \
    seed-permissions \
    "/var/lib/sbol-db/$(basename "$artifact")" \
    /var/lib/sbol-db/recovery.agekey; then
    fixer_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$fixer_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
    die "seed permission normalizer failed to start cleanly; leaving Machine ${fixer_id:-$fixer_name} attached for inspection"
  fi

  fixer_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$fixer_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
  [[ -n "$fixer_id" ]] || die "could not resolve the seed permission normalizer Machine"
  wait_for_machine_state "$FLY_APP" "$fixer_id" stopped 600
  fixer_status="$(fly_cmd machine status --app "$FLY_APP" "$fixer_id")"
  printf '%s\n' "$fixer_status"
  if ! grep -q 'exit_code=0,oom_killed=false' <<<"$fixer_status"; then
    die "seed permission normalization did not report a clean exit; leaving Machine $fixer_id attached for inspection"
  fi
  fly_cmd machine destroy --app "$FLY_APP" "$fixer_id"
}

seed_upload() {
  require_full_config
  [[ -f "$SBOL_DB_SEED_JSON" ]] || die "missing $SBOL_DB_SEED_JSON; run $0 create"
  [[ -f "$SBOL_DB_RECOVERY_IDENTITY_FILE" ]] || die "missing recovery identity"

  local artifact
  local backup_id
  local holder_id
  local holder_name
  local image
  local machines_json
  local machine_id
  local volume_id
  artifact="$(jq -r .path "$SBOL_DB_SEED_JSON")"
  backup_id="$(jq -r .backup_id "$SBOL_DB_SEED_JSON")"
  [[ -f "$artifact" ]] || die "seed artifact not found: $artifact"
  machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
  [[ "$(jq 'length' <<<"$machines_json")" == "0" ]] || die \
    "seed upload requires zero Machines so the production volume is offline"
  volume_id="$(fly_cmd volumes list --app "$FLY_APP" --json | jq -r --arg name "$FLY_VOLUME_NAME" '[.[] | select((.Name // .name) == $name and (.State // .state) == "created")][0] | (.ID // .id) // empty')"
  [[ -n "$volume_id" ]] || die "could not resolve the production volume"
  image="${FLY_IMAGE:-}"
  [[ -n "$image" ]] || die "set FLY_IMAGE to the candidate image"
  holder_name="seed-upload-${backup_id%%-*}"
  holder_id=""
  cleanup_upload_holder() {
    local target="${holder_id:-$holder_name}"
    fly_cmd machine stop --app "$FLY_APP" "$target" >/dev/null 2>&1 || true
    wait_for_machine_state "$FLY_APP" "$target" stopped 120 >/dev/null 2>&1 || true
    fly_cmd machine destroy --app "$FLY_APP" "$target" >/dev/null 2>&1 || true
  }
  trap cleanup_upload_holder EXIT

  note "starting a private, temporary upload holder on the offline volume"
  fly_cmd machine run \
    --app "$FLY_APP" \
    --region "$FLY_PRIMARY_REGION" \
    --volume "$volume_id:/var/lib/sbol-db" \
    --vm-size shared-cpu-1x \
    --name "$holder_name" \
    --restart no \
    -- \
    "$image" \
    --database-url rocksdb:///tmp/sbol-db-upload/rocksdb \
    server \
    --blob-root /tmp/sbol-db-upload/blobs \
    --bind 127.0.0.1:8888 \
    --operations-bind 127.0.0.1:9090 \
    --no-worker
  holder_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$holder_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
  [[ -n "$holder_id" ]] || die "could not resolve the temporary upload holder"
  machine_id="$holder_id"

  note "uploading encrypted seed artifact to the volume restore area"
  fly_cmd ssh sftp put "$artifact" \
    "/var/lib/sbol-db/$(basename "$artifact")" \
    --app "$FLY_APP" --machine "$machine_id" --user root --mode 0600
  note "uploading the temporary recovery identity with owner-only permissions"
  fly_cmd ssh sftp put "$SBOL_DB_RECOVERY_IDENTITY_FILE" \
    "/var/lib/sbol-db/recovery.agekey" \
    --app "$FLY_APP" --machine "$machine_id" --user root --mode 0600

  note "stopping and removing the temporary upload holder"
  cleanup_upload_holder
  if fly_cmd machine list --app "$FLY_APP" --json | jq -e --arg name "$holder_name" \
    'any(.[]?; (.name // .Name) == $name)' >/dev/null; then
    die "temporary upload holder $holder_name still exists after cleanup"
  fi
  holder_id=""
  trap - EXIT
  seed_fix_permissions
}

seed_restore() {
  require_full_config
  [[ -f "$SBOL_DB_SEED_JSON" ]] || die "missing $SBOL_DB_SEED_JSON"
  [[ "${SBOL_DB_SEED_CONFIRM:-}" == "RESTORE $FLY_APP" ]] || die \
    "set SBOL_DB_SEED_CONFIRM='RESTORE $FLY_APP' to authorize replacing the empty production generation"

  local artifact
  local backup_id
  local machines_json
  local volume_id
  local image
  local recovery_id
  local recovery_name
  local recovery_status
  artifact="$(jq -r .path "$SBOL_DB_SEED_JSON")"
  backup_id="$(jq -r .backup_id "$SBOL_DB_SEED_JSON")"
  recovery_name="seed-restore-${backup_id%%-*}"
  machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
  recovery_id="$(jq -r --arg name "$recovery_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty' <<<"$machines_json")"
  if [[ -n "$recovery_id" ]]; then
    [[ "$(jq 'length' <<<"$machines_json")" == "1" ]] || die \
      "cannot resume offline recovery while another Machine exists"
    note "resuming existing offline recovery Machine $recovery_id"
  else
    [[ "$(jq 'length' <<<"$machines_json")" == "0" ]] || die \
      "seed restore requires zero Machines so the production volume is offline"
  fi
  image="${FLY_IMAGE:-}"
  [[ -n "$image" ]] || die "set FLY_IMAGE to the deployed immutable image"
  volume_id="$(fly_cmd volumes list --app "$FLY_APP" --json | jq -r --arg name "$FLY_VOLUME_NAME" '[.[] | select((.Name // .name) == $name and (.State // .state) == "created")][0] | (.ID // .id) // empty')"
  [[ -n "$volume_id" ]] || die "could not resolve the production volume"
  if [[ -z "$recovery_id" ]]; then
    note "atomically restoring backup $backup_id with a one-off volume-owning Machine"
    fly_cmd machine run \
      --app "$FLY_APP" \
      --region "$FLY_PRIMARY_REGION" \
      --volume "$volume_id:/var/lib/sbol-db" \
      --vm-size "$FLY_VM_SIZE" \
      --vm-memory "$FLY_VM_MEMORY" \
      --name "$recovery_name" \
      --restart no \
      -- \
      "$image" \
      backup restore \
      --artifact "/var/lib/sbol-db/$(basename "$artifact")" \
      --identity-file /var/lib/sbol-db/recovery.agekey \
      --data-dir /var/lib/sbol-db \
      --confirmation "RESTORE $backup_id" \
      --remove-artifact-on-success \
      --remove-identity-on-success

    recovery_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$recovery_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
    [[ -n "$recovery_id" ]] || die "could not resolve the offline recovery Machine"
  fi
  note "waiting for offline recovery Machine $recovery_id to exit"
  wait_for_machine_state "$FLY_APP" "$recovery_id" stopped 7200 10
  recovery_status="$(fly_cmd machine status --app "$FLY_APP" "$recovery_id")"
  printf '%s\n' "$recovery_status"
  if ! grep -q 'exit_code=0,oom_killed=false' <<<"$recovery_status"; then
    die "offline recovery did not report a clean exit; leaving Machine $recovery_id attached for inspection"
  fi
  fly_cmd machine destroy --app "$FLY_APP" "$recovery_id"
  note "production data is restored offline; configure DNS before the first deploy"
}

case "${1:-}" in
  keygen) seed_keygen ;;
  create) seed_create ;;
  upload) seed_upload ;;
  fix-permissions) seed_fix_permissions ;;
  restore) seed_restore ;;
  *) die "usage: $0 {keygen|create|upload|fix-permissions|restore}" ;;
esac
