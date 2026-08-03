#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command fly
require_command jq
require_command shasum
prepare_state_dir

image="${1:-${FLY_IMAGE:-}}"
username="${2:-}"
email="${3:-}"
[[ -n "$image" && -n "$username" && -n "$email" ]] || die \
  "usage: $0 <immutable-image-reference> <username> <email>"
[[ "$image" != *:latest ]] || die "refusing to use a latest tag"
[[ "$username" =~ ^[A-Za-z0-9_.-]+$ ]] || die "username contains unsupported characters"
[[ "$email" =~ ^[^[:space:]@]+@[^[:space:]@]+$ ]] || die "email address is invalid"

confirmation="SET SOLE ADMIN $username $email ON $FLY_APP"
[[ "${SBOL_DB_ADMIN_RECOVERY_CONFIRM:-}" == "$confirmation" ]] || die \
  "set SBOL_DB_ADMIN_RECOVERY_CONFIRM='$confirmation' to authorize the offline role change"

operation_hash="$(printf '%s\n%s\n%s\n' "$image" "$username" "$email" | shasum -a 256 | awk '{print substr($1, 1, 12)}')"
helper_name="admin-recovery-$operation_hash"
state_file="$FLY_STATE_DIR/admin-recovery-$operation_hash.json"

if [[ -f "$state_file" ]] && jq -e \
  --arg image "$image" --arg username "$username" --arg email "$email" \
  '.status == "complete" and .image == $image and .username == $username and .email == $email' \
  "$state_file" >/dev/null; then
  note "administrator recovery is already recorded complete; verifying the deployment"
  "$SCRIPT_DIR/verify.sh" "$image"
  exit 0
fi

volumes_json="$(fly_cmd volumes list --app "$FLY_APP" --json)"
volume_id="$(jq -r --arg name "$FLY_VOLUME_NAME" '[.[] | select((.Name // .name) == $name and (.State // .state) == "created")][0] | (.ID // .id) // empty' <<<"$volumes_json")"
[[ -n "$volume_id" ]] || die "could not resolve the production volume"

machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
helper_id="$(jq -r --arg name "$helper_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty' <<<"$machines_json")"
if [[ -n "$helper_id" ]]; then
  [[ "$(jq 'length' <<<"$machines_json")" == "1" ]] || die \
    "the recovery helper exists alongside another Machine; refusing to continue"
  note "resuming administrator recovery helper $helper_id"
