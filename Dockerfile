# syntax=docker/dockerfile:1.7

# Multi-stage build for sbol-db.
#
# - cargo-chef caches workspace dependency builds in their own layer.
# - The binary links glibc dynamically. The RocksDB backend compiles a C++
#   library (librocksdb-sys), which a static musl target cannot link without a
#   musl C++ toolchain, so the build targets glibc and the runtime image
#   carries the C/C++ runtime. All TLS in this project is rustls, so no OpenSSL
#   is required.
# - The runtime stage is gcr.io/distroless/cc-debian12:nonroot: glibc plus
#   libstdc++/libgcc, no shell, no package manager, runs as UID 65532.

ARG RUST_VERSION=1.93

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

COPY --from=planner /work/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo chef cook --release --bin sbol-db --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release --bin sbol-db \
    && cp target/release/sbol-db /usr/local/bin/sbol-db \
    && strip /usr/local/bin/sbol-db

############################
# Stage 4 — runtime: distroless cc (glibc + libstdc++), nonroot
############################
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/sbol-db /usr/local/bin/sbol-db
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/sbol-db"]
CMD ["server", "--bind", "0.0.0.0:8080"]
