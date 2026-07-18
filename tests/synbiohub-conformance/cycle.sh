#!/usr/bin/env bash
# One-command, reproducible full-corpus differential run.
#
# Brings up the classic SynBioHub reference with its full search stack
# (Virtuoso + Elasticsearch + SBOLExplorer, useSBOLExplorer=true), applies the
# container-local SBOLExplorer patches the corpus needs (patch_explorer.py:
# drop vsearch's redundant `-sort length`, skip empty/gapped/non-nucleotide
# sequences, and resolve real sbolType/type in the empty-/search projection),
# seeds the full SBOL2 test corpus into the reference and the subject
# identically, rebuilds the subject's native search index on its embedded
# worker, then runs the byte-equal V1 differential (every endpoint except
# /similar and /similarCount, which are characterized in
# docs/similar-explorer-gap.md and excluded here).
#
# Usage: cycle.sh [run|seedonly|refonly|subjectonly]
#   run         full cycle: reference + subject + seed + reindex + differential
#   refonly     bring the reference up, configure it, seed it (no subject/run)
#   subjectonly reset+seed the subject and reindex against a warm reference
#   seedonly    (re)seed both without resetting containers, then run
#
# Environment:
#   FRESH=1        tear the reference down (`down -v`) and reseed from scratch
#   SBOL2_CORPUS   directory of SBOL2 .xml files (default: the SBOLTestSuite)
#   DETAIL         per-case diff detail length printed on failure (default 900)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PY="$HERE/.venv/bin/python"
DB="/tmp/sbol-db-subject.sqlite"
SUBJECT="http://127.0.0.1:18903"
REFERENCE="http://localhost:17777"
ADMIN_EMAIL="test@user.synbiohub"
ADMIN_PW="test"
MODE="${1:-run}"

# The full SBOL2 test corpus (multi-object real files) plus the repo's fixture
# anchor (smoke.xml) that the object-scoped cases key on. Both are staged into
# one directory so seed_both.py submits a single identical corpus to each side.
SBOL2_CORPUS="${SBOL2_CORPUS:-/Users/marpaia/git/SynBioHub/synbiohub/tests/SBOLTestRunner/src/main/resources/SBOLTestSuite/SBOL2}"
FIXTURE_CORPUS="$HERE/fixtures/corpus"
CORPUS="/tmp/conformance-corpus-full"

