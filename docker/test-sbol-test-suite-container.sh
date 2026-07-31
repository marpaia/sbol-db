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
readonly projection_container_name="${container_name}-projection"
readonly volume_name="sbol-db-test-suite-e2e-$$"
readonly host_port="${SBOL_DB_TEST_SUITE_SMOKE_PORT:-18082}"
readonly base_url="http://127.0.0.1:${host_port}"
readonly public_graph="http://synbiohub.org/public"
readonly public_graph_parameter="http%3A%2F%2Fsynbiohub.org%2Fpublic"
readonly top_level_predicate="http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel"
readonly rebuild_timeout_seconds="${SBOL_DB_TEST_SUITE_REBUILD_TIMEOUT_SECS:-900}"
readonly bge_enabled="${SBOL_DB_TEST_SUITE_BGE_ENABLED:-false}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-test-suite.XXXXXX")"
test_completed=false

case "$bge_enabled" in
  true|false) ;;
  *)
    echo "SBOL_DB_TEST_SUITE_BGE_ENABLED must be true or false" >&2
    exit 2
    ;;
esac

cleanup() {
  if [ "$test_completed" != true ]; then
    docker logs "$container_name" 2>/dev/null || true
    docker logs "$projection_container_name" 2>/dev/null || true
  fi
  docker rm --force "$container_name" >/dev/null 2>&1 || true
  docker rm --force "$projection_container_name" >/dev/null 2>&1 || true
  docker volume rm "$volume_name" >/dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT

for command in cmp curl docker git jq rg sort split wc; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required for the SBOLTestSuite container test" >&2
    exit 1
  fi
done

wait_for_health() {
  local label="$1"
  for attempt in $(seq 1 60); do
    if curl -fsS "$base_url/healthz" >/dev/null 2>&1; then
      return 0
    fi
    if [ "$attempt" -eq 60 ]; then
      echo "$label container did not become healthy" >&2
      return 1
    fi
    sleep 1
  done
}

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

# Build a graph-independent N-Triples projection of the complete imported
# corpus. The canonical SBOL3 imports above deliberately keep the semantic
# vector fixture stable; this additional projection makes every authoritative
# object from all 447 documents visible to the normalized discovery contract in
# this throwaway database.
printf '%s\n' 'CONSTRUCT { ?subject ?predicate ?object } WHERE { ?subject ?predicate ?object }' \
  >"$test_root/full-corpus.rq"
docker run --rm \
  --volume "$volume_name:/var/lib/sbol-db" \
  --volume "$test_root:/fixtures/work" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" query sparql /fixtures/work/full-corpus.rq \
    --format ntriples --timeout-secs 900 --max-rows 2000000 \
  >"$test_root/full-corpus.nt"
if [ ! -s "$test_root/full-corpus.nt" ]; then
  echo "full SBOLTestSuite RDF projection is empty" >&2
  exit 1
fi
split -l 50000 "$test_root/full-corpus.nt" "$test_root/public-corpus-part-"

docker run --detach --name "$projection_container_name" \
  --publish "127.0.0.1:${host_port}:8080" \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" server --bind 0.0.0.0:8080 --no-worker >/dev/null
wait_for_health "SBOLTestSuite discovery projection"

expected_discovery="$(jq -r '.public_discovery_corpus.expected_objects' "$manifest")"
inventory_page_size="$(jq -r '.public_discovery_corpus.page_size' "$manifest")"
expected_iris="$test_root/expected-discovery-iris.txt"
top_level_markers="$test_root/discovery-top-level.nt"
: >"$expected_iris"
: >"$top_level_markers"
cursor=''
while :; do
  if [ -n "$cursor" ]; then
    inventory="$(curl -fsS --get \
      --data-urlencode "limit=$inventory_page_size" \
      --data-urlencode "after=$cursor" \
      "$base_url/objects/list")"
  else
    inventory="$(curl -fsS --get \
      --data-urlencode "limit=$inventory_page_size" \
      "$base_url/objects/list")"
  fi
  if ! jq -e '.objects | type == "array"' <<<"$inventory" >/dev/null; then
    echo "invalid native object inventory response: $inventory" >&2
    exit 1
  fi
  jq -r '.objects[].iri' <<<"$inventory" >>"$expected_iris"
  jq -r --arg predicate "$top_level_predicate" \
    '.objects[].iri | "<" + . + "> <" + $predicate + "> <" + . + "> ."' \
    <<<"$inventory" >>"$top_level_markers"
  cursor="$(jq -r '.next_cursor // empty' <<<"$inventory")"
  if [ -z "$cursor" ]; then
    break
  fi
done

inventory_count="$(wc -l <"$expected_iris" | tr -d ' ')"
inventory_unique="$(LC_ALL=C sort -u "$expected_iris" | wc -l | tr -d ' ')"
if [ "$inventory_count" -ne "$expected_discovery" ] || \
   [ "$inventory_unique" -ne "$expected_discovery" ]; then
  echo "expected $expected_discovery unique discovery objects, found $inventory_count rows / $inventory_unique unique" >&2
  exit 1
fi

for corpus_part in "$test_root"/public-corpus-part-*; do
  write_response="$(curl -fsS \
    --user dba:dba \
    --request POST \
    --header 'content-type: application/n-triples' \
    --data-binary "@$corpus_part" \
    "$base_url/sparql-graph-crud-auth/?graph-uri=$public_graph_parameter")"
  if ! jq -e '.inserted >= 0' <<<"$write_response" >/dev/null; then
    echo "invalid public corpus projection response: $write_response" >&2
    exit 1
  fi
done
marker_response="$(curl -fsS \
  --user dba:dba \
  --request POST \
  --header 'content-type: application/n-triples' \
  --data-binary "@$top_level_markers" \
  "$base_url/sparql-graph-crud-auth/?graph-uri=$public_graph_parameter")"
if ! jq -e --argjson expected "$expected_discovery" \
  '.inserted >= 0 and .inserted <= $expected' \
  <<<"$marker_response" >/dev/null; then
  echo "invalid public top-level marker response: $marker_response" >&2
  exit 1
fi

docker rm --force "$projection_container_name" >/dev/null

# Keep exhaustive discovery fast by default. This explicit topology retains
# the ranked text strategy and its maintenance job without constructing an
# embedding provider or vector index. The release-scale BGE path remains
# available through SBOL_DB_TEST_SUITE_BGE_ENABLED=true.
search_runtime_args=()
if [ "$bge_enabled" = false ]; then
  printf '%s\n' \
    '{' \
    '  "topology": {' \
    '    "default_strategy": "legacy.explorer.v1",' \
    '    "indexes": [],' \
    '    "embedding_strategies": []' \
    '  },' \
    '  "embeddings": [],' \
    '  "vector_backends": []' \
    '}' >"$test_root/text-search.json"
  search_runtime_args=(
    --volume "$test_root/text-search.json:/fixtures/text-search.json:ro"
    --env SBOL_DB_SEARCH_CONFIG=/fixtures/text-search.json
  )
fi

docker run --detach --name "$container_name" \
  --publish "127.0.0.1:${host_port}:8080" \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "${search_runtime_args[@]}" \
  "$image" >/dev/null
wait_for_health "SBOLTestSuite acceptance"

if [ "$bge_enabled" = true ]; then
  rebuild=''
  for attempt in $(seq 1 "$rebuild_timeout_seconds"); do
    rebuild="$(curl -fsS "$base_url/jobs?kind=rebuild_vector_index&limit=10")"
    if jq -e '.[] | select(.status == "failed" or .status == "dead" or .status == "cancelled")' \
      <<<"$rebuild" >/dev/null; then
      echo "SBOLTestSuite startup vector rebuild failed: $rebuild" >&2
      exit 1
    fi
    if jq -e \
      --argjson documents "$expected_indexed_objects" \
      '.[] | select(.status == "succeeded" and .result.documents == $documents and .result.embedding_profile == "builtin.sbol-text-bge-small.v1")' \
      <<<"$rebuild" >/dev/null; then
      break
    fi
    if [ "$attempt" -eq "$rebuild_timeout_seconds" ]; then
      echo "SBOLTestSuite startup vector rebuild did not finish: $rebuild" >&2
      exit 1
    fi
    sleep 1
  done
else
  curl -fsS \
    --header 'content-type: application/json' \
    --data '{"kind":"rebuild_search_index","payload":{}}' \
    "$base_url/jobs" >/dev/null
fi

search_rebuild=''
for attempt in $(seq 1 "$rebuild_timeout_seconds"); do
  search_rebuild="$(curl -fsS "$base_url/jobs?kind=rebuild_search_index&limit=10")"
  if jq -e '.[] | select(.status == "failed" or .status == "dead" or .status == "cancelled")' \
    <<<"$search_rebuild" >/dev/null; then
    echo "SBOLTestSuite text-index rebuild failed: $search_rebuild" >&2
    exit 1
  fi
  if jq -e \
    --argjson documents "$expected_indexed_objects" \
    '.[] | select(.status == "succeeded" and .result.indexed == $documents)' \
    <<<"$search_rebuild" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq "$rebuild_timeout_seconds" ]; then
    echo "SBOLTestSuite text-index rebuild did not finish: $search_rebuild" >&2
    exit 1
  fi
  sleep 1
