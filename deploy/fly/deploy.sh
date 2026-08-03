#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command fly
require_command jq

image="${1:-${FLY_IMAGE:-}}"
[[ -n "$image" ]] || die "usage: $0 <immutable-image-reference> (or set FLY_IMAGE)"
[[ "$image" != *:latest ]] || die "refusing to deploy a latest tag"

"$SCRIPT_DIR/render.sh"
fly_cmd config validate --config "$FLY_TOML"
app_exists || die "Fly app $FLY_APP does not exist; run bootstrap.sh first"

machine_count="$(fly_cmd machine list --app "$FLY_APP" --json | jq 'length')"
backup_gate="$FLY_STATE_DIR/predeploy-backup.json"
if ((machine_count > 0)) && [[ "${SBOL_DB_SKIP_PREDEPLOY_BACKUP:-0}" != "1" ]]; then
  if [[ ! -f "$backup_gate" ]] || ! jq -e --arg release "$image" \
    '.release == $release and .status == "succeeded" and (.remote_object_key | length > 0) and (.artifact_sha256 | length == 64)' \
    "$backup_gate" >/dev/null; then
    die "an existing Machine requires a verified pre-deploy backup for this exact image; run predeploy-backup.sh first"
  fi
fi

note "deploying $image to $FLY_APP as exactly one volume-owning Machine"
fly_cmd deploy \
  --config "$FLY_TOML" \
  --image "$image" \
  --ha=false \
  --strategy rolling \
  --wait-timeout 15m
