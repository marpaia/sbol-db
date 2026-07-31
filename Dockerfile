# syntax=docker/dockerfile:1.7

# Multi-stage build for sbol-db.
#
# - FAISS is built from a checksum-pinned source release with its C API,
#   OpenBLAS, and OpenMP enabled. The production binary always includes the
#   `faiss` feature. The zero-config server ships a checksum-pinned BGE-small
#   ONNX semantic index; selecting FAISS remains an explicit search-config
#   choice.
# - The CPU-only ONNX Runtime used by local FastEmbed profiles is copied from
#   checksum-pinned Microsoft release archives for Linux x86-64 and ARM64.
# - cargo-chef caches workspace dependency builds in their own layer.
# - The binary links glibc dynamically. The RocksDB backend compiles a C++
#   library (librocksdb-sys), which a static musl target cannot link without a
#   musl C++ toolchain, so the build targets glibc and the runtime image
#   carries the C/C++ runtime. All TLS in this project is rustls, so no OpenSSL
#   is required.
# - The runtime stage is gcr.io/distroless/cc-debian12:nonroot: glibc plus
#   libstdc++/libgcc, no shell, no package manager, runs as UID 65532.

ARG RUST_VERSION=1.93
ARG FAISS_VERSION=1.14.3
ARG FAISS_SHA256=7f3c4ed9aec3bd7524382862f5fcbd4d8984e2a8979ff3bdb2c0bcea5144149e
ARG ONNXRUNTIME_VERSION=1.24.2
ARG ONNXRUNTIME_SHA256_AMD64=43725474ba5663642e17684717946693850e2005efbd724ac72da278fead25e6
ARG ONNXRUNTIME_SHA256_ARM64=6715b3d19965a2a6981e78ed4ba24f17a8c30d2d26420dbed10aac7ceca0085e
ARG SO_COMMIT=01c33c6d9b6c8dca12e7d3e37b49ee113093c2fa
ARG SO_SHA256=dde032d4c7cfb89a7013f2f8ab7420a8ef7dc469fbc2b0ffb38bef2a064a1d1f

