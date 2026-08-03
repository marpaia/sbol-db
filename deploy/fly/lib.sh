#!/usr/bin/env bash

set -euo pipefail

FLY_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$FLY_SCRIPT_DIR/../.." && pwd)"
FLY_STATE_DIR="${SBOL_DB_FLY_STATE_DIR:-$FLY_SCRIPT_DIR/.state}"
FLY_CONFIG_ENV="${SBOL_DB_FLY_CONFIG:-$FLY_SCRIPT_DIR/config.env}"
FLY_TOML="${SBOL_DB_FLY_TOML:-$FLY_STATE_DIR/fly.toml}"
export FLY_TOML

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

note() {
  printf '==> %s\n' "$*"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

load_public_config() {
  if [[ -f "$FLY_CONFIG_ENV" ]]; then
    # config.env is deliberately public operator configuration. Secret values
    # belong in the process environment, the root .env, or Fly/GitHub secrets.
    set -a
    # shellcheck disable=SC1090
    source "$FLY_CONFIG_ENV"
    set +a
  fi

  : "${FLY_ORG:=sbol}"
  : "${FLY_VOLUME_NAME:=sbol_db_data}"
  : "${FLY_VOLUME_SIZE_GB:=100}"
  : "${FLY_VOLUME_SNAPSHOT_RETENTION_DAYS:=14}"
  : "${FLY_VOLUME_AUTO_EXTEND_THRESHOLD:=70}"
  : "${FLY_VOLUME_AUTO_EXTEND_INCREMENT_GB:=25}"
  : "${FLY_VOLUME_AUTO_EXTEND_LIMIT_GB:=500}"
  : "${FLY_VOLUME_INIT_IMAGE:=docker.io/library/alpine:3.22@sha256:14358309a308569c32bdc37e2e0e9694be33a9d99e68afb0f5ff33cc1f695dce}"
  : "${FLY_VM_SIZE:=performance-4x}"
  : "${FLY_VM_MEMORY:=16GB}"
  : "${SBOL_DB_MIN_FREE_BYTES:=10737418240}"
  : "${SBOL_DB_BACKUP_INTERVAL_SECS:=21600}"
  : "${SBOL_DB_BACKUP_PREFIX:=registry/production}"
  : "${SBOL_DB_LOCAL_DATA_DIR:=$REPO_ROOT/.sbol-db}"
  : "${SBOL_DB_RECOVERY_IDENTITY_FILE:=$FLY_STATE_DIR/recovery.agekey}"
  : "${SBOL_DB_SEED_BACKUP_DIR:=$FLY_STATE_DIR/seed-artifacts}"
  : "${SBOL_DB_SEED_JSON:=$FLY_STATE_DIR/seed.json}"
}

dotenv_value() {
  local key="$1"
  local file="$2"
  awk -v wanted="$key" '
    {
      line = $0
      sub(/^[[:space:]]*export[[:space:]]+/, "", line)
      equals = index(line, "=")
      if (equals == 0) next
      name = substr(line, 1, equals - 1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
      if (name != wanted) next
      value = substr(line, equals + 1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/ || value ~ /^\047.*\047$/) {
        value = substr(value, 2, length(value) - 2)
      }
      print value
      exit
    }
  ' "$file"
}

load_fly_token() {
  if [[ -z "${FLY_API_TOKEN:-}" && -f "$REPO_ROOT/.env" ]]; then
    FLY_API_TOKEN="$(dotenv_value FLY_API_TOKEN "$REPO_ROOT/.env")"
  fi
  [[ -n "${FLY_API_TOKEN:-}" ]] || die \
    "FLY_API_TOKEN is not set and was not found in $REPO_ROOT/.env"
  export FLY_API_TOKEN
}

prepare_state_dir() {
  mkdir -p "$FLY_STATE_DIR"
  chmod 700 "$FLY_STATE_DIR"
  export FLY_CONFIG_DIR="$FLY_STATE_DIR/flyctl"
  mkdir -p "$FLY_CONFIG_DIR"
  chmod 700 "$FLY_CONFIG_DIR"
}

fly_cmd() {
  load_fly_token
  prepare_state_dir
  FLY_API_TOKEN="$FLY_API_TOKEN" FLY_CONFIG_DIR="$FLY_CONFIG_DIR" fly "$@"
}

wait_for_machine_state() {
  local app="$1"
  local machine_id="$2"
  local desired_state="$3"
  local timeout_seconds="$4"
  local poll_seconds="${5:-5}"
  local deadline=$((SECONDS + timeout_seconds))
  local state

  while ((SECONDS < deadline)); do
    state="$(fly_cmd machine list --app "$app" --json | jq -r --arg id "$machine_id" '[.[] | select((.id // .ID) == $id)][0] | (.state // .State) // empty')"
    if [[ -z "$state" ]]; then
      printf 'error: Machine %s disappeared while waiting for state %s\n' \
        "$machine_id" "$desired_state" >&2
      return 1
    fi
    if [[ "$state" == "$desired_state" ]]; then
      return 0
    fi
    sleep "$poll_seconds"
  done
  printf 'error: Machine %s did not reach state %s within %ss (last state: %s)\n' \
    "$machine_id" "$desired_state" "$timeout_seconds" "$state" >&2
  return 1
}

require_vars() {
  local name
  for name in "$@"; do
    [[ -n "${!name:-}" ]] || die "$name must be set in $FLY_CONFIG_ENV or the environment"
  done
}

require_full_config() {
  require_vars \
    FLY_APP FLY_ORG FLY_PRIMARY_REGION SBOL_DB_HOSTNAME SBOL_DB_ACME_CONTACT \
    SBOL_DB_BACKUP_BUCKET SBOL_DB_BACKUP_RECOVERY_RECIPIENT FLY_VOLUME_NAME \
    FLY_VOLUME_SIZE_GB FLY_VM_SIZE FLY_VM_MEMORY

  [[ "$FLY_APP" =~ ^[a-z0-9][a-z0-9-]*[a-z0-9]$ ]] || die "invalid FLY_APP: $FLY_APP"
  [[ "$FLY_PRIMARY_REGION" =~ ^[a-z]{3}$ ]] || die "invalid FLY_PRIMARY_REGION: $FLY_PRIMARY_REGION"
  [[ "$SBOL_DB_HOSTNAME" =~ ^[A-Za-z0-9.-]+$ ]] || die "invalid SBOL_DB_HOSTNAME"
  [[ "$SBOL_DB_HOSTNAME" != *.fly.dev ]] || die "SBOL_DB_HOSTNAME must be an operator-owned custom hostname"
  [[ "$SBOL_DB_BACKUP_RECOVERY_RECIPIENT" == age1* ]] || die \
    "SBOL_DB_BACKUP_RECOVERY_RECIPIENT must be generated before rendering"
}

app_exists() {
  fly_cmd apps list --json | jq -e --arg app "$FLY_APP" \
    'any(.[]?; .Name == $app or .ID == $app)' >/dev/null
}
