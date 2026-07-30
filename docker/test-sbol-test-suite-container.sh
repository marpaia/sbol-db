#!/usr/bin/env bash

# Exercise the final sbol-db image against a pinned, local SBOLTestSuite
# checkout. The source files stay in SBOLTestSuite: this repository stores only
# their provenance and integrity checks in the manifest below.

set -euo pipefail

readonly image="${1:?usage: docker/test-sbol-test-suite-container.sh IMAGE}"
readonly repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly manifest="$repository_root/crates/sbol-db-search-eval/fixtures/sbol-test-suite-integration-v1.json"
readonly suite_root="${SBOL_DB_SBOL_TEST_SUITE_ROOT:?set SBOL_DB_SBOL_TEST_SUITE_ROOT to a pinned SBOLTestSuite checkout}"
readonly expected_commit="0044284331b2f915a6e4b9d50e1cbf3ea2f62dcd"
readonly container_name="sbol-db-test-suite-e2e-$$"
readonly volume_name="sbol-db-test-suite-e2e-$$"
readonly host_port="${SBOL_DB_TEST_SUITE_SMOKE_PORT:-18082}"
readonly base_url="http://127.0.0.1:${host_port}"
readonly public_graph="http://synbiohub.org/public"
readonly rebuild_timeout_seconds="${SBOL_DB_TEST_SUITE_REBUILD_TIMEOUT_SECS:-900}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-test-suite.XXXXXX")"
test_completed=false

cleanup() {
  if [ "$test_completed" != true ]; then
    docker logs "$container_name" 2>/dev/null || true
  fi
  docker rm --force "$container_name" >/dev/null 2>&1 || true
  docker volume rm "$volume_name" >/dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT

for command in curl docker git jq rg; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required for the SBOLTestSuite container test" >&2
    exit 1
  fi
done

if [ ! -d "$suite_root/.git" ]; then
  echo "SBOLTestSuite checkout is not a Git worktree: $suite_root" >&2
  exit 1
fi
if [ "$(git -C "$suite_root" rev-parse HEAD)" != "$expected_commit" ]; then
  echo "SBOLTestSuite checkout must be pinned at $expected_commit" >&2
  exit 1
fi
if ! git -C "$suite_root" diff --quiet || ! git -C "$suite_root" diff --cached --quiet; then
  echo "SBOLTestSuite checkout has tracked modifications; refuse non-reproducible validation" >&2
  exit 1
fi
jq empty "$manifest"

docker volume create "$volume_name" >/dev/null
docker run --rm \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" db migrate >/dev/null

total_imported=0
while IFS=$'\t' read -r relative_path expected_imported expected_failures; do
  source_directory="$suite_root/$relative_path"
  if [ ! -d "$source_directory" ]; then
    echo "SBOLTestSuite source directory is missing: $relative_path" >&2
    exit 1
  fi
  import_output="$(docker run --rm \
    --volume "$volume_name:/var/lib/sbol-db" \
    --volume "$source_directory:/fixtures/corpus:ro" \
    --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
    "$image" graph import /fixtures/corpus --continue-on-error --parallel 1)"
  expected_summary="summary: $expected_imported imported, 0 skipped, $expected_failures failed"
  actual_summary="$(printf '%s\n' "$import_output" | rg '^summary:' | tail -1)"
  if [ "$actual_summary" != "$expected_summary" ]; then
    echo "unexpected TestSuite import summary for $relative_path: $actual_summary" >&2
    exit 1
  fi
  total_imported=$((total_imported + expected_imported))
done < <(jq -r '.import_groups[] | [.path, (.expected_imported_documents | tostring), (.expected_parse_failures | tostring)] | @tsv' "$manifest")

expected_total="$(jq -r '.expected_imported_documents' "$manifest")"
expected_indexed_objects="$(jq -r '.expected_indexed_objects' "$manifest")"
if [ "$total_imported" -ne "$expected_total" ]; then
  echo "manifest TestSuite total is inconsistent: $total_imported != $expected_total" >&2
  exit 1
