#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 SOURCE_VIRTUOSO_DB OUTPUT_NQUADS [WORK_DIRECTORY]" >&2
  exit 2
}

[[ $# -ge 2 && $# -le 3 ]] || usage

source_db=$1
output_nq=$2
work_parent=${3:-${TMPDIR:-/tmp}}
image='tenforce/virtuoso@sha256:de97286328aa0babb9e06e9626321d91363ba3f260529b9083d1fa02f36ad664'
isql='/usr/local/virtuoso-opensource/bin/isql-v'
container="sbol-db-virtuoso-export-$$"
work_dir=$(mktemp -d "${work_parent%/}/sbol-db-virtuoso-export.XXXXXX")
temporary_output=""
dba_password=${VIRTUOSO_DBA_PASSWORD:-dba}

if [[ -z "${VIRTUOSO_DBA_PASSWORD:-}" && -n "${SYNBIOHUB_CONFIG:-}" ]]; then
  if [[ ! -f "$SYNBIOHUB_CONFIG" ]]; then
    echo "SYNBIOHUB_CONFIG is not a file: $SYNBIOHUB_CONFIG" >&2
    exit 1
  fi
  dba_password=$(jq -er '.triplestore.password | select(type == "string" and length > 0)' \
    "$SYNBIOHUB_CONFIG")
fi

if [[ ! -f "$source_db" ]]; then
  echo "source Virtuoso database is not a file: $source_db" >&2
  exit 1
fi
if [[ -e "$output_nq" ]]; then
  echo "refusing to overwrite existing output: $output_nq" >&2
  exit 1
fi

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ -n "$temporary_output" ]]; then
    rm -f "$temporary_output"
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

mkdir -p "$work_dir/data/dumps"
cp "$source_db" "$work_dir/data/virtuoso.db"

docker run --detach --platform linux/amd64 --name "$container" \
  --env DBA_PASSWORD=dba \
  --volume "$work_dir/data:/data" \
  "$image" >/dev/null

online=0
for _ in $(seq 1 120); do
  if docker logs "$container" 2>&1 | grep -q 'Server online at 1111'; then
    online=1
    break
  fi
  sleep 2
done
if [[ "$online" != 1 ]]; then
  docker logs "$container" >&2
  echo "Virtuoso did not become ready" >&2
  exit 1
fi
if ! docker exec "$container" test -x "$isql"; then
  echo "Virtuoso is online, but the pinned image does not provide executable $isql." >&2
  echo "The image layout changed; this is not a credential failure." >&2
  exit 1
fi

isql_diagnostic="$work_dir/isql-diagnostic.txt"
if ! docker exec "$container" "$isql" 1111 dba "$dba_password" \
  exec='select count(*) from DB.DBA.RDF_QUAD;' >"$isql_diagnostic" 2>&1; then
  if grep -q 'CL034: Bad login' "$isql_diagnostic"; then
    echo "Virtuoso is online, but the supplied DBA credential was rejected." >&2
    echo "Set VIRTUOSO_DBA_PASSWORD to the SQL DBA password recorded for this snapshot." >&2
  else
    echo "Virtuoso is online and isql-v exists, but the validation query failed:" >&2
    sed -E 's/(dba[[:space:]]+)[^[:space:]]+/\1[REDACTED]/g' "$isql_diagnostic" >&2
  fi
  exit 1
fi

docker exec "$container" "$isql" 1111 dba "$dba_password" \
  exec="dump_nquads('dumps', 1, 1000000000, 1);"

docker stop --timeout 60 "$container" >/dev/null

fragments=()
while IFS= read -r fragment; do
  fragments+=("$fragment")
done < <(find "$work_dir/data/dumps" -type f \( -name '*.nq' -o -name '*.nq.gz' \) -print | sort)
if [[ ${#fragments[@]} -eq 0 ]]; then
  echo "Virtuoso produced no N-Quads fragments" >&2
  exit 1
fi

mkdir -p "$(dirname "$output_nq")"
temporary_output="${output_nq}.part.$$"
for fragment in "${fragments[@]}"; do
  if [[ "$fragment" == *.gz ]]; then
    gzip -cd -- "$fragment" >>"$temporary_output"
  else
    command cat -- "$fragment" >>"$temporary_output"
  fi
done
mv "$temporary_output" "$output_nq"
temporary_output=""

output_sha256=$(shasum -a 256 "$output_nq" | awk '{print $1}')
output_bytes=$(stat -f '%z' "$output_nq")
echo "exported $output_bytes bytes to $output_nq"
echo "sha256 $output_sha256"