done

discovered_iris="$test_root/discovered-iris.txt"
: >"$discovered_iris"
offset=0
while [ "$offset" -lt "$expected_discovery" ]; do
  response="$(curl -fsS --get \
    --data-urlencode 'sort=iri' \
    --data-urlencode 'direction=asc' \
    --data-urlencode "offset=$offset" \
    --data-urlencode "limit=$inventory_page_size" \
    "$base_url/api/v2/search")"
  if ! jq -e \
    --argjson total "$expected_discovery" \
    --argjson offset "$offset" \
    --argjson limit "$inventory_page_size" \
    '.total == $total and .offset == $offset and .limit == $limit and (.items | length) > 0' \
    <<<"$response" >/dev/null; then
    echo "invalid normalized discovery page at offset $offset: $response" >&2
    exit 1
  fi
  jq -r '.items[].uri' <<<"$response" >>"$discovered_iris"
  returned="$(jq '.items | length' <<<"$response")"
  offset=$((offset + returned))
done

discovered_count="$(wc -l <"$discovered_iris" | tr -d ' ')"
discovered_unique="$(LC_ALL=C sort -u "$discovered_iris" | wc -l | tr -d ' ')"
LC_ALL=C sort -u "$expected_iris" >"$test_root/expected-discovery-sorted.txt"
LC_ALL=C sort -u "$discovered_iris" >"$test_root/discovered-sorted.txt"
if [ "$discovered_count" -ne "$expected_discovery" ] || \
   [ "$discovered_unique" -ne "$expected_discovery" ] || \
   ! cmp -s "$test_root/expected-discovery-sorted.txt" "$test_root/discovered-sorted.txt"; then
  echo "normalized discovery did not return the exact $expected_discovery-object corpus ($discovered_count rows / $discovered_unique unique)" >&2
  exit 1