fi
stored_documents="$(docker run --rm \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" graph list --limit 1000 | jq 'length')"
if [ "$stored_documents" -ne "$expected_total" ]; then
  echo "expected $expected_total imported TestSuite documents, found $stored_documents" >&2
  exit 1
fi

public_directory="$(jq -r '.public_semantic_corpus.directory' "$manifest")"
public_extension="$(jq -r '.public_semantic_corpus.extension' "$manifest")"
expected_public="$(jq -r '.public_semantic_corpus.expected_documents' "$manifest")"
public_imported=0
while IFS= read -r source_path; do
  import_report="$(docker run --rm \
    --volume "$volume_name:/var/lib/sbol-db" \
    --volume "$source_path:/fixtures/document.${public_extension}:ro" \
    --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
    "$image" graph import "/fixtures/document.${public_extension}" \
      --document-iri "$public_graph" \
      --name "SBOLTestSuite SBOL3 canonical corpus")"
  if ! jq -e '.object_count > 0' <<<"$import_report" >/dev/null; then
    echo "canonical public import produced no SBOL objects: $source_path" >&2
    exit 1
  fi
  public_imported=$((public_imported + 1))
done < <(rg --files "$suite_root/$public_directory" -g "*.${public_extension}" | sort)
if [ "$public_imported" -ne "$expected_public" ]; then
  echo "expected $expected_public canonical SBOL3 public documents, imported $public_imported" >&2
  exit 1
fi

docker run --detach --name "$container_name" \
  --publish "127.0.0.1:${host_port}:8080" \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" >/dev/null

for attempt in $(seq 1 60); do
  if curl -fsS "$base_url/healthz" >/dev/null 2>&1; then
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    echo "SBOLTestSuite semantic-search container did not become healthy" >&2
    exit 1
  fi
  sleep 1
done

rebuild=''
for attempt in $(seq 1 "$rebuild_timeout_seconds"); do
  rebuild="$(curl -fsS "$base_url/jobs?kind=rebuild_vector_index&limit=10")"
  if jq -e '.[] | select(.status == "failed" or .status == "dead" or .status == "cancelled")' \
    <<<"$rebuild" >/dev/null; then
    echo "SBOLTestSuite startup rebuild failed: $rebuild" >&2
    exit 1
  fi
  if jq -e \
    --argjson documents "$expected_indexed_objects" \
    '.[] | select(.status == "succeeded" and .result.documents == $documents and .result.embedding_profile == "builtin.sbol-text-bge-small.v1")' \
    <<<"$rebuild" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq "$rebuild_timeout_seconds" ]; then
    echo "SBOLTestSuite startup rebuild did not finish: $rebuild" >&2
    exit 1
  fi
  sleep 1
done

while IFS=$'\t' read -r query expected_uri; do
  response=''
  for attempt in $(seq 1 90); do
    response="$(curl --silent --show-error \
      --header 'content-type: application/json' \
      --data "{\"query\":{\"kind\":\"text\",\"text\":\"$query\"},\"page\":{\"limit\":3}}" \
      "$base_url/api/v2/search" 2>/dev/null || true)"
    if jq -e \
      --arg uri "$expected_uri" \
      '.strategy.id == "builtin.sbol-text-vector.v2" and .items[0].uri == $uri and .items[0].score_kind == "cosine_similarity"' \
      <<<"$response" >/dev/null 2>&1; then
      break
    fi
    if [ "$attempt" -eq 90 ]; then
      echo "SBOLTestSuite query did not rank the expected object first for $query: $response" >&2
      exit 1
    fi
    sleep 1
  done
done < <(jq -r '.cases[] | [.query, .expected_uri] | @tsv' "$manifest")

test_completed=true
echo "SBOLTestSuite integration passed: $expected_total importable documents and $expected_public canonical SBOL3 semantic documents"
