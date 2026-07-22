#!/usr/bin/env bash
#
# Run the real SynBioHub UI (sbh3frontend) against sbol-db as the complete
# backend, in one command.
#
#   ./run.sh up            # build sbol-db, start it + the UI, print URLs
#   ./run.sh up --fresh    # start from an empty instance (first-launch wizard)
#   ./run.sh up --seed P   # start from a copy of the SQLite store at path P
#   ./run.sh down          # stop the UI and sbol-db
#   ./run.sh logs          # tail sbol-db and UI logs
#   ./run.sh status        # what is running
#
# sbol-db runs as a host process (rebuilds are fast); the frontend runs in
# Docker via docker-compose.yml. State lives in ./data (gitignored):
# the SQLite store persists across restarts, so `up` resumes where you left off.
set -euo pipefail

cd "$(dirname "$0")"
REPO_ROOT="$(cd ../.. && pwd)"
DATA_DIR="$PWD/data"
DB_PATH="$DATA_DIR/sbol-db-ui.sqlite"
PID_FILE="$DATA_DIR/sbol-db.pid"
LOG_FILE="$DATA_DIR/sbol-db.log"

export SBOLDB_PORT="${SBOLDB_PORT:-18903}"
export UI_PORT="${UI_PORT:-3333}"

info()  { printf '\033[0;36m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[0;33m!! \033[0m %s\n' "$*"; }
die()   { printf '\033[0;31mxx \033[0m %s\n' "$*" >&2; exit 1; }

sbol_db_running() {
  [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

stop_sbol_db() {
  if sbol_db_running; then
    info "stopping sbol-db (pid $(cat "$PID_FILE"))"
    kill "$(cat "$PID_FILE")" 2>/dev/null || true
  fi
  rm -f "$PID_FILE"
}

cmd_up() {
  local fresh=0 seed=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --fresh) fresh=1 ;;
      --seed)  shift; seed="${1:-}"; [ -n "$seed" ] || die "--seed needs a path" ;;
      *) die "unknown option: $1" ;;
    esac
    shift
  done

  command -v docker >/dev/null || die "docker is required"
  mkdir -p "$DATA_DIR"

  # Don't collide with a UI already bound to the port (e.g. an old manual run).
  if [ -z "$(docker compose ps -q ui 2>/dev/null)" ] && lsof -iTCP:"$UI_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    die "port $UI_PORT is already in use by another process; free it or set UI_PORT"
  fi

  info "building sbol-db"
  ( cd "$REPO_ROOT" && SBOL_DB_SKIP_UI_BUILD=1 cargo build -p sbol-db ) \
    || die "cargo build failed"
  local bin="$REPO_ROOT/target/debug/sbol-db"

  # Prepare the SQLite store: a seed copy, a fresh migrated store, or reuse.
  if [ -n "$seed" ]; then
    [ -f "$seed" ] || die "seed not found: $seed"
    stop_sbol_db
    info "seeding store from $seed"
    cp "$seed" "$DB_PATH"
  elif [ "$fresh" = 1 ] || [ ! -f "$DB_PATH" ]; then
    stop_sbol_db
    [ "$fresh" = 1 ] && rm -f "$DB_PATH"
    info "creating and migrating a fresh store"
    DATABASE_URL="sqlite://$DB_PATH?mode=rwc" "$bin" db migrate >/dev/null
  fi

  # Start sbol-db bound to all interfaces so the UI container can reach it.
  stop_sbol_db
  info "starting sbol-db on 0.0.0.0:$SBOLDB_PORT"
  DATABASE_URL="sqlite://$DB_PATH?mode=rwc" SBOL_DB_ALLOW_PUBLIC_SIGNUP=true \
    "$bin" server --bind "0.0.0.0:$SBOLDB_PORT" >"$LOG_FILE" 2>&1 &
  echo $! >"$PID_FILE"

  info "waiting for sbol-db"
  for _ in $(seq 1 30); do
    curl -sf -m 2 "http://localhost:$SBOLDB_PORT/admin/theme" >/dev/null 2>&1 && break
    sbol_db_running || die "sbol-db exited early; see $LOG_FILE"
    sleep 1
  done

  # Recreate the frontend so its Next.js SSR cache reflects the current backend.
  info "starting the SynBioHub UI"
  docker compose up -d --force-recreate >/dev/null
  for _ in $(seq 1 40); do
    curl -sf -m 2 "http://localhost:$UI_PORT/" >/dev/null 2>&1 && break
    sleep 1
  done

  local first_launch
  first_launch=$(curl -s -m 5 "http://localhost:$SBOLDB_PORT/admin/theme" \
    | grep -o '"firstLaunch":[a-z]*' | cut -d: -f2 || true)

  echo
  info "SynBioHub UI:  http://localhost:$UI_PORT"
  info "sbol-db API:   http://localhost:$SBOLDB_PORT"
  info "store:         $DB_PATH"
  if [ "$first_launch" = "true" ]; then
    info "This is a fresh instance: open the UI and complete the setup wizard."
  else
    info "Instance is provisioned. If a page misbehaves after switching stores,"
    info "clear site data for localhost:$UI_PORT (stale localStorage) and reload."
  fi
}

cmd_down() {
  info "stopping the UI"
  docker compose down 2>/dev/null || true
  stop_sbol_db
  info "stopped"
}

cmd_logs() {
  sbol_db_running && info "sbol-db log: $LOG_FILE"
  ( tail -n 40 "$LOG_FILE" 2>/dev/null || true )
  echo
  docker compose logs --tail 40 2>/dev/null || true
}

cmd_status() {
  if sbol_db_running; then info "sbol-db: running (pid $(cat "$PID_FILE"), port $SBOLDB_PORT)"
  else warn "sbol-db: not running"; fi
  docker compose ps 2>/dev/null || true
}

case "${1:-}" in
  up)     shift; cmd_up "$@" ;;
  down)   cmd_down ;;
  logs)   cmd_logs ;;
  status) cmd_status ;;
  *) echo "usage: $0 {up [--fresh|--seed PATH] | down | logs | status}" >&2; exit 2 ;;
esac
