#!/usr/bin/env bash

set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=model-download.sh
source "$script_directory/model-download.sh"

# Apache-2.0 model used only by the production-container semantic-search test.
# Every URL names an immutable Hugging Face commit and every file is verified
# independently so CI never builds an index from moving or corrupted weights.
readonly model_repository="Qdrant/all-MiniLM-L6-v2-onnx"
readonly model_revision="5f1b8cd78bc4fb444dd171e59b18f3a3af89a079"

fetch() {
  local destination="$1"
  local file="$2"
  local expected="$3"
  download_verified \
    "https://huggingface.co/${model_repository}/resolve/${model_revision}/${file}" \
    "$destination/$file" \
    "$expected"
}

main() {
  local destination="${1:?usage: docker/fetch-test-embedding-model.sh DESTINATION}"
  mkdir -p "$destination"
  fetch "$destination" config.json 1b4d8e2a3988377ed8b519a31d8d31025a25f1c5f8606998e8014111438efcd7
  fetch "$destination" special_tokens_map.json 5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a
  fetch "$destination" tokenizer.json da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0
  fetch "$destination" tokenizer_config.json bd2e06a5b20fd1b13ca988bedc8763d332d242381b4fbc98f8fead4524158f79
  fetch "$destination" model.onnx bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
