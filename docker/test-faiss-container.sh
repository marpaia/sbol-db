#!/usr/bin/env bash

set -euo pipefail

readonly image="${1:?usage: docker/test-faiss-container.sh IMAGE}"
readonly repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly fixture="$repository_root/tests/fixtures/faiss-container-smoke.json"
readonly container_name="sbol-db-faiss-smoke-$$"
readonly host_port="${SBOL_DB_FAISS_SMOKE_PORT:-18080}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-faiss-container.XXXXXX")"

cleanup() {
  docker logs "$container_name" 2>/dev/null || true
  docker rm --force "$container_name" >/dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT

# Loading the executable proves the distroless image contains the complete
# FAISS/OpenBLAS/OpenMP dynamic-library closure before the server smoke test.
docker run --rm "$image" --version >/dev/null

docker run --detach --name "$container_name" \
  --publish "127.0.0.1:${host_port}:8080" \
  --tmpfs /data:rw,noexec,nosuid,size=128m,uid=65532,gid=65532,mode=0750 \
  --volume "$fixture:/etc/sbol-db/search.json:ro" \
  --env DATABASE_URL=sqlite:///data/sbol.db \
  --env SBOL_DB_SEARCH_CONFIG=/etc/sbol-db/search.json \
  "$image" >/dev/null

for attempt in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${host_port}/healthz" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 30 ]; then
    echo "FAISS-enabled container did not become healthy" >&2
    exit 1
  fi
  sleep 1
done

docker logs "$container_name" 2>&1 | grep -q "search plugin deployment configured"
docker cp "$container_name:/data/faiss" "$test_root/faiss" >/dev/null
test -f "$test_root/faiss/backend.lock"
