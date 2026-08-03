#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_command fly
require_command jq
require_vars FLY_APP FLY_ORG

app_only=false
if [[ "${1:-}" == "--app-only" ]]; then
  app_only=true
elif [[ $# -ne 0 ]]; then
  die "usage: $0 [--app-only]"
fi

if app_exists; then
  note "Fly app already exists: $FLY_APP"
else
  note "creating Fly app $FLY_APP in organization $FLY_ORG"
  fly_cmd apps create "$FLY_APP" --org "$FLY_ORG" --yes
fi

if [[ "$app_only" == true ]]; then
  exit 0
fi

require_full_config
"$SCRIPT_DIR/render.sh"
fly_cmd config validate --config "$FLY_TOML"

ips_json="$(fly_cmd ips list --app "$FLY_APP" --json)"
if ! jq -e 'any(.[]?; (.Address // .address // "") | contains("."))' <<<"$ips_json" >/dev/null; then
  note "allocating dedicated IPv4 address"
  fly_cmd ips allocate-v4 --app "$FLY_APP" --yes
fi
if ! jq -e 'any(.[]?; (.Address // .address // "") | contains(":"))' <<<"$ips_json" >/dev/null; then
  note "allocating IPv6 address"
  fly_cmd ips allocate-v6 --app "$FLY_APP"
fi

volumes_json="$(fly_cmd volumes list --app "$FLY_APP" --json)"
volume_count="$(jq --arg name "$FLY_VOLUME_NAME" '[.[]? | select((.Name // .name) == $name and (.State // .state) != "destroyed")] | length' <<<"$volumes_json")"
if [[ "$volume_count" == "0" ]]; then
  note "creating ${FLY_VOLUME_SIZE_GB}GB encrypted volume $FLY_VOLUME_NAME in $FLY_PRIMARY_REGION"
  fly_cmd volumes create "$FLY_VOLUME_NAME" \
    --app "$FLY_APP" \
    --region "$FLY_PRIMARY_REGION" \
    --size "$FLY_VOLUME_SIZE_GB" \
    --snapshot-retention "$FLY_VOLUME_SNAPSHOT_RETENTION_DAYS" \
    --scheduled-snapshots \
    --vm-size "$FLY_VM_SIZE" \
    --yes
elif [[ "$volume_count" != "1" ]]; then
  die "expected at most one active $FLY_VOLUME_NAME volume, found $volume_count"
else
  note "Fly volume already exists: $FLY_VOLUME_NAME"
fi

if fly_cmd storage list --org "$FLY_ORG" | awk -v bucket="$SBOL_DB_BACKUP_BUCKET" '$1 == bucket { found = 1 } END { exit !found }'; then
  note "Tigris bucket already exists: $SBOL_DB_BACKUP_BUCKET"
else
  note "creating private Tigris bucket $SBOL_DB_BACKUP_BUCKET and attaching its credentials"
  fly_cmd storage create \
    --app "$FLY_APP" \
    --org "$FLY_ORG" \
    --name "$SBOL_DB_BACKUP_BUCKET" \
    --yes >/dev/null
fi

if ! fly_cmd secrets list --app "$FLY_APP" --json | jq -e 'any(.[]?; (.Name // .name) == "SBOL_DB_SETUP_TOKEN")' >/dev/null; then
  require_command openssl
  setup_token="$(openssl rand -hex 32)"
  note "creating first-launch setup token as a staged Fly secret"
  printf 'SBOL_DB_SETUP_TOKEN=%s\n' "$setup_token" | \
    fly_cmd secrets import --app "$FLY_APP" --stage >/dev/null
  unset setup_token
else
  note "first-launch setup token is already staged"
fi

note "foundation ready; configure DNS to the following raw-TCP addresses before deploy"
fly_cmd ips list --app "$FLY_APP"
