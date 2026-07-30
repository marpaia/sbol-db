#!/usr/bin/env bash

set -euo pipefail

readonly image="${1:?usage: docker/test-faiss-container.sh IMAGE}"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly repository_root
readonly corpus="$repository_root/crates/sbol-db-postgres/tests/fixtures/simple_component.ttl"
readonly container_name="sbol-db-faiss-e2e-$$"
readonly volume_name="sbol-db-faiss-e2e-$$"
readonly host_port="${SBOL_DB_FAISS_SMOKE_PORT:-18080}"
readonly base_url="http://127.0.0.1:${host_port}"
readonly public_graph="http://synbiohub.org/public"
readonly expected_uri="https://example.org/sbol-db/test/promoter_j23119"
readonly builtin_model_dir="/opt/sbol-db/models/bge-small-en-v1.5"
readonly builtin_model_revision="sha3-256:bf577972c34b37578aa42965fa8401d5538f4b4c007c810332e936548658c7b3"
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

for command in curl docker jq python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command is required for the FAISS container test" >&2
    exit 1
  fi
done

# Prove that the production model-fetch helper survives an interrupted
# transfer before relying on it for the real checksum-pinned model bundle.
"$repository_root/docker/test-model-download-retry.sh"

# This probes the final distroless image, not the builder. The image must carry
# the checksum-pinned default BGE bundle, and the revision command gives a
# reproducible value for a custom FAISS composition root.
docker run --rm "$image" --version >/dev/null
model_revision="$({
  docker run --rm "$image" util fastembed-revision "$builtin_model_dir" \
    --onnx-file model_optimized.onnx
} | jq -er '.revision')"
if [ "$model_revision" != "$builtin_model_revision" ]; then
  echo "production image contains an unexpected BGE bundle revision: $model_revision" >&2
  exit 1
fi

jq -n --arg revision "$model_revision" --arg model_dir "$builtin_model_dir" '{
  topology: {
    default_strategy: "legacy.explorer.v1",
    indexes: [{
      index: "components",
      backend: "faiss-local",
      embedding_profile: "builtin.sbol-text-bge-small.v1",
      vector_name: "content",
      graph_payload_field: "graph"
    }],
    embedding_strategies: [{
      id: "semantic.components.v1",
      version: "1",
      display_name: "Semantic components",
      description: "Container semantic-search lifecycle test",
      embedding_profile: "builtin.sbol-text-bge-small.v1",
      vector_index: "components",
      vector_name: "content",
      graph_payload_field: "graph",
      distance: "cosine"
    }]
  },
  embeddings: [{
    kind: "fastembed_local",
    profile: {
      id: "builtin.sbol-text-bge-small.v1",
      model: "Qdrant/bge-small-en-v1.5-onnx-Q@52398278842ec682c6f32300af41344b1c0b0bb2",
      revision: $revision,
      dimension: 384,
      normalization: "l2",
      batch_size: 32
    },
    bundle: {
      directory: $model_dir,
      onnx_file: "model_optimized.onnx",
      pooling: "cls",
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
    --volume "$test_root/search.json:/etc/sbol-db/search.json:ro" \
    --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
    --env SBOL_DB_SEARCH_CONFIG=/etc/sbol-db/search.json \
    "$image" >/dev/null
}

start_builtin_container() {
  docker run --detach --name "$container_name" \
    --publish "127.0.0.1:${host_port}:8080" \
    --volume "$volume_name:/var/lib/sbol-db" \
    --env DATABASE_URL=sqlite:///var/lib/sbol-db/sbol.db \
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

query_builtin_index() {
  curl -fsS \
    --header 'content-type: application/json' \
    --data '{
      "query": {"kind": "text", "text": "strong constitutive promoter"},
      "page": {"limit": 5}
    }' \
    "$base_url/api/v2/search"
}

# The zero-configuration image must load its shipped model, register the BGE
# strategy, queue its startup rebuild, and return the imported object without
# a model mount or an external search configuration.
start_builtin_container
wait_for_health

builtin_strategies="$(curl -fsS "$base_url/api/v2/search/strategies")"
if ! jq -e '
  .default_strategy == "builtin.sbol-text-vector.v2"
  and (.items | any(.id == "builtin.sbol-text-vector.v2" and .version == "2"))
' <<<"$builtin_strategies" >/dev/null; then
  echo "built-in BGE strategy was not registered: $builtin_strategies" >&2
  exit 1
fi

for attempt in $(seq 1 90); do
  builtin_search="$(query_builtin_index)"
  if jq -e --arg uri "$expected_uri" '
    .strategy.id == "builtin.sbol-text-vector.v2"
    and .items[0].uri == $uri
    and .items[0].score_kind == "cosine_similarity"
  ' <<<"$builtin_search" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 90 ]; then
    echo "built-in BGE index did not finish its startup rebuild: $builtin_search" >&2
    exit 1
  fi
  sleep 1
done

docker stop "$container_name" >/dev/null
docker rm "$container_name" >/dev/null

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
      "embedding_profile": "builtin.sbol-text-bge-small.v1",
      "distance": "cosine",
      "batch_size": 32,
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
