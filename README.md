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

Lineage Fleet is a Rust workspace. Install a recent toolchain via [rustup](https://rustup.rs), then clone this repository. On Linux you may need build dependencies similar to the `chef` stage in the root `Dockerfile` (LLVM/Clang, X11/Glfw headers, and related packages).

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

For day-to-day work: `cargo build --release`, `cargo test`, and IDE integration work as usual. **For a multi-node local stack, prefer Docker Compose** (next section)—it mirrors the hardened runtime and pinned base images CI uses.

---

## Running with Docker Compose

[`docker-compose.yml`](docker-compose.yml) runs **mempool**, **storage**, and **miner** on an isolated bridge network (`lineage`). The miner starts after mempool and storage via `depends_on`.

**Defaults**

| Item | Behaviour |
|------|-----------|
| Build | `fleet-node:local`, same `Dockerfile` as CI |
| Platform | `linux/amd64` (`FLEET_COMPOSE_PLATFORM=linux/arm64` for Apple Silicon–native builds) |
| Writable DB | Per-service `/src` backed by a tmpfs named volume (`uid=65532`, distroless nonroot); reset with `docker compose down -v` after changing volumes |
| Config | Only **one** bind mount from the host: `./.docker/conf/node_settings.toml` → `/etc/node_settings.toml` (override path with `NODE_SETTINGS`) |
| Hardening | `read_only: true`, `/tmp` tmpfs, `cap_drop: [ALL]`, `no-new-privileges` |

**Quick start**

From the repo root:

```bash
docker compose build
docker compose up
```

Customize the settings path:

```bash
NODE_SETTINGS=/absolute/path/to/node_settings.toml docker compose up
```

The first image build downloads toolchains and compiles everything; subsequent runs are faster. Published ports match the bundled example settings (`3003` mempool API, `3001` storage, etc.—see Compose `ports:`).

Optional: rebuild one service (`docker compose build mempool-node`). Stop and remove volumes: `docker compose down -v`.

---

## Building only the container image

```bash
docker build -t fleet-node:local --platform linux/amd64 .
```

The final stage runs as **`nonroot`**; the shipped binary is **`/lineage/lineage`** (distroless **`cc-debian12`**, digest-pinned, plus X11 runtime `.so` copied from Debian bookworm). Inspect `Dockerfile` for exact `FROM` digests after pull-through mirrors.

---

## Bumping pinned base images

The root `Dockerfile` pins **immutable digests** for:

- **`rust:X.Y-bookworm`** (chef / build stages; see `Dockerfile` for current `X.Y` and digest),
- **`debian:bookworm-slim`** (temporary stage that installs X11 runtime `.so` files copied into the final image),
- **`gcr.io/distroless/cc-debian12`** (runtime).

Recommended flow:

1. Choose the **Rust toolchain** revision you want (`rust:X.Y-bookworm`), matching Cargo / lockfile constraints.
2. Pull candidate images; copy the SHA256 digest from your registry mirror or vendor docs (`docker manifest inspect …` or your cloud console).
3. Update each `FROM …@sha256:…` in `Dockerfile` in one atomic commit.
4. Re-run `docker build --platform linux/amd64 …` locally and let **CI Trivy** pass on CRITICAL/HIGH.
5. If `cargo-chef` fails after a toolchain jump, bump the pinned `cargo install cargo-chef --version …` line only if absolutely required—record why in the commit message.

Owner / merge policy: bumps are normal maintenance PRs; default reviewer same as infra or core Rust changes—align with team practice.

---

## Git flow

Base new work on an up-to-date **`main`** (fetch and merge or rebase from `origin/main` as your team prefers). Open pull requests to **`main`** per [CONTRIBUTING.md](CONTRIBUTING.md).

**Commit messages** follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

- Use a type and optional scope: `type(scope): short summary` (imperative mood: *add*, *fix*, not *added*). Common types include `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`, and `perf`.
- Mark breaking changes with `!` after the type/scope (e.g. `feat(api)!: …`) and/or a `BREAKING CHANGE:` paragraph in the commit body, per the spec.
- Keep the first line within ~72 characters; put detail in the body when needed.

Branch names are up to your team; what matters for history and releases is consistent **Conventional Commits** on `main`.

---

## Trivy scanning

Workflow [`.github/workflows/trivy.yml`](.github/workflows/trivy.yml) runs:

- **`trivy fs`** — vulnerabilities + misconfiguration on the repository (respects [.trivyignore](.trivyignore))
- **`trivy image`** — vulnerabilities on the freshly built **`fleet-node:ci`** image

Pull requests touching `Dockerfile`, Compose, Cargo, `.docker/`, `.trivyignore`, or the workflow itself gate on **severity `CRITICAL` and `HIGH`** (see workflow `env.TRIVY_SEVERITY`).

### Handling policy exceptions

Trivy misconfiguration hits include rules such as Dockerfile `FROM …` pinning. Exceptions belong in [.trivyignore](.trivyignore) **only as stable AVD IDs** with one-line rationale. Look up IDs on [AVD Aquasec](https://avd.aquasec.com/) (`avd-ds-0001` → `AVD-DS-0001`).

Example excerpt (today’s allowances—re-validate whenever `Dockerfile` changes):

**.trivyignore**

```
# Exceptions must map to an https://avd.aquasec.com/ AVD ID and a one-line justification.

# Misconfig re-evaluated alongside Dockerfile bumps.
AVD-DS-0001
AVD-DS-0026
```

Do **not** add blanket ignores without an AVD and owner review.

## Links

- [Lineage Foundation](https://lineage.foundation)
- [Organization on GitHub](https://github.com/lineage-foundation)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0 — see [LICENSE](LICENSE). This project continues the open-source Network lineage under the same license.
