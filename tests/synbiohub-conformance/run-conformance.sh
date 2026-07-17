#!/bin/bash
#
# Differential conformance run: classic SynBioHub (reference) vs sbol-db
# (subject, one per backend). Brings up the compose stack, waits for every
# target to answer POST /sparql, runs the pytest matrix, and writes the
# reference-vs-subject diff report.
#
# Usage:
#   tests/synbiohub-conformance/run-conformance.sh --corpus /path/to/sbol2-rdfxml-dir
#   tests/synbiohub-conformance/run-conformance.sh --corpus ./corpus --down
#
# ENVIRONMENT NOTE: the reference services are amd64 images; on Apple Silicon
# Virtuoso is emulated and Elasticsearch 6.3.2 may OOM. The full live run is
# intended for the amd64 CI runner. The comparison-library and driver unit
# tests (test_compare.py, test_conformance_driver.py) run anywhere with no
# stack.

set -eu

PROJECT=sbhconformance
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE="$HERE/docker-compose.yaml"
RESULTS="$HERE/results"
PYTHON="${PYTHON:-$HERE/.venv/bin/python}"

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

msg() { echo "[conformance] $1"; }
compose() { docker compose -p "$PROJECT" -f "$COMPOSE" "$@"; }

if [[ -f "$HERE/.env" ]]; then
    set -a; . "$HERE/.env"; set +a
fi

if ! "$PYTHON" -c "import requests, rdflib, bs4, pytest" >/dev/null 2>&1; then
    msg "ERROR: $PYTHON is missing harness deps. Create the venv:"
    msg "  python3 -m venv $HERE/.venv && $HERE/.venv/bin/pip install -r $HERE/requirements.txt"
    exit 1
fi

mkdir -p "$RESULTS"

msg "Cleaning any prior conformance stack"
compose down -v --remove-orphans >/dev/null 2>&1 || true

msg "Starting reference (classic SynBioHub) + sbol-db subjects"
compose up -d

OUT="${CONFORMANCE_OUT:-$RESULTS/conformance-$(hostname -s).json}"
export CONFORMANCE_OUT="$OUT"
export CORPUS

msg "Running pytest matrix (report -> $OUT)"
"$PYTHON" -m pytest "$HERE" \
    ${PASSTHROUGH[@]+"${PASSTHROUGH[@]}"}

if [[ "$DOWN" -eq 1 ]]; then
    msg "Tearing down conformance stack"
    compose down -v --remove-orphans
else
    msg "Stack left running (project: $PROJECT). Tear down with:"
    msg "  docker compose -p $PROJECT -f $COMPOSE down -v"
fi
