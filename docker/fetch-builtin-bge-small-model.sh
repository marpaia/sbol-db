#!/usr/bin/env bash

set -euo pipefail

# The zero-configuration semantic-search model. The repository and commit are
# immutable; every required runtime artifact has its own SHA-256 below. Keep
# this list in review with the profile revision in `search_config.rs`.
#
# Source: Qdrant's Apache-2.0 quantized ONNX port of
# BAAI/bge-small-en-v1.5. It is 384-dimensional and approximately 67 MB.
readonly model_repository="Qdrant/bge-small-en-v1.5-onnx-Q"
readonly model_revision="52398278842ec682c6f32300af41344b1c0b0bb2"

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=model-download.sh
source "$script_directory/model-download.sh"

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
  local destination="${1:?usage: docker/fetch-builtin-bge-small-model.sh DESTINATION}"
  mkdir -p "$destination"
  fetch "$destination" config.json 13582bcf2effc85b7bf3d3f5532e686bc1c9ce86bb009d10f0ec33cbe92299dd
  fetch "$destination" special_tokens_map.json 5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a
  fetch "$destination" tokenizer.json d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66
  fetch "$destination" tokenizer_config.json 0b29c7bfc889e53b36d9dd3e686dd4300f6525110eaa98c76a5dafceb2029f53
  fetch "$destination" model_optimized.onnx 51f1bd0addd6e859e42c2c8021a5e5461385bb676a649f4b269aa445449f2431
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