############################
# Stage 0 — pinned FAISS C API and runtime library bundle
############################
FROM debian:bookworm-slim AS faiss-builder
ARG FAISS_VERSION
ARG FAISS_SHA256
ARG FAISS_BUILD_JOBS=2
ARG FAISS_OPT_LEVEL=generic

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates cmake curl libopenblas-dev ninja-build \
    && rm -rf /var/lib/apt/lists/*
COPY docker/install-faiss.sh /usr/local/bin/install-faiss
RUN FAISS_VERSION="$FAISS_VERSION" \
    FAISS_SHA256="$FAISS_SHA256" \
    FAISS_BUILD_JOBS="$FAISS_BUILD_JOBS" \
    FAISS_OPT_LEVEL="$FAISS_OPT_LEVEL" \
    FAISS_PREFIX=/opt/faiss \
    /usr/local/bin/install-faiss

# Distroless cc already supplies glibc, libstdc++, and libgcc. Bundle FAISS
# plus the remaining transitive libraries (OpenBLAS, OpenMP, Fortran runtime,
# and their non-base dependencies) under one loader path.
RUN set -eux; \
    mkdir -p /opt/faiss-runtime/lib; \
    mkdir -p /opt/sbol-db-data; \
    cp -a /opt/faiss/lib/libfaiss*.so* /opt/faiss-runtime/lib/; \
    ldd /opt/faiss/lib/libfaiss.so /opt/faiss/lib/libfaiss_c.so \
        | awk '$2 == "=>" && $3 ~ /^\// { print $3 }' \
        | sort -u > /tmp/faiss-runtime-libs; \
    while read -r library; do \
        case "$(basename "$library")" in \
            libc.so.*|libdl.so.*|libm.so.*|libpthread.so.*|librt.so.*|libstdc++.so.*|libgcc_s.so.*|ld-linux-*.so.*) ;; \
            *) cp -L "$library" "/opt/faiss-runtime/lib/$(basename "$library")" ;; \
        esac; \
    done < /tmp/faiss-runtime-libs; \
    test -e /opt/faiss-runtime/lib/libfaiss.so; \
    test -e /opt/faiss-runtime/lib/libfaiss_c.so

############################
# Stage 1 — pinned ONNX Runtime CPU library
############################
FROM debian:bookworm-slim AS onnxruntime-builder
ARG TARGETARCH
ARG ONNXRUNTIME_VERSION
ARG ONNXRUNTIME_SHA256_AMD64
ARG ONNXRUNTIME_SHA256_ARM64

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    case "$TARGETARCH" in \
        amd64) archive_arch=x64; expected="$ONNXRUNTIME_SHA256_AMD64" ;; \
        arm64) archive_arch=aarch64; expected="$ONNXRUNTIME_SHA256_ARM64" ;; \
        *) echo "unsupported ONNX Runtime architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    archive="onnxruntime-linux-${archive_arch}-${ONNXRUNTIME_VERSION}.tgz"; \
    curl --fail --location --retry 3 \
        "https://github.com/microsoft/onnxruntime/releases/download/v${ONNXRUNTIME_VERSION}/${archive}" \
        --output "/tmp/${archive}"; \
    echo "${expected}  /tmp/${archive}" | sha256sum --check --strict; \
    mkdir -p /opt/onnxruntime/lib; \
    tar -xzf "/tmp/${archive}" -C /tmp; \
    cp -a "/tmp/onnxruntime-linux-${archive_arch}-${ONNXRUNTIME_VERSION}/lib/"libonnxruntime.so* \
        /opt/onnxruntime/lib/; \
    test -e /opt/onnxruntime/lib/libonnxruntime.so

############################
# Stage 2 — immutable default BGE-small ONNX model bundle
############################
FROM debian:bookworm-slim AS builtin-model
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY docker/model-download.sh docker/fetch-builtin-bge-small-model.sh /usr/local/bin/
RUN bash /usr/local/bin/fetch-builtin-bge-small-model.sh \
    /opt/sbol-db/models/bge-small-en-v1.5 \
    && test -s /opt/sbol-db/models/bge-small-en-v1.5/model_optimized.onnx

############################
# Stage 2a — immutable Sequence Ontology snapshot
############################
FROM debian:bookworm-slim AS sequence-ontology
ARG SO_COMMIT
ARG SO_SHA256
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    mkdir -p /opt/sbol-db/ontologies; \
    curl --fail --location --retry 3 \
        "https://raw.githubusercontent.com/The-Sequence-Ontology/SO-Ontologies/${SO_COMMIT}/Ontology_Files/so.obo" \
        --output /opt/sbol-db/ontologies/so.obo; \
    echo "${SO_SHA256}  /opt/sbol-db/ontologies/so.obo" \
        | sha256sum --check --strict

############################
# Stage 3 — chef base
############################
FROM rust:${RUST_VERSION}-bookworm AS chef
# Node.js 20 is required by `sbol-db-ui`'s build.rs, which drives the
# Vite build of the embedded TypeScript SPA.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates gnupg \
    && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version ^0.1
WORKDIR /work

############################
# Stage 4 — planner: produce the dependency recipe
############################
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

############################
# Stage 5 — builder: cook deps, then build the binary
############################
FROM chef AS builder
# g++ (build-essential) builds the bundled RocksDB C++; clang/libclang drive
# bindgen (librocksdb-sys, aws-lc-sys behind rustls); protobuf-compiler and
# rustfmt are used by pg_query's and RocksDB's build scripts.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential clang libclang-dev libopenblas-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt

COPY --from=faiss-builder /opt/faiss /opt/faiss
ENV FAISS_DIR=/opt/faiss
ENV LD_LIBRARY_PATH=/opt/faiss/lib

COPY --from=planner /work/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo chef cook --release --bin sbol-db --no-default-features \
        --features lab,faiss,dynamic-ort --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release --bin sbol-db --no-default-features \
        --features lab,faiss,dynamic-ort \
    && cp target/release/sbol-db /usr/local/bin/sbol-db \
    && strip /usr/local/bin/sbol-db

############################
# Optional CI target — tests against the exact container-built FAISS
############################
FROM builder AS faiss-test
COPY --from=onnxruntime-builder /opt/onnxruntime /opt/onnxruntime
COPY --from=builtin-model /opt/sbol-db/models /opt/sbol-db/models
ENV LD_LIBRARY_PATH=/opt/faiss/lib:/opt/onnxruntime/lib
ENV ORT_DYLIB_PATH=/opt/onnxruntime/lib/libonnxruntime.so
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo test -p sbol-db-search-faiss --features native \
    && SBOL_DB_BGE_SMALL_MODEL_DIR=/opt/sbol-db/models/bge-small-en-v1.5 \
       cargo run -p sbol-db-search-eval --example bge_small_release_gate \
    && cp target/debug/examples/bge_small_release_gate /usr/local/bin/sbol-db-bge-release-gate

############################
# Stage 6 — runtime: distroless cc plus FAISS/ONNX Runtime, model, nonroot
############################
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/sbol-db /usr/local/bin/sbol-db
COPY --from=faiss-builder /opt/faiss-runtime/lib/ /usr/local/lib/
COPY --from=onnxruntime-builder /opt/onnxruntime/lib/ /usr/local/lib/
COPY --chown=65532:65532 --from=builtin-model /opt/sbol-db/models/ /opt/sbol-db/models/
COPY --chown=65532:65532 --from=sequence-ontology /opt/sbol-db/ontologies/ /opt/sbol-db/ontologies/
COPY --chown=65532:65532 --from=faiss-builder /opt/sbol-db-data/ /var/lib/sbol-db/
ENV LD_LIBRARY_PATH=/usr/local/lib
ENV ORT_DYLIB_PATH=/usr/local/lib/libonnxruntime.so
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/sbol-db"]
CMD ["server", "--bind", "0.0.0.0:8080"]
