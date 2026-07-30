#!/usr/bin/env bash

# Shared, integrity-first helper for model bundles fetched during a container
# build or an operator's local setup. Callers pin both an immutable upstream
# revision and an SHA-256 for every file; downloads become visible only after
# verification succeeds.

set -euo pipefail

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

download_verified() {
  local url="$1"
  local target="$2"
  local expected="$3"
  local temporary="$target.partial"

  if [ -f "$target" ] && [ "$(sha256 "$target")" = "$expected" ]; then
    return
  fi

  mkdir -p "$(dirname "$target")"
  rm -f "$temporary"
  curl \
    --fail \
    --location \
    --silent \
    --show-error \
    --connect-timeout 30 \
    --max-time 600 \
    --retry 8 \
    --retry-all-errors \
    --retry-delay "${SBOL_DB_DOWNLOAD_RETRY_DELAY_SECONDS:-2}" \
    --retry-max-time 300 \
    --output "$temporary" \
    "$url"
  local actual
  actual="$(sha256 "$temporary")"
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $target: expected $expected, got $actual" >&2
    rm -f "$temporary"
    exit 1
  fi
  mv "$temporary" "$target"
}
