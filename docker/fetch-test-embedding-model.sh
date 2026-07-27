#!/usr/bin/env bash

set -euo pipefail

# Apache-2.0 model used only by the production-container semantic-search test.
# Every URL names an immutable Hugging Face commit and every file is verified
# independently so CI never builds an index from moving or corrupted weights.
readonly model_repository="Qdrant/all-MiniLM-L6-v2-onnx"
readonly model_revision="5f1b8cd78bc4fb444dd171e59b18f3a3af89a079"
readonly destination="${1:?usage: docker/fetch-test-embedding-model.sh DESTINATION}"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

fetch() {
  local file="$1"
  local expected="$2"
  local target="$destination/$file"
  local temporary="$target.partial"

  if [ -f "$target" ] && [ "$(sha256 "$target")" = "$expected" ]; then
    return
  fi

  mkdir -p "$(dirname "$target")"
  curl --fail --location --retry 3 \
    "https://huggingface.co/${model_repository}/resolve/${model_revision}/${file}" \
    --output "$temporary"
  local actual
  actual="$(sha256 "$temporary")"
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $file: expected $expected, got $actual" >&2
    rm -f "$temporary"
    exit 1
  fi
  mv "$temporary" "$target"
}

mkdir -p "$destination"
fetch config.json 1b4d8e2a3988377ed8b519a31d8d31025a25f1c5f8606998e8014111438efcd7
fetch special_tokens_map.json 5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a
fetch tokenizer.json da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0
fetch tokenizer_config.json bd2e06a5b20fd1b13ca988bedc8763d332d242381b4fbc98f8fead4524158f79
fetch model.onnx bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5