assemble_corpus() {
  rm -rf "$CORPUS"
  mkdir -p "$CORPUS"
  cp "$SBOL2_CORPUS"/*.xml "$CORPUS"/
  cp "$FIXTURE_CORPUS"/*.xml "$CORPUS"/
  echo "corpus: $(ls "$CORPUS"/*.xml | wc -l | tr -d ' ') files staged in $CORPUS"
}

reset_reference() {
  ( cd "$HERE"
    if [ "${FRESH:-0}" = "1" ]; then
      docker compose down -v >/dev/null 2>&1 || true
    fi
    docker compose up -d autoheal virtuoso elasticsearch synbiohub explorer >/dev/null 2>&1 )
  # A fresh instance serves the /setup onboarding page (200); an already
  # onboarded one 404s /setup and serves / (200). Treat either as ready.
  echo "waiting for reference to answer..."
  for _ in $(seq 1 60); do
    root=$(curl -s -o /dev/null -w "%{http_code}" "$REFERENCE/" 2>/dev/null || true)
    setup=$(curl -s -o /dev/null -w "%{http_code}" "$REFERENCE/setup" 2>/dev/null || true)
    { [ "$root" = "200" ] || [ "$setup" = "200" ]; } && break
    sleep 5
  done
  echo "waiting for elasticsearch..."
  for _ in $(seq 1 60); do
    code=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:9200/_cluster/health" 2>/dev/null || true)
    [ "$code" = "200" ] && break
    sleep 5
  done
}

# Configure the reference to be the correct oracle: SBOLExplorer enabled, and
# SBOLExplorer's known corpus-indexing defects patched in place. Both steps are
# idempotent so a warm reference is left untouched.
configure_reference() {
  ( cd "$HERE"
    docker compose exec -T synbiohub \
      sed -i 's/"useSBOLExplorer": false,/"useSBOLExplorer": true,/' /synbiohub/config.json || true
    docker compose cp reference-patches/patch_explorer.py explorer:/tmp/patch_explorer.py >/dev/null
    if docker compose exec -T explorer python3 /tmp/patch_explorer.py | grep -q "applied"; then
      echo "explorer patches applied; restarting explorer + synbiohub"
      docker compose restart explorer synbiohub >/dev/null 2>&1
      for _ in $(seq 1 40); do
        code=$(curl -s -o /dev/null -w "%{http_code}" "$REFERENCE/" 2>/dev/null || true)
        [ "$code" = "200" ] && break
        sleep 3
      done
    else
      echo "explorer already patched"
    fi )
}

reset_subject() {
  lsof -ti tcp:18903 | xargs kill -9 2>/dev/null || true
  rm -f "$DB" "$DB"-* 2>/dev/null || true
  DATABASE_URL="sqlite://$DB?mode=rwc" "$ROOT/target/debug/sbol-db" db migrate >/dev/null
  DATABASE_URL="sqlite://$DB?mode=rwc" SBOL_DB_ALLOW_PUBLIC_SIGNUP=true \
    nohup "$ROOT/target/debug/sbol-db" server --bind 127.0.0.1:18903 >/tmp/sbol-db-subject.log 2>&1 &
  sleep 3
}

# True when the reference already holds the full corpus (so its seed can be
# skipped: re-submitting collides harmlessly but wastes minutes on libSBOLj).
reference_is_seeded() {
  local n
  n=$(curl -s "$REFERENCE/rootCollections" -H "Accept: application/json" 2>/dev/null \
    | "$PY" -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
  [ "${n:-0}" -ge 100 ]
}

seed() {
  local skip_ref=""
  if reference_is_seeded && [ "${FRESH:-0}" != "1" ]; then
    echo "reference already seeded; seeding subject only"
    skip_ref="--skip-reference"
  fi
  "$PY" "$HERE/seed_both.py" --corpus "$CORPUS" --subject-db "$DB" $skip_ref
}

# Rebuild the subject's native ranked search index on its embedded worker and
# block until the job completes, so /search and the ranked query surface reflect
# the seeded corpus before the differential runs.
reindex_subject() {
  local token job status
  token=$(curl -s -X POST "$SUBJECT/login" -H "Accept: text/plain" \
    --data-urlencode "email=$ADMIN_EMAIL" --data-urlencode "password=$ADMIN_PW")
  job=$(curl -s -X POST "$SUBJECT/admin/reindex" -H "X-authorization: $token" \
    -H "Accept: application/json" | "$PY" -c "import sys,json; print(json.load(sys.stdin)['jobId'])")
  echo "subject reindex job $job enqueued; waiting..."
  for _ in $(seq 1 120); do
    status=$(DATABASE_URL="sqlite://$DB?mode=rwc" "$ROOT/target/debug/sbol-db" jobs status "$job" 2>/dev/null \
      | "$PY" -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
    case "$status" in
      succeeded) echo "reindex succeeded"; return 0 ;;
      failed|dead|cancelled) echo "reindex $status"; return 1 ;;
    esac
    sleep 5
  done
  echo "reindex did not complete in time"; return 1
}

assemble_corpus

case "$MODE" in
  refonly)
    reset_reference; configure_reference; seed ;;
  subjectonly)
    reset_subject; seed; reindex_subject ;;
  seedonly)
    seed; reindex_subject
    "$PY" "$HERE/run53.py" "${DETAIL:-900}" ;;
  run)
    reset_reference; configure_reference; reset_subject; seed; reindex_subject
    "$PY" "$HERE/run53.py" "${DETAIL:-900}" ;;
  *)
    echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
