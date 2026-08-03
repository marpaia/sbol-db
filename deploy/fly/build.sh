#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=deploy/fly/lib.sh
source "$SCRIPT_DIR/lib.sh"

load_public_config
require_full_config
require_command fly
require_command git
require_command shasum

prepare_state_dir
"$SCRIPT_DIR/render.sh"

source_hash="$({
  git -C "$REPO_ROOT" ls-files --cached --others --exclude-standard -z |
    LC_ALL=C sort -z |
    xargs -0 shasum -a 256
} | shasum -a 256 | awk '{print substr($1, 1, 12)}')"
timestamp="$(date -u +%Y%m%d%H%M%S)"
label="${SBOL_DB_IMAGE_LABEL:-source-${source_hash}-${timestamp}}"
[[ "$label" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$ ]] || die "invalid Docker image label: $label"
image="registry.fly.io/${FLY_APP}:${label}"

note "building the current source and pushing $image"
fly_cmd deploy "$REPO_ROOT" \
  --config "$FLY_TOML" \
  --build-only \
  --push \
  --image-label "$label" \
  --yes

printf '%s\n' "$image" >"$FLY_STATE_DIR/image"
chmod 600 "$FLY_STATE_DIR/image"
note "candidate image ready: $image"
note "set FLY_IMAGE=$image in $FLY_CONFIG_ENV before deploy"
