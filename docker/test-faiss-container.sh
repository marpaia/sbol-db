#!/usr/bin/env bash

set -euo pipefail

readonly image="${1:?usage: docker/test-faiss-container.sh IMAGE}"
readonly repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly corpus="$repository_root/crates/sbol-db-postgres/tests/fixtures/simple_component.ttl"
readonly container_name="sbol-db-faiss-e2e-$$"
readonly volume_name="sbol-db-faiss-e2e-$$"
readonly host_port="${SBOL_DB_FAISS_SMOKE_PORT:-18080}"
readonly base_url="http://127.0.0.1:${host_port}"
readonly public_graph="http://synbiohub.org/public"
readonly expected_uri="https://example.org/sbol-db/test/promoter_j23119"
readonly model_cache="${SBOL_DB_TEST_MODEL_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/sbol-db/models/all-MiniLM-L6-v2-onnx-5f1b8cd}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-faiss-container.XXXXXX")"
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

for command in curl docker jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required for the FAISS container test" >&2
    exit 1
  fi
done

# This probes the final distroless image, not the builder. The model revision
# command also gives operators a reproducible value for their search config.
docker run --rm "$image" --version >/dev/null
"$repository_root/docker/fetch-test-embedding-model.sh" "$model_cache"
model_revision="$({
  docker run --rm \
    --volume "$model_cache:/opt/sbol-db/models/minilm:ro" \
    "$image" util fastembed-revision /opt/sbol-db/models/minilm
} | jq -er '.revision')"

jq -n --arg revision "$model_revision" '{
  topology: {
    default_strategy: "legacy.explorer.v1",
    indexes: [{
      index: "components",
      backend: "faiss-local",
      embedding_profile: "local.minilm.container.v1",
      vector_name: "content",
      graph_payload_field: "graph"
    }],
    embedding_strategies: [{
      id: "semantic.components.v1",
      version: "1",
      display_name: "Semantic components",
      description: "Container semantic-search lifecycle test",
      embedding_profile: "local.minilm.container.v1",
      vector_index: "components",
      vector_name: "content",
      graph_payload_field: "graph",
      distance: "cosine"
    }]
  },
  embeddings: [{
    kind: "fastembed_local",
    profile: {
      id: "local.minilm.container.v1",
      model: "Qdrant/all-MiniLM-L6-v2-onnx",
      revision: $revision,
      dimension: 384,
      normalization: "l2",
      batch_size: 16
    },
    bundle: {
      directory: "/opt/sbol-db/models/minilm",
      onnx_file: "model.onnx",
      pooling: "mean",
      max_length: 512,
      intra_threads: 2
    }
  }],
  vector_backends: [{
    kind: "faiss",
    config: {
      id: "faiss-local",
      path: "/var/lib/sbol-db/faiss",
      default_nlist: 16,
      default_nprobe: 4,
      flat_search_cutoff: 64,
      max_query_k: 1000
    }
  }]
}' > "$test_root/search.json"

docker volume create "$volume_name" >/dev/null
docker run --rm \
  --volume "$volume_name:/var/lib/sbol-db" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" db migrate >/dev/null
docker run --rm \
  --volume "$volume_name:/var/lib/sbol-db" \
  --volume "$corpus:/fixtures/simple-component.ttl:ro" \
  --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
  "$image" graph import /fixtures/simple-component.ttl \
    --document-iri "$public_graph" \
    --name "FAISS container semantic fixture" >/dev/null

start_container() {
  docker run --detach --name "$container_name" \
    --publish "127.0.0.1:${host_port}:8080" \
    --volume "$volume_name:/var/lib/sbol-db" \
    --volume "$model_cache:/opt/sbol-db/models/minilm:ro" \
    --volume "$test_root/search.json:/etc/sbol-db/search.json:ro" \
    --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
    --env SBOL_DB_SEARCH_CONFIG=/etc/sbol-db/search.json \
    "$image" >/dev/null
}

wait_for_health() {
  for attempt in $(seq 1 60); do
    if curl -fsS "$base_url/healthz" >/dev/null 2>&1; then
      return
    fi
    if [ "$attempt" -eq 60 ]; then
      echo "FAISS-enabled semantic-search container did not become healthy" >&2
      exit 1
    fi
    sleep 1
  done
}

query_semantic_index() {
  curl -fsS \
    --header 'content-type: application/json' \
    --data '{
      "strategy": "semantic.components.v1",
      "query": {"kind": "text", "text": "strong constitutive promoter"},
      "page": {"limit": 5},
      "options": {"explain": true}
    }' \
    "$base_url/api/v2/search"
}

start_container
wait_for_health

docker logs "$container_name" 2>&1 | grep -q "search plugin deployment configured"

strategies="$(curl -fsS "$base_url/api/v2/search/strategies")"
if ! jq -e '.items | any(.id == "semantic.components.v1")' <<<"$strategies" >/dev/null; then
  echo "semantic strategy was not registered: $strategies" >&2
  exit 1
fi

job_response="$(curl -fsS \
  --header 'content-type: application/json' \
  --data '{
    "kind": "rebuild_vector_index",
    "payload": {
      "artifact_id": "components",
      "generation": "container-e2e-v1",
      "vector_name": "content",
      "embedding_profile": "local.minilm.container.v1",
      "distance": "cosine",
      "batch_size": 16,
      "backend_parameters": {}
    },
    "max_attempts": 1
  }' \
  "$base_url/jobs")"
job_id="$(jq -er '.job.id' <<<"$job_response")"

for attempt in $(seq 1 90); do
  job="$(curl -fsS "$base_url/jobs/$job_id")"
  status="$(jq -er '.status' <<<"$job")"
  case "$status" in
    succeeded) break ;;
    failed|dead|cancelled)
      echo "vector rebuild ended in status $status: $job" >&2
      exit 1
      ;;
  esac
  if [ "$attempt" -eq 90 ]; then
    echo "vector rebuild did not finish: $job" >&2
    exit 1
  fi
  sleep 1
done

if ! jq -e --arg revision "$model_revision" '
  .result.documents >= 1
  and .result.backend_id == "faiss-local"
  and .result.embedding_revision == $revision
  and .result.generation == "container-e2e-v1"
' <<<"$job" >/dev/null; then
  echo "vector rebuild provenance is incomplete or incorrect: $job" >&2
  exit 1
fi

first_search="$(query_semantic_index)"
if ! jq -e --arg uri "$expected_uri" '
  .strategy.id == "semantic.components.v1"
  and .items[0].uri == $uri
  and .items[0].score_kind == "cosine_similarity"
  and (.items[0].evidence | length) == 1
' <<<"$first_search" >/dev/null; then
  echo "semantic result did not rank the expected component first: $first_search" >&2
  exit 1
fi

# A fresh process must discover the active checksummed FAISS generation from
# the shared volume and serve the same semantic result without rebuilding.
docker stop "$container_name" >/dev/null
docker rm "$container_name" >/dev/null
start_container
wait_for_health
second_search="$(query_semantic_index)"
if ! jq -e --arg uri "$expected_uri" '.items[0].uri == $uri' <<<"$second_search" >/dev/null; then
  echo "restarted container did not recover the active generation: $second_search" >&2
  exit 1
fi

docker cp "$container_name:/var/lib/sbol-db/faiss" "$test_root/faiss" >/dev/null
test -f "$test_root/faiss/backend.lock"
test -f "$test_root/faiss/active/components.json"
test_completed=true
echo "FAISS container semantic lifecycle passed"
