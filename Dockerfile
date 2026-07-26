# syntax=docker/dockerfile:1.7

# Multi-stage build for sbol-db.
#
# - FAISS is built from a checksum-pinned source release with its C API,
#   OpenBLAS, and OpenMP enabled. The production binary always includes the
#   `faiss` feature; selecting the backend remains an explicit search-config
#   choice at runtime.
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
# Stage 1 — chef base
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
# Stage 2 — planner: produce the dependency recipe
############################
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

############################
# Stage 3 — builder: cook deps, then build the binary
############################
FROM chef AS builder
# g++ (build-essential) builds the bundled RocksDB C++; clang/libclang drive
# bindgen (librocksdb-sys, aws-lc-sys behind rustls); protobuf-compiler and
# rustfmt are used by pg_query's and RocksDB's build scripts.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential clang libclang-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt

COPY --from=faiss-builder /opt/faiss /opt/faiss
ENV FAISS_DIR=/opt/faiss
ENV LD_LIBRARY_PATH=/opt/faiss/lib

COPY --from=planner /work/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo chef cook --release --bin sbol-db --features faiss --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release --bin sbol-db --features faiss \
    && cp target/release/sbol-db /usr/local/bin/sbol-db \
    && strip /usr/local/bin/sbol-db

############################
# Optional CI target — tests against the exact container-built FAISS
############################
FROM builder AS faiss-test
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo test -p sbol-db-search-faiss --features native

############################
# Stage 4 — runtime: distroless cc plus FAISS, nonroot
############################
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/sbol-db /usr/local/bin/sbol-db
COPY --from=faiss-builder /opt/faiss-runtime/lib/ /usr/local/lib/
ENV LD_LIBRARY_PATH=/usr/local/lib
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/sbol-db"]
CMD ["server", "--bind", "0.0.0.0:8080"]