fi

facets="$(curl -fsS "$base_url/api/v2/search/facets")"
for facet_kind in types roles; do
  while IFS=$'\t' read -r facet_iri facet_count; do
    [ -n "$facet_iri" ] || continue
    facet_parameter="${facet_kind%s}"
    facet_response="$(curl -fsS --get \
      --data-urlencode "$facet_parameter=$facet_iri" \
      --data-urlencode 'sort=iri' \
      --data-urlencode 'limit=1' \
      "$base_url/api/v2/search")"
    if ! jq -e --argjson expected "$facet_count" '.total == $expected' \
      <<<"$facet_response" >/dev/null; then
      echo "$facet_kind facet count disagrees with normalized discovery for $facet_iri: $facet_response" >&2
      exit 1
    fi
  done < <(jq -r --arg kind "$facet_kind" '.[$kind][0:5][] | [.iri, .count] | @tsv' <<<"$facets")
done

if [ "$bge_enabled" = true ]; then
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
fi

test_completed=true
if [ "$bge_enabled" = true ]; then
  echo "SBOLTestSuite integration passed: $expected_total importable documents, $expected_discovery discoverable objects, and $expected_public canonical SBOL3 semantic documents with BGE ranking"
else
  echo "SBOLTestSuite integration passed: $expected_total importable documents and $expected_discovery discoverable objects; BGE ranking disabled (set SBOL_DB_TEST_SUITE_BGE_ENABLED=true to include it)"
fi
