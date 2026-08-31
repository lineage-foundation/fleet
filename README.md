<div align="center">
  <a>
    <img src="https://raw.githubusercontent.com/lineage-foundation/fleet/main/assets/hero.jpg" alt="Logo" style="width:100%;max-width:700px">
  </a>

  <h2 align="center">Lineage Fleet</h2> <div style="height:30px"></div>
<!--
  <div>
  <img src="https://img.shields.io/github/actions/workflow/status/lineage-foundation/fleet/.github/workflows/trivy.yml?branch=main" alt="Pipeline Status" style="display:inline-block"/>
  <img src="https://img.shields.io/crates/v/tw_chain" alt="Cargo Crates Version" style="display:inline-block" />
  </div> -->

  <p align="center">
    The network layer for the Lineage chain.
    <br />
    <br />
    <a href="https://lineage.foundation"><strong>Lineage Foundation »</strong></a>
    <br />
    <br />
  </p>
</div>

**Repository:** [lineage-foundation/fleet](https://github.com/lineage-foundation/fleet)

---

## Developing from source

Lineage Fleet is a Rust workspace. Install a recent toolchain via [rustup](https://rustup.rs), then clone this repository. On Linux you may need build dependencies similar to the `chef` stage in the root `Dockerfile` (LLVM/Clang, X11/GLFW headers, and related packages).

```bash
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
rustc --version
```

On Ubuntu-class systems:

```bash
sudo apt-get update && sudo apt-get install -y \
  build-essential m4 llvm libclang-dev clang cmake pkg-config \
  git curl python3 libglfw3-dev libxrandr-dev libxinerama-dev \
  libxcursor-dev libxi-dev
```

For day-to-day work, `cargo build --release`, `cargo test`, and IDE integration work as usual. `cargo build --release` produces one binary per node type under `target/release/`: `mempool`, `storage`, `miner`, `user`, and `pre_launch` (plus the `upgrade` helper). Run one directly, for example `target/release/storage --config=...`, instead of the old `node <subcommand>` dispatcher. For a multi-node local stack, prefer Docker Compose (next section); it mirrors the hardened runtime and pinned base images CI uses.

### Native vs Docker workflows

A native build (Linux or macOS host) is usually the fastest edit-compile loop: Cargo reuses incremental artifacts, and you avoid image layer rebuilds. You install the toolchain and system libraries yourself (see the Ubuntu package list above; other distros need equivalent GLFW/X11 and LLVM packages). The GPU miner backends (Vulkan/OpenGL) are behind an off-by-default `gpu` Cargo feature on the `miner` binary crate, so a plain `cargo build` is CPU-only and does not need the Vulkan/OpenGL/GLFW crates or their X11 system libraries; `mempool`, `storage`, `user`, and `pre_launch` never pull those in regardless of features. Build the miner with GPU acceleration via `cargo build --release -p miner --features gpu`; the Docker `miner` image already does this.

Docker and Compose have a higher cold-start cost (image build, large contexts without cache) but reproduce CI's pinned bases, distroless runtime, and Compose network layout. Most of the caching benefit lands on the dependency and toolchain layers. Enable [BuildKit](https://docs.docker.com/build/buildkit/) so the `Dockerfile` cache mounts apply (`DOCKER_BUILDKIT=1`, or a recent Docker Desktop where it is the default). See [Building only the container image](#building-only-the-container-image) for details.

The repo sets `split-debuginfo = "unpacked"` under `[profile.dev]` where supported, for slightly faster dev links. You can add `[profile.dev.package."*"]` with `opt-level = 1` locally if you want faster debug binaries at the cost of longer dependency compiles.

On Linux you can optionally use a faster linker ([mold](https://github.com/rui314/mold)), a shared compilation cache ([sccache](https://github.com/mozilla/sccache)), or [LLVM lld](https://lld.llvm.org/). These are quality-of-life only; Docker image builds do not need them.

---

## Running with Docker Compose

[`docker-compose.yml`](docker-compose.yml) runs mempool, storage, and miner on an isolated bridge network (`lineage`). The miner starts after mempool and storage via `depends_on`.

Defaults:

| Item | Behaviour |
|------|-----------|
| Build | Same `Dockerfile` as CI, one target per node: `fleet-mempool:local`, `fleet-storage:local`, `fleet-miner:local` (the miner target additionally carries the GPU/X11 runtime) |
| Platform | `linux/amd64` (set `FLEET_COMPOSE_PLATFORM=linux/arm64` for native Apple Silicon builds) |
| Writable DB | Per-service `/src` backed by a tmpfs named volume (`uid=65532`, distroless nonroot); reset with `docker compose down -v` after changing volumes |
| Config | One bind mount from the host: `./.docker/conf/node_settings.toml` → `/etc/node_settings.toml` (override the path with `NODE_SETTINGS`) |
| Hardening | `read_only: true`, `/tmp` tmpfs, `cap_drop: [ALL]`, `no-new-privileges` |

Quick start, from the repo root:

```bash
docker compose build
docker compose up
```

`docker compose build` uses this repo's root `Dockerfile`. If your daemon still defaults to the legacy builder, set `DOCKER_BUILDKIT=1` so the Cargo cache mounts apply (see [Building only the container image](#building-only-the-container-image)).

Customize the settings path:

```bash
NODE_SETTINGS=/absolute/path/to/node_settings.toml docker compose up
```

The first image build downloads toolchains and compiles everything; later runs are faster. Published ports match the bundled example settings (`3003` mempool API, `3001` storage, and so on; see the Compose `ports:` entries).

To rebuild a single service: `docker compose build mempool-node`. To stop and remove volumes: `docker compose down -v`.

---

## Configuration overrides

The config files under `.docker/conf/` are baked into the image as defaults (see `Dockerfile`). You don't have to mount a modified copy just to change one value: any config key can be overridden with a `LINEAGE_`-prefixed environment variable. Nested keys are joined with `__`. A few examples:

```
LINEAGE_MEMPOOL_API_PORT=3005
LINEAGE_PEER_LIMIT=2000
LINEAGE_MEMPOOL_UNICORN_FIXED_PARAM__ITERATIONS=2
```

The peer lists (`mempool_nodes`, `storage_nodes`, `miner_nodes`, `user_nodes`) are arrays in the config files, but as an env var they take a comma-separated address string instead:

```
LINEAGE_MEMPOOL_NODES=http://a:12300,http://b:12300
LINEAGE_STORAGE_NODES=http://storage-node:12330
LINEAGE_MINER_NODES=http://miner-node:12340
LINEAGE_USER_NODES=http://user-node:12360
```

Setting one of these fully replaces the corresponding list from the config file; it doesn't merge with it.

Precedence, lowest to highest: built-in defaults, then the config file, then `LINEAGE_*` env vars, then command-line flags where a flag exists for that value.

Env values are parsed by type, so a value for a text field that looks purely numeric (for example a jurisdiction or passphrase of only digits) can be misread as a number. For those few string fields, prefer the config file.

Because the config files are already in the image, a deployment can set only the `LINEAGE_*` variables it needs and skip mounting any config file at all.

---

## HTTP API

Each API-serving node (`mempool`, `storage`, `miner`, `user`) exposes a RESTful API under `/v1` on its existing API port. The API is described with OpenAPI 3.1; Swagger UI is served at `/v1/docs` and the raw spec at `/v1/openapi.json`. TLS, when enabled, uses the node's existing certs.

This replaces the previous RPC-style API. The old flat action-paths (`/make_payment`, `/block_by_num`, and so on) and the `{id, status, reason, route, content}` response envelope no longer exist; clients need to move to the `/v1` routes and plain JSON responses described in the OpenAPI document.

Behaviour differences worth noting for anyone porting from the old API: errors now use standard HTTP status codes with an `application/problem+json` body instead of a 200 with an in-body status string, so for example `GET /v1/blocks/latest` returns `404` when there is no block yet (the old endpoint returned `204`), and debug peer entries are now JSON objects rather than positional tuples.

Routes:

| Route | Nodes |
|-------|-------|
| `GET /v1/debug` | mempool, storage, miner, user |
| `GET /v1/blocks/latest` | storage |
| `GET /v1/blocks/{num}` | storage |
| `GET /v1/blocks` | storage |
| `GET /v1/blockchain-entries/{key}` | storage |
| `POST /v1/blockchain-entries/query` | storage |
| `GET /v1/supply` | mempool |
| `GET /v1/balances` | mempool |
| `POST /v1/balances/query` | mempool |
| `GET /v1/transactions/status` | mempool |
| `POST /v1/transactions/status:query` | mempool |
| `POST /v1/transactions` | mempool |
| `POST /v1/items` | mempool, user, miner (paired with an embedded user node) |
| `GET /v1/wallet` | user, miner |
| `GET /v1/wallet/keypairs` | user, miner |
| `POST /v1/wallet/addresses` | user, miner |
| `PUT /v1/wallet/passphrase` | user, miner |
| `POST /v1/wallet/keypairs` | user, miner |
| `POST /v1/wallet/running-total:refresh` | user, miner |
| `GET /v1/transactions/outgoing` | user, miner |
| `POST /v1/transactions:serialize` | user |
| `POST /v1/transactions:deserialize` | user |
| `GET /v1/mining/current-block` | miner |
| `POST /v1/payments` | user, miner (paired with an embedded user node) |
| `POST /v1/donation-requests` | user |

`GET /v1/blocks` and the `.../query` POST routes take repeated query params or a JSON body respectively, for looking up more than one key at a time. `GET /v1/wallet` takes optional `page` and `spent` query params, matching the paging/spent-filter behaviour of the old wallet-info endpoint.

Wallet routes are mounted on any node carrying a wallet DB (user, and a miner whether solo or paired with an embedded user node); `mining/current-block` is mounted on any node that mines.

The table above lists routes against the node kinds that can serve them, but a solo miner and a miner paired with an embedded user node don't expose quite the same set, so here's the exposure broken down per node type instead:

| Node type | Exposes |
|-----------|---------|
| `pre_launch` | no HTTP API surface |
| `storage` | `debug`, `blocks/latest`, `blocks/{num}`, `blocks`, `blockchain-entries/{key}`, `blockchain-entries/query` |
| `mempool` | `debug`, `supply`, `balances`, `balances/query`, `transactions/status`, `transactions/status:query`, `transactions`, `items` |
| `user` | `debug`, `wallet`, `wallet/keypairs` (GET + POST), `wallet/addresses`, `wallet/passphrase`, `wallet/running-total:refresh`, `transactions/outgoing`, `transactions:serialize`, `transactions:deserialize`, `donation-requests`, `items`, `payments` |
| `miner` (solo) | `debug`, `wallet`, `wallet/keypairs` (GET + POST), `wallet/addresses`, `wallet/passphrase`, `wallet/running-total:refresh`, `transactions/outgoing`, `mining/current-block` |
| `miner` (paired with an embedded user node) | everything the solo miner exposes, plus `items` and `payments` |

(All of the above are under `/v1/`.) A node's own `/v1/openapi.json` reflects exactly its own row here rather than the full combined spec, so Swagger UI at `/v1/docs` only ever shows routes that node actually serves.

Routes that have a configured key require an `x-api-key` header; this is documented as an OpenAPI security scheme (`api_key`) in the spec. `pre_launch` has no HTTP API surface.

---

## Building only the container image

Enable [BuildKit](https://docs.docker.com/build/buildkit/) when building locally (`DOCKER_BUILDKIT=1`, the default with Docker Desktop and modern Compose). The root `Dockerfile` uses cache mounts for Cargo registry/git downloads during `cargo chef cook` and `cargo build`, so repeat builds reuse crates across invocations.

The `Dockerfile` builds a separate final stage per node type, selected with `--target`: `mempool`, `storage`, `user`, `pre_launch`, and `miner`.

```bash
DOCKER_BUILDKIT=1 docker build --target mempool -t fleet-mempool:local --platform linux/amd64 .
DOCKER_BUILDKIT=1 docker build --target miner -t fleet-miner:local --platform linux/amd64 .
```

CI ([`.github/workflows/trivy.yml`](.github/workflows/trivy.yml)) builds the same `Dockerfile` with BuildKit via `docker/build-push-action` and the GitHub Actions cache (`cache-from` / `cache-to`), so repeat pipeline runs reuse layers across commits. Locally, the Dockerfile's `RUN --mount=type=cache` targets complement that when BuildKit is enabled.

Each final stage runs as `nonroot` on distroless `cc-debian13` (digest-pinned) and ships a single fixed-entrypoint binary: `/lineage/mempool`, `/lineage/storage`, `/lineage/user`, `/lineage/pre_launch`, or `/lineage/miner`. Only the `miner` target additionally copies X11 runtime `.so` files from `debian:trixie-slim` (so glibc matches the distroless Debian 13 base), since it is the only one built with the GPU feature; the other targets are slim, GPU-free images. See the `Dockerfile` for the exact `FROM` digests.

---

## Bumping pinned base images

The root `Dockerfile` pins immutable digests for:

- `rust:X.Y-bookworm` (chef and build stages; see `Dockerfile` for the current `X.Y` and digest),
- `debian:trixie-slim` (a temporary stage that installs the X11 runtime `.so` files copied into the final image; trixie so the libraries match distroless Debian 13),
- `gcr.io/distroless/cc-debian13` (runtime).

Suggested flow:

1. Choose the Rust toolchain revision you want (`rust:X.Y-bookworm`), matching Cargo and lockfile constraints.
2. Pull candidate images and copy the SHA256 digest from your registry mirror or vendor docs (`docker manifest inspect ...`, or your cloud console).
3. Update each `FROM ...@sha256:...` in `Dockerfile` in one atomic commit.
4. Re-run `docker build --platform linux/amd64 ...` locally and let CI Trivy pass on CRITICAL/HIGH.
5. If `cargo-chef` breaks after a toolchain jump, bump the pinned `cargo install cargo-chef --version ...` line only if you have to, and record why in the commit message.

Bumps are normal maintenance PRs; use the same reviewers as other infra or core Rust changes.

---

## Git flow

Base new work on an up-to-date `main` (fetch and rebase or merge from `origin/main`, whichever your team prefers). Open pull requests against `main` per [CONTRIBUTING.md](CONTRIBUTING.md).

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

- Use a type and optional scope: `type(scope): short summary`, in the imperative mood (*add*, *fix*, not *added*). Common types are `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`, and `perf`.
- Mark breaking changes with `!` after the type/scope (for example `feat(api)!: ...`) and/or a `BREAKING CHANGE:` paragraph in the body.
- Keep the first line within about 72 characters, and put detail in the body when needed.

Branch names are up to you; what matters for history and releases is consistent Conventional Commits on `main`.

---

## Trivy scanning

Workflow [`.github/workflows/trivy.yml`](.github/workflows/trivy.yml) runs:

- `trivy fs` for vulnerabilities and misconfiguration on the repository (respects [.trivyignore](.trivyignore)),
- `trivy image` for vulnerabilities on the freshly built per-node images (`fleet-miner:ci` and the slim `fleet-mempool:ci`).

Pull requests that touch `Dockerfile`, Compose, Cargo, `.docker/`, `.trivyignore`, or the workflow itself gate on severity `CRITICAL` and `HIGH` (see `env.TRIVY_SEVERITY` in the workflow).

### Handling policy exceptions

Trivy misconfiguration hits include rules such as Dockerfile `FROM ...` pinning. Exceptions belong in [.trivyignore](.trivyignore), only as stable AVD IDs with a one-line rationale. Look up IDs on [Aqua AVD](https://avd.aquasec.com/) (`avd-ds-0001` becomes `AVD-DS-0001`).

Example excerpt (current allowances; re-validate whenever `Dockerfile` changes):

`.trivyignore`:

```
# Exceptions must map to an https://avd.aquasec.com/ AVD ID and a one-line justification.

# Misconfig re-evaluated alongside Dockerfile bumps.
AVD-DS-0001
AVD-DS-0026
```

Do not add blanket ignores without an AVD ID and owner review.

## Links

- [Lineage Foundation](https://lineage.foundation)
- [Organization on GitHub](https://github.com/lineage-foundation)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0, see [LICENSE](LICENSE). This project continues the open-source Network lineage under the same license.
