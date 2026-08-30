# syntax=docker/dockerfile:1
#
# Requires Docker BuildKit (`DOCKER_BUILDKIT=1`): `RUN --mount=type=cache` reuses Cargo registry/git across builds.
#
# Build: Debian Bookworm (`chef`, planner, builder). Runtime: distroless `cc-debian13`
# (pinned digest), one final stage per node binary. `mempool`/`storage`/`user`/`pre_launch`
# are slim (no GPU deps). `miner` is built with the `gpu` feature and additionally carries
# X11/xcb shared libraries copied from Debian trixie (bookworm glibc has no DSA for
# CVE-2026-0861; trixie libc is fixed per Debian security tracker). Bump image digests
# deliberately when rotating bases. Select an image with `docker build --target <node>`.

# Rust toolchain + cargo-chef (pins avoid registry/toolchain breakage on older Rust releases).
FROM rust:1.88-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS chef

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        git \
        build-essential \
        m4 \
        llvm \
        libclang-dev \
        diffutils \
        curl \
        cmake \
        libglfw3-dev \
        libxrandr-dev \
        libxinerama-dev \
        libxcursor-dev \
        libxi-dev \
        python3 \
        libgmp-dev \
        libmpfr-dev \
        libmpc-dev \
    && rm -rf /var/lib/apt/lists/*
# libglfw-dev + X11 dev headers: required by `glfw` / windowing in the workspace (vulkano miner path).

RUN cargo install cargo-chef --version 0.1.72

WORKDIR /lineage
ENV CARGO_TARGET_DIR=/lineage

FROM chef AS planner
# Minimal graph for `cargo chef prepare`; `.dockerignore` strips paths rustc does not need.
COPY Cargo.toml Cargo.lock /lineage/
COPY crates/fleet/Cargo.toml /lineage/crates/fleet/Cargo.toml
COPY crates/fleet/src /lineage/crates/fleet/src
COPY crates/fleet-integration/Cargo.toml /lineage/crates/fleet-integration/Cargo.toml
COPY crates/fleet-integration/src /lineage/crates/fleet-integration/src
COPY crates/fleet-core/Cargo.toml /lineage/crates/fleet-core/Cargo.toml
COPY crates/fleet-core/src /lineage/crates/fleet-core/src
COPY crates/fleet-api/Cargo.toml /lineage/crates/fleet-api/Cargo.toml
COPY crates/fleet-api/src /lineage/crates/fleet-api/src
COPY crates/fleet-node-common/Cargo.toml /lineage/crates/fleet-node-common/Cargo.toml
COPY crates/fleet-node-common/src /lineage/crates/fleet-node-common/src
COPY crates/fleet-wallet/Cargo.toml /lineage/crates/fleet-wallet/Cargo.toml
COPY crates/fleet-wallet/src /lineage/crates/fleet-wallet/src
COPY crates/fleet-mempool/Cargo.toml /lineage/crates/fleet-mempool/Cargo.toml
COPY crates/fleet-mempool/src /lineage/crates/fleet-mempool/src
COPY crates/fleet-storage/Cargo.toml /lineage/crates/fleet-storage/Cargo.toml
COPY crates/fleet-storage/src /lineage/crates/fleet-storage/src
COPY crates/fleet-miner/Cargo.toml /lineage/crates/fleet-miner/Cargo.toml
COPY crates/fleet-miner/src /lineage/crates/fleet-miner/src
COPY crates/fleet-user/Cargo.toml /lineage/crates/fleet-user/Cargo.toml
COPY crates/fleet-user/src /lineage/crates/fleet-user/src
COPY bins/mempool/Cargo.toml /lineage/bins/mempool/Cargo.toml
COPY bins/mempool/src /lineage/bins/mempool/src
COPY bins/storage/Cargo.toml /lineage/bins/storage/Cargo.toml
COPY bins/storage/src /lineage/bins/storage/src
COPY bins/user/Cargo.toml /lineage/bins/user/Cargo.toml
COPY bins/user/src /lineage/bins/user/src
COPY bins/miner/Cargo.toml /lineage/bins/miner/Cargo.toml
COPY bins/miner/src /lineage/bins/miner/src
COPY bins/pre_launch/Cargo.toml /lineage/bins/pre_launch/Cargo.toml
COPY bins/pre_launch/src /lineage/bins/pre_launch/src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /lineage/recipe.json /lineage/recipe.json

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release --features gpu --recipe-path /lineage/recipe.json

COPY Cargo.toml Cargo.lock /lineage/
COPY crates/fleet/Cargo.toml /lineage/crates/fleet/Cargo.toml
COPY crates/fleet/src /lineage/crates/fleet/src
COPY crates/fleet-integration/Cargo.toml /lineage/crates/fleet-integration/Cargo.toml
COPY crates/fleet-integration/src /lineage/crates/fleet-integration/src
COPY crates/fleet-core/Cargo.toml /lineage/crates/fleet-core/Cargo.toml
COPY crates/fleet-core/src /lineage/crates/fleet-core/src
COPY crates/fleet-api/Cargo.toml /lineage/crates/fleet-api/Cargo.toml
COPY crates/fleet-api/src /lineage/crates/fleet-api/src
COPY crates/fleet-node-common/Cargo.toml /lineage/crates/fleet-node-common/Cargo.toml
COPY crates/fleet-node-common/src /lineage/crates/fleet-node-common/src
COPY crates/fleet-wallet/Cargo.toml /lineage/crates/fleet-wallet/Cargo.toml
COPY crates/fleet-wallet/src /lineage/crates/fleet-wallet/src
COPY crates/fleet-mempool/Cargo.toml /lineage/crates/fleet-mempool/Cargo.toml
COPY crates/fleet-mempool/src /lineage/crates/fleet-mempool/src
COPY crates/fleet-storage/Cargo.toml /lineage/crates/fleet-storage/Cargo.toml
COPY crates/fleet-storage/src /lineage/crates/fleet-storage/src
COPY crates/fleet-miner/Cargo.toml /lineage/crates/fleet-miner/Cargo.toml
COPY crates/fleet-miner/src /lineage/crates/fleet-miner/src
COPY crates/fleet-user/Cargo.toml /lineage/crates/fleet-user/Cargo.toml
COPY crates/fleet-user/src /lineage/crates/fleet-user/src
COPY bins/mempool/Cargo.toml /lineage/bins/mempool/Cargo.toml
COPY bins/mempool/src /lineage/bins/mempool/src
COPY bins/storage/Cargo.toml /lineage/bins/storage/Cargo.toml
COPY bins/storage/src /lineage/bins/storage/src
COPY bins/user/Cargo.toml /lineage/bins/user/Cargo.toml
COPY bins/user/src /lineage/bins/user/src
COPY bins/miner/Cargo.toml /lineage/bins/miner/Cargo.toml
COPY bins/miner/src /lineage/bins/miner/src
COPY bins/pre_launch/Cargo.toml /lineage/bins/pre_launch/Cargo.toml
COPY bins/pre_launch/src /lineage/bins/pre_launch/src
# Slim binaries: no GPU deps. Keep flags aligned with `cargo chef cook` above.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --bin mempool --bin storage --bin user --bin pre_launch
# Miner: built with the `gpu` feature (needs X11 at runtime; see runtime-trixie-so below).
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --bin miner --features gpu

# Runtime libs absent from distroless/cc (`ldd`-based list on Debian-built `node`).
# Pulled via apt so transitive deps match distroless/cc-debian13 (trixie).
FROM debian:trixie-slim@sha256:cedb1ef40439206b673ee8b33a46a03a0c9fa90bf3732f54704f99cb061d2c5a AS runtime-trixie-so

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libx11-6 \
        libxcb1 \
        libxau6 \
        libxdmcp6 \
        libbsd0 \
        libmd0 \
    && rm -rf /var/lib/apt/lists/*

ARG TARGETARCH
RUN set -eu; \
    gnu="$(case "$TARGETARCH" in amd64) echo x86_64-linux-gnu ;; arm64) echo aarch64-linux-gnu ;; *) echo >&2 "unsupported TARGETARCH=$TARGETARCH"; exit 1 ;; esac)"; \
    mkdir -p "/dist/usr/lib/$gnu"; \
    cp -a "/usr/lib/$gnu/libX11.so"* "/dist/usr/lib/$gnu/"; \
    cp -a "/usr/lib/$gnu/libxcb.so"* "/dist/usr/lib/$gnu/"; \
    cp -a "/usr/lib/$gnu/libXau.so"* "/dist/usr/lib/$gnu/"; \
    cp -a "/usr/lib/$gnu/libXdmcp.so"* "/dist/usr/lib/$gnu/"; \
    cp -a "/usr/lib/$gnu/libbsd.so"* "/dist/usr/lib/$gnu/"; \
    cp -a "/usr/lib/$gnu/libmd.so"* "/dist/usr/lib/$gnu/"

# distroless/cc-debian13 — immutable digest (fixed glibc vs bookworm; refreshed to Debian 13.6
# to clear libssl3t64 CVE-2026-14456 and CVE-2026-45447).
FROM gcr.io/distroless/cc-debian13@sha256:9b615fff20e1a4fad29c2b30562580b212c7dd5e2225236735cca0070ed11c78 AS node-base

COPY .docker/conf/node_settings.toml /etc/node_settings.toml
COPY .docker/conf/tls_certificates.json /etc/tls_certificates.json
COPY .docker/conf/initial_block.json /etc/initial_block.json
COPY .docker/conf/api_config.json /etc/api_config.json
COPY .docker/conf/initial_issuance.json /etc/initial_issuance.json
COPY .docker/conf/mempool_miner_whitelist.json /etc/mempool_miner_whitelist.json

ENV CONFIG=/etc/node_settings.toml
ENV TLS_CONFIG=/etc/tls_certificates.json
ENV INITIAL_BLOCK_CONFIG=/etc/initial_block.json
ENV API_CONFIG=/etc/api_config.json
ENV INITIAL_ISSUANCE=/etc/initial_issuance.json
ENV API_USE_TLS=0
ENV MEMPOOL_MINER_WHITELIST=/etc/mempool_miner_whitelist.json
ENV RUST_LOG=info,debug

USER nonroot:nonroot

WORKDIR /

# Slim node images: no GPU deps, no X11 donor libs.
FROM node-base AS mempool
COPY --from=builder /lineage/release/mempool /lineage/mempool
ENTRYPOINT ["/lineage/mempool"]

FROM node-base AS storage
COPY --from=builder /lineage/release/storage /lineage/storage
ENTRYPOINT ["/lineage/storage"]

FROM node-base AS user
COPY --from=builder /lineage/release/user /lineage/user
ENTRYPOINT ["/lineage/user"]

FROM node-base AS pre_launch
COPY --from=builder /lineage/release/pre_launch /lineage/pre_launch
ENTRYPOINT ["/lineage/pre_launch"]

# Miner image: GPU build plus X11/xcb donor libs (vulkano/windowing path).
FROM node-base AS miner
COPY --from=builder /lineage/release/miner /lineage/miner
COPY --from=runtime-trixie-so /dist/usr/lib/ /usr/lib/
ENTRYPOINT ["/lineage/miner"]
