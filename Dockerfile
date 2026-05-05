# syntax=docker/dockerfile:1
#
# Build: Debian Bookworm (`chef`, planner, builder). Runtime: distroless `cc-debian12`
# (pinned digest) plus X11/xcb shared libraries copied from Debian bookworm. Bump image
# digests deliberately when rotating bases.

# Rust toolchain + cargo-chef (pins avoid registry/toolchain breakage on older Rust releases).
FROM rust:1.85-bookworm@sha256:e51d0265072d2d9d5d320f6a44dde6b9ef13653b035098febd68cce8fa7c0bc4 AS chef

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
    && rm -rf /var/lib/apt/lists/*
# libglfw-dev + X11 dev headers: required by `glfw` / windowing in the workspace (vulkano miner path).

RUN cargo install cargo-chef --version 0.1.72

WORKDIR /aiblock
ENV CARGO_TARGET_DIR=/aiblock

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /aiblock/recipe.json /aiblock/recipe.json

RUN cargo chef cook --release --recipe-path /aiblock/recipe.json

COPY . .
RUN cargo build --release --bin node

# Runtime libs absent from distroless/cc (`ldd`-based list on bookworm-built `node`).
# Pulled via apt so transitive deps stay consistent with bookworm.
FROM debian:bookworm-slim@sha256:f9c6a2fd2ddbc23e336b6257a5245e31f996953ef06cd13a59fa0a1df2d5c252 AS runtime-bookworm-so

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

# distroless/cc-debian12 — immutable digest.
FROM gcr.io/distroless/cc-debian12@sha256:e2d29aec8061843706b7e484c444f78fafb05bfe47745505252b1769a05d14f1 AS runner

COPY --from=builder /aiblock/release/node /aiblock/aiblock

COPY --from=runtime-bookworm-so /dist/usr/lib/ /usr/lib/

COPY .docker/conf/node_settings.toml /etc/node_settings.toml
COPY .docker/conf/tls_certificates.json /etc/tls_certificates.json
COPY .docker/conf/initial_block.json /etc/initial_block.json
COPY .docker/conf/api_config.json /etc/api_config.json
COPY .docker/conf/initial_issuance.json /etc/initial_issuance.json
COPY .docker/conf/mempool_miner_whitelist.json /etc/mempool_miner_whitelist.json

ARG NODE_TYPE_ARG=mempool
ENV NODE_TYPE=$NODE_TYPE_ARG
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

ENTRYPOINT ["/aiblock/aiblock"]

# Exec form — no shell; overrides with `docker run … storage` etc.
CMD ["mempool"]
