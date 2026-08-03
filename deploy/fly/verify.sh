#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command curl
require_command jq

expected_image="${1:-${FLY_IMAGE:-}}"
[[ -n "$expected_image" ]] || die "usage: $0 <immutable-image-reference> (or set FLY_IMAGE)"
curl_retry=(--retry 5 --retry-all-errors --retry-delay 2 --connect-timeout 15 --max-time 90)

machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
machine_count="$(jq 'length' <<<"$machines_json")"
[[ "$machine_count" == "1" ]] || die "expected exactly one Machine, found $machine_count"
machine_id="$(jq -r '.[0].id // .[0].ID' <<<"$machines_json")"
[[ "$(jq -r '.[0].state // .[0].State' <<<"$machines_json")" == "started" ]] || die \
  "the sole production Machine is not started"
[[ "$(jq -r '.[0].region // .[0].Region' <<<"$machines_json")" == "$FLY_PRIMARY_REGION" ]] || die \
  "the sole production Machine is not in $FLY_PRIMARY_REGION"
[[ "$(jq -r '.[0].config.image // empty' <<<"$machines_json")" == "$expected_image" ]] || die \
  "the deployed image does not match expected image $expected_image"
[[ "$(jq -r '.[0].config.guest.cpu_kind // empty' <<<"$machines_json")" == "performance" ]] || die \
  "the production Machine is not using performance CPUs"
[[ "$(jq -r '.[0].config.guest.cpus // 0' <<<"$machines_json")" == "4" ]] || die \
  "the production Machine does not have 4 vCPUs"
[[ "$(jq -r '.[0].config.guest.memory_mb // 0' <<<"$machines_json")" == "16384" ]] || die \
  "the production Machine does not have 16GB memory"

volumes_json="$(fly_cmd volumes list --app "$FLY_APP" --json)"
volume_count="$(jq --arg name "$FLY_VOLUME_NAME" '[.[]? | select((.Name // .name) == $name and (.State // .state) == "created")] | length' <<<"$volumes_json")"
[[ "$volume_count" == "1" ]] || die "expected exactly one created $FLY_VOLUME_NAME volume, found $volume_count"
volume_id="$(jq -r --arg name "$FLY_VOLUME_NAME" '.[] | select((.Name // .name) == $name) | (.ID // .id)' <<<"$volumes_json")"
[[ "$(jq -r --arg id "$volume_id" '[.[0].config.mounts[]? | select(.volume == $id and .path == "/var/lib/sbol-db")] | length' <<<"$machines_json")" == "1" ]] || die \
  "the production volume is not mounted at /var/lib/sbol-db"
[[ "$(jq -r --arg name "$FLY_VOLUME_NAME" '.[] | select((.Name // .name) == $name) | (.attached_machine_id // empty)' <<<"$volumes_json")" == "$machine_id" ]] || die \
  "the production volume is not attached to the sole production Machine"
[[ "$(jq -r --arg name "$FLY_VOLUME_NAME" '.[] | select((.Name // .name) == $name) | (.encrypted // false)' <<<"$volumes_json")" == "true" ]] || die \
  "the production volume is not encrypted"

checks_json="$(fly_cmd checks list --app "$FLY_APP" --json)"
[[ "$(jq '[to_entries[].value[]] | length' <<<"$checks_json")" -gt 0 ]] || die \
  "the production Machine has no Fly health checks"
jq -e '[to_entries[].value[]] | all(.status == "passing")' <<<"$checks_json" >/dev/null || die \
  "one or more Fly health checks are not passing"

note "Fly Machine"
jq '[.[] | {id: (.id // .ID), name: (.name // .Name), state: (.state // .State), region: (.region // .Region), image: (.config.image // .image_ref)}]' <<<"$machines_json"
note "Fly service checks"
printf '%s\n' "$checks_json" | jq .
note "public HTTP redirect"
redirect="$(curl "${curl_retry[@]}" --silent --show-error --output /dev/null --write-out '%{http_code} %{redirect_url}' "http://$SBOL_DB_HOSTNAME/")"
[[ "$redirect" == "308 https://$SBOL_DB_HOSTNAME/" ]] || die \
  "unexpected public HTTP redirect: $redirect"
printf '%s\n' "$redirect"
note "public browser portal"
portal_status="$(curl "${curl_retry[@]}" --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --header 'Accept: text/html,application/xhtml+xml' "https://$SBOL_DB_HOSTNAME/")"
[[ "$portal_status" == "200" ]] || die "public browser portal returned HTTP $portal_status"
printf 'HTTP %s\n' "$portal_status"
note "public instance discovery"
curl "${curl_retry[@]}" --fail --silent --show-error "https://$SBOL_DB_HOSTNAME/api/v2/instance" | jq .
