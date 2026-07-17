#!/usr/bin/env bash
# Reset reference + subject to pristine, seed the curated corpus into both, and
# run the full 53-case differential suite once. Usage: cycle.sh [run|seed|refonly]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
CORPUS="${CORPUS:-/tmp/conformance-corpus}"
DB="/tmp/sbol-db-subject.sqlite"
PY="$HERE/.venv/bin/python"
MODE="${1:-run}"

reset_reference() {
  ( cd "$HERE" && docker compose down -v >/dev/null 2>&1 && docker compose up -d virtuoso synbiohub >/dev/null 2>&1 )
  echo "waiting for reference /setup..."
  for i in $(seq 1 60); do
    code=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:17777/setup 2>/dev/null || true)
    [ "$code" = "200" ] && break
    sleep 5
  done
}

reset_subject() {
  lsof -ti tcp:18903 | xargs kill -9 2>/dev/null || true
  rm -f "$DB" "$DB"-* 2>/dev/null || true
  DATABASE_URL="sqlite://$DB?mode=rwc" "$ROOT/target/debug/sbol-db" db migrate >/dev/null
  DATABASE_URL="sqlite://$DB?mode=rwc" SBOL_DB_ALLOW_PUBLIC_SIGNUP=true \
    nohup "$ROOT/target/debug/sbol-db" server --bind 127.0.0.1:18903 --no-worker >/tmp/sbol-db-subject.log 2>&1 &
  sleep 3
}

# sbol-db ships no SBOLExplorer service; its native similarity engine backs the
# V2 API and the SPARQL explorer bypass, and its V1 /similar surface serves
# classic's disabled-backend contract (a 503). Configure the reference to match:
# with useSBOLExplorer=false, classic gates /similar and /similarCount to the
# same 503 for a non-HTML request instead of the degenerate all-objects fallback
# it returns when the flag is on but no explorer is reachable. /setup never
# writes this key, so flipping it after the seed and restarting is stable.
configure_reference_no_explorer() {
  ( cd "$HERE" \
    && docker compose exec -T synbiohub sed -i 's/"useSBOLExplorer": true,/"useSBOLExplorer": false,/' /synbiohub/config.json \
    && docker compose restart synbiohub >/dev/null 2>&1 )
  for _ in $(seq 1 40); do
    code=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:17777/ 2>/dev/null || true)
    [ "$code" = "200" ] && break
    sleep 3
  done
}

if [ "$MODE" != "seedonly" ]; then
  reset_reference
  reset_subject
fi
"$PY" "$HERE/seed_both.py" --corpus "$CORPUS" --subject-db "$DB"
configure_reference_no_explorer
if [ "$MODE" = "run" ] || [ "$MODE" = "seedonly" ]; then
  SCRATCH_ID="${SCRATCH_ID:-scratch$(date +%s)}" "$PY" "$HERE/run53.py" "${DETAIL:-1}"
fi
