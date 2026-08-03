#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command fly
require_command jq

[[ "${SBOL_DB_VOLUME_INIT_CONFIRM:-}" == "INITIALIZE $FLY_APP" ]] || die \
  "set SBOL_DB_VOLUME_INIT_CONFIRM='INITIALIZE $FLY_APP' to authorize initializing the offline production volume"

machines_json="$(fly_cmd machine list --app "$FLY_APP" --json)"
[[ "$(jq 'length' <<<"$machines_json")" == "0" ]] || die \
  "volume initialization requires zero Machines so the production volume is offline"

volume_id="$(fly_cmd volumes list --app "$FLY_APP" --json | jq -r --arg name "$FLY_VOLUME_NAME" '[.[] | select((.Name // .name) == $name and (.State // .state) == "created")][0] | (.ID // .id) // empty')"
[[ -n "$volume_id" ]] || die "could not resolve the production volume"

initializer_name="volume-init"
initializer_id=""
note "initializing the offline volume for the production UID with a pinned helper image"
if ! fly_cmd machine run \
  --app "$FLY_APP" \
  --region "$FLY_PRIMARY_REGION" \
  --volume "$volume_id:/var/lib/sbol-db" \
  --vm-size shared-cpu-1x \
  --name "$initializer_name" \
  --restart no \
  -- \
  "$FLY_VOLUME_INIT_IMAGE" \
  /bin/sh -ceu \
  'test -d /var/lib/sbol-db
   chown 65532:65532 /var/lib/sbol-db
   chmod 0700 /var/lib/sbol-db
   touch /var/lib/sbol-db/.volume-initialized
   chown 65532:65532 /var/lib/sbol-db/.volume-initialized
   chmod 0600 /var/lib/sbol-db/.volume-initialized'; then
  initializer_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$initializer_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
  die "volume initializer failed to start cleanly; leaving Machine ${initializer_id:-$initializer_name} attached for inspection"
fi

initializer_id="$(fly_cmd machine list --app "$FLY_APP" --json | jq -r --arg name "$initializer_name" '[.[] | select((.name // .Name) == $name)][0] | (.id // .ID) // empty')"
[[ -n "$initializer_id" ]] || die "could not resolve the volume initializer Machine"
note "waiting for volume initializer Machine $initializer_id to exit"
wait_for_machine_state "$FLY_APP" "$initializer_id" stopped 600
initializer_status="$(fly_cmd machine status --app "$FLY_APP" "$initializer_id")"
printf '%s\n' "$initializer_status"
if ! grep -q 'exit_code=0,oom_killed=false' <<<"$initializer_status"; then
  die "volume initialization did not report a clean exit; leaving Machine $initializer_id attached for inspection"
fi

fly_cmd machine destroy --app "$FLY_APP" "$initializer_id"
note "offline volume is ready for the nonroot production image"
