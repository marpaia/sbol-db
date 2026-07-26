#!/usr/bin/env bash

set -euo pipefail

readonly faiss_version="${FAISS_VERSION:-1.14.3}"
readonly faiss_sha256="${FAISS_SHA256:-7f3c4ed9aec3bd7524382862f5fcbd4d8984e2a8979ff3bdb2c0bcea5144149e}"
readonly faiss_prefix="${FAISS_PREFIX:-/opt/faiss}"
readonly faiss_jobs="${FAISS_BUILD_JOBS:-2}"
readonly faiss_opt_level="${FAISS_OPT_LEVEL:-generic}"

build_root="$(mktemp -d "${TMPDIR:-/tmp}/sbol-db-faiss.XXXXXX")"
trap 'rm -rf "$build_root"' EXIT

archive="$build_root/faiss.tar.gz"
source_dir="$build_root/source"

curl -fsSL \
  "https://github.com/facebookresearch/faiss/archive/refs/tags/v${faiss_version}.tar.gz" \
  -o "$archive"
echo "${faiss_sha256}  ${archive}" | sha256sum --check --strict

mkdir -p "$source_dir"
tar -xzf "$archive" --strip-components=1 -C "$source_dir"

cmake -S "$source_dir" -B "$source_dir/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$faiss_prefix" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBLA_VENDOR=OpenBLAS \
  -DBUILD_SHARED_LIBS=ON \
  -DBUILD_TESTING=OFF \
  -DFAISS_ENABLE_C_API=ON \
  -DFAISS_ENABLE_CUVS=OFF \
  -DFAISS_ENABLE_GPU=OFF \
  -DFAISS_ENABLE_PYTHON=OFF \
  -DFAISS_OPT_LEVEL="$faiss_opt_level"
cmake --build "$source_dir/build" --parallel "$faiss_jobs"
cmake --install "$source_dir/build"

test -f "$faiss_prefix/include/faiss/Index.h"
test -e "$faiss_prefix/lib/libfaiss.so"
test -e "$faiss_prefix/lib/libfaiss_c.so"
