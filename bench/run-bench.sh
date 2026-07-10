#!/bin/bash
#
# Benchmark Virtuoso against sbol-db on SynBioHub's triplestore workloads.
#
# Brings up the side-by-side bench stack (Virtuoso + sbol-db on Postgres,
# SQLite, and RocksDB), waits for every triplestore to answer, then runs
# bench.py, which loads the same SBOL corpus into each backend and times
# ingest and the realized SynBioHub read queries. Results land in
# results/bench-<host>.json and LaTeX fragments in out/.
#
# Usage:
#   bench/run-bench.sh --corpus /path/to/sbol2-rdfxml-dir
#   bench/run-bench.sh --corpus ./corpus --iterations 100
#   bench/run-bench.sh --corpus ./corpus --down     # tear the stack down after
#
# The corpus is a directory of SBOL2 RDF/XML (`*.xml`) files; see README.md for
# how the captured run's corpus was produced. Requires Docker and a Python 3
# with `requests` (set PYTHON to point at a specific interpreter/venv).

set -eu

PROJECT=sboldbbench
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE="$HERE/docker-compose.yml"
RESULTS="$HERE/results"
PYTHON="${PYTHON:-python3}"

CORPUS=""
DOWN=0
PASSTHROUGH=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --corpus) CORPUS="$2"; shift 2 ;;
        --down) DOWN=1; shift ;;
        *) PASSTHROUGH+=("$1"); shift ;;
    esac
done

msg() { echo "[bench harness] $1"; }
compose() { docker compose -p "$PROJECT" -f "$COMPOSE" "$@"; }

if [[ -z "$CORPUS" || ! -d "$CORPUS" ]]; then
    msg "ERROR: pass --corpus <dir> pointing at a directory of SBOL2 RDF/XML files"
    msg "See bench/README.md for how the captured corpus was produced."
    exit 1
fi

if ! "$PYTHON" -c "import requests" >/dev/null 2>&1; then
    msg "ERROR: $PYTHON cannot import 'requests' (pip install requests, or set PYTHON)"
    exit 1
fi

mkdir -p "$RESULTS"

msg "Cleaning any prior bench stack"
compose down -v --remove-orphans >/dev/null 2>&1 || true

msg "Starting Virtuoso + sbol-db bench stack"
compose up -d

OUT="${BENCH_OUT:-$RESULTS/bench-$(hostname -s).json}"
msg "Running bench.py (results -> $OUT)"
"$PYTHON" -u "$HERE/bench.py" \
    --corpus "$CORPUS" \
    --out "$OUT" \
    ${PASSTHROUGH[@]+"${PASSTHROUGH[@]}"}

if [[ -z "${BENCH_NO_REPORT:-}" ]]; then
    msg "Rendering LaTeX fragments"
    "$PYTHON" -u "$HERE/gen_report.py" "$OUT" --outdir "$HERE/out"
fi

if [[ "$DOWN" -eq 1 ]]; then
    msg "Tearing down bench stack"
    compose down -v --remove-orphans
else
    msg "Stack left running (project: $PROJECT). Tear down with:"
    msg "  docker compose -p $PROJECT -f $COMPOSE down -v"
fi