else
  machine_count="$(jq 'length' <<<"$machines_json")"
  if [[ "$machine_count" == "1" ]]; then
    production_id="$(jq -r '.[0].id // .[0].ID' <<<"$machines_json")"
    production_state="$(jq -r '.[0].state // .[0].State' <<<"$machines_json")"
    case "$production_state" in
      started)
        note "gracefully stopping production Machine $production_id"
        fly_cmd machine stop --app "$FLY_APP" --signal SIGTERM --timeout 300 \
          --wait-timeout 7m "$production_id"
        ;;
      stopped) ;;
      *) die "production Machine $production_id is in unexpected state $production_state" ;;
    esac
    wait_for_machine_state "$FLY_APP" "$production_id" stopped 420

    if [[ -f "$state_file" ]]; then
      jq -e --arg image "$image" --arg username "$username" --arg email "$email" \
        '(.status == "snapshot_pending" or .status == "snapshotted") and .image == $image and .username == $username and .email == $email and (.snapshot.id | length > 0)' \
        "$state_file" >/dev/null || die "administrator recovery state does not match this operation"
      snapshot_id="$(jq -r '.snapshot.id' "$state_file")"
      note "resuming recorded Fly volume snapshot $snapshot_id"
    else
      snapshots_json="$(fly_cmd volumes snapshots list "$volume_id" --app "$FLY_APP" --json)"
      running_snapshot_count="$(jq '[.[] | select(.status == "running")] | length' <<<"$snapshots_json")"
      if [[ "$running_snapshot_count" == "1" ]]; then
        snapshot_id="$(jq -r '.[] | select(.status == "running") | .id' <<<"$snapshots_json")"
        note "resuming offline Fly volume snapshot $snapshot_id"
      elif [[ "$running_snapshot_count" == "0" ]]; then
        note "creating an offline Fly volume snapshot before changing roles"
        if ! snapshot_output="$(fly_cmd volumes snapshots create "$volume_id" --app "$FLY_APP" --json 2>&1)"; then
          printf '%s\n' "$snapshot_output" >&2
          die "Fly volume snapshot request failed"
        fi
        snapshot_id="$(jq -r '.id // empty' <<<"$snapshot_output" 2>/dev/null || true)"
        if [[ -z "$snapshot_id" ]]; then
          snapshot_id="$(grep -Eo 'vs_[A-Za-z0-9]+' <<<"$snapshot_output" | head -1 || true)"
        fi
        if [[ -z "$snapshot_id" ]]; then
          snapshots_json="$(fly_cmd volumes snapshots list "$volume_id" --app "$FLY_APP" --json)"
          [[ "$(jq '[.[] | select(.status == "running")] | length' <<<"$snapshots_json")" == "1" ]] || die \
            "could not resolve the requested Fly volume snapshot"
          snapshot_id="$(jq -r '.[] | select(.status == "running") | .id' <<<"$snapshots_json")"
        fi
      else
        die "multiple Fly volume snapshots are already running; refusing to choose one"
      fi

      snapshots_json="$(fly_cmd volumes snapshots list "$volume_id" --app "$FLY_APP" --json)"
      snapshot_json="$(jq -c --arg id "$snapshot_id" '[.[] | select(.id == $id)][0] // empty' <<<"$snapshots_json")"
      [[ -n "$snapshot_json" ]] || die "Fly volume snapshot $snapshot_id disappeared"
      temporary="$(mktemp "$FLY_STATE_DIR/admin-recovery.json.XXXXXX")"
      jq -n \
        --arg image "$image" \
        --arg username "$username" \
        --arg email "$email" \
        --arg production_machine_id "$production_id" \
        --arg volume_id "$volume_id" \
        --argjson snapshot "$snapshot_json" \
        '{version:1,status:"snapshot_pending",image:$image,username:$username,email:$email,production_machine_id:$production_machine_id,volume_id:$volume_id,snapshot:$snapshot}' \
        >"$temporary"
      mv "$temporary" "$state_file"
      chmod 600 "$state_file"
    fi

    note "waiting for offline Fly volume snapshot $snapshot_id to complete"
    snapshot_deadline=$((SECONDS + ${SBOL_DB_SNAPSHOT_WAIT_SECS:-1800}))
    while ((SECONDS < snapshot_deadline)); do
      snapshots_json="$(fly_cmd volumes snapshots list "$volume_id" --app "$FLY_APP" --json)"
      snapshot_json="$(jq -c --arg id "$snapshot_id" '[.[] | select(.id == $id)][0] // empty' <<<"$snapshots_json")"
      [[ -n "$snapshot_json" ]] || die "Fly volume snapshot $snapshot_id disappeared"
      snapshot_status="$(jq -r '.status' <<<"$snapshot_json")"
      case "$snapshot_status" in
        created | complete | completed | success) break ;;
        running | pending)
          if [[ "${SBOL_DB_ACCEPT_PENDING_SNAPSHOT:-}" == "ACCEPT PENDING SNAPSHOT $snapshot_id" ]]; then
            note "recording explicitly accepted pending snapshot $snapshot_id"
            break
          fi
          sleep 5
          ;;
        *) die "Fly volume snapshot $snapshot_id ended with status $snapshot_status" ;;
      esac
    done
    if [[ "$snapshot_status" == "running" || "$snapshot_status" == "pending" ]]; then
      [[ "${SBOL_DB_ACCEPT_PENDING_SNAPSHOT:-}" == "ACCEPT PENDING SNAPSHOT $snapshot_id" ]] || die \
        "timed out waiting for Fly volume snapshot $snapshot_id"
    fi
    temporary="$(mktemp "$FLY_STATE_DIR/admin-recovery.json.XXXXXX")"
    jq --arg status "snapshotted" --argjson snapshot "$snapshot_json" \
      '.status = $status | .snapshot = $snapshot' "$state_file" >"$temporary"
    mv "$temporary" "$state_file"
    chmod 600 "$state_file"

    note "destroying the stopped production Machine so the volume can be attached offline"
    fly_cmd machine destroy --app "$FLY_APP" "$production_id"
  elif [[ "$machine_count" == "0" ]]; then
    [[ -f "$state_file" ]] || die \
      "zero Machines exist but no snapshot state is recorded at $state_file"
  else
    die "expected zero or one production Machine, found $machine_count"
  fi

  note "running the atomic sole-administrator update on the offline managed generation"
  fly_cmd machine run \
    --app "$FLY_APP" \
    --region "$FLY_PRIMARY_REGION" \
    --volume "$volume_id:/var/lib/sbol-db" \
    --vm-size performance-1x \
    --vm-memory 4096 \
    --name "$helper_name" \
    --restart no \
    -- \
    "$image" \
    users \
    --data-dir /var/lib/sbol-db \
    set-sole-admin \
    --username "$username" \
    --email "$email" \
    --confirmation "set-sole-admin:$username:$email"

  helper_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$helper_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
  [[ -n "$helper_id" ]] || die "could not resolve administrator recovery helper"
fi

note "waiting for administrator recovery helper $helper_id to exit"
wait_for_machine_state "$FLY_APP" "$helper_id" stopped 900
helper_status="$(fly_cmd machine status --app "$FLY_APP" "$helper_id")"
printf '%s\n' "$helper_status"
if ! grep -q 'exit_code=0,oom_killed=false' <<<"$helper_status"; then
  die "administrator recovery did not exit cleanly; leaving Machine $helper_id attached for inspection"
fi

fly_cmd machine destroy --app "$FLY_APP" "$helper_id"
note "deploying the same validated image as the sole production Machine"
"$SCRIPT_DIR/deploy.sh" "$image"
"$SCRIPT_DIR/verify.sh" "$image"

temporary="$(mktemp "$FLY_STATE_DIR/admin-recovery.json.XXXXXX")"
jq --arg completed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '.status = "complete" | .completed_at = $completed_at' "$state_file" >"$temporary"
mv "$temporary" "$state_file"
chmod 600 "$state_file"
note "sole-administrator recovery completed and verified"
