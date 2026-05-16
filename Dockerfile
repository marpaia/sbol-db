# syntax=docker/dockerfile:1.7

# Multi-stage build for sbol-db.
#
# - cargo-chef caches workspace dependency builds in their own layer.
# - The binary is statically linked against musl libc, so the final
#   image has zero shared-library surface (all TLS in this project is
#   rustls, so no OpenSSL is required).
# - The runtime stage is gcr.io/distroless/static-debian12:nonroot,
#   ~2 MB, no shell, no package manager, runs as UID 65532.

ARG RUST_VERSION=1.93

############################
# Stage 1 — chef base
############################
FROM rust:${RUST_VERSION}-bookworm AS chef
RUN cargo install cargo-chef --locked --version ^0.1
WORKDIR /work

############################
# Stage 2 — planner: produce the dependency recipe
############################
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

############################
# Stage 3 — builder: cook deps, then build the binary statically
############################
FROM chef AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

RUN rust_target="$(uname -m)-unknown-linux-musl" \
    && rustup target add "${rust_target}" \
    && echo "${rust_target}" > /tmp/rust_target

COPY --from=planner /work/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    rust_target="$(cat /tmp/rust_target)" \
    && cargo chef cook --release --target "${rust_target}" \
         --bin sbol-db --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    rust_target="$(cat /tmp/rust_target)" \
    && cargo build --release --target "${rust_target}" --bin sbol-db \
    && cp "target/${rust_target}/release/sbol-db" /usr/local/bin/sbol-db \
    && strip /usr/local/bin/sbol-db

############################
# Stage 4 — runtime: distroless static, nonroot
############################
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /usr/local/bin/sbol-db /usr/local/bin/sbol-db
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/sbol-db"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
