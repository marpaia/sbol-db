#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command curl
require_command fly
require_command jq
prepare_state_dir

release="${1:-${FLY_IMAGE:-}}"
[[ -n "$release" ]] || die "usage: $0 <immutable-image-reference> (or set FLY_IMAGE)"
(( ${#release} <= 200 )) || die "release identity exceeds the API's 200-character idempotency limit"

machine_count="$(fly_cmd machine list --app "$FLY_APP" --json | jq 'length')"
if [[ "$machine_count" == "0" ]]; then
  note "no existing Machine; pre-deploy backup is not applicable to the initial deployment"
  exit 0
fi
require_vars SBOL_DB_ADMIN_TOKEN

base_url="https://$SBOL_DB_HOSTNAME/api/v2/admin"
request="$(jq -nc --arg key "$release" '{trigger:"pre_deploy", idempotency_key:$key}')"
note "enqueueing complete pre-deploy backup for $release"
response="$(curl --fail --silent --show-error \
  --request POST \
  --header "Authorization: Bearer $SBOL_DB_ADMIN_TOKEN" \
  --header 'Content-Type: application/json' \
  --data "$request" \
  "$base_url/backup")"
job_id="$(jq -r '.job.id // empty' <<<"$response")"
[[ -n "$job_id" ]] || die "backup response did not contain a job id"

wait_seconds="${SBOL_DB_BACKUP_WAIT_SECS:-7200}"
poll_seconds="${SBOL_DB_BACKUP_POLL_SECS:-10}"
deadline=$((SECONDS + wait_seconds))
while ((SECONDS < deadline)); do
  job="$(curl --fail --silent --show-error \
    --header "Authorization: Bearer $SBOL_DB_ADMIN_TOKEN" \
    "$base_url/jobs/$job_id")"
  status="$(jq -r .status <<<"$job")"
  case "$status" in
    succeeded)
      remote_object_key="$(jq -r '.result.remote.object_key // empty' <<<"$job")"
      artifact_sha256="$(jq -r '.result.artifact_sha256 // empty' <<<"$job")"
      [[ -n "$remote_object_key" ]] || die "backup succeeded without a remotely verified object key"
      [[ "$artifact_sha256" =~ ^[0-9a-f]{64}$ ]] || die "backup succeeded without a valid artifact SHA-256"
      temporary="$(mktemp "$FLY_STATE_DIR/predeploy-backup.json.XXXXXX")"
      jq -n \
        --arg release "$release" \
        --arg job_id "$job_id" \
        --arg remote_object_key "$remote_object_key" \
        --arg artifact_sha256 "$artifact_sha256" \
        '{release:$release, job_id:$job_id, status:"succeeded", remote_object_key:$remote_object_key, artifact_sha256:$artifact_sha256}' \
        >"$temporary"
      mv "$temporary" "$FLY_STATE_DIR/predeploy-backup.json"
      chmod 600 "$FLY_STATE_DIR/predeploy-backup.json"
      note "backup $job_id is remotely verified at $remote_object_key"
      exit 0
      ;;
    failed | cancelled | dead)
      jq '{id, status, error, attempts, max_attempts}' <<<"$job" >&2
      die "pre-deploy backup job $job_id ended with status $status"
      ;;
    queued | running)
      sleep "$poll_seconds"
      ;;
    *) die "unknown backup job status: $status" ;;
  esac
done

die "timed out after ${wait_seconds}s waiting for backup job $job_id"
