# Contributing to fleet

Thank you for your interest in contributing to Lineage Foundation **fleet** (network node).

## How to contribute

1. **Open an issue** – For bugs, features, or questions, open an issue in this repository.
2. **Fork and clone** – Fork the repo and clone it locally.
3. **Create a branch** – Use a descriptive branch name (e.g. `feat/add-x`, `fix/issue-y`).
4. **Make changes** – Follow the code style in this repo (see [.editorconfig](.editorconfig)). Run `cargo fmt` and `cargo clippy` where applicable.
5. **Run tests** – `cargo test` (and `cargo build --release` for integration checks as needed).
6. **Submit a PR** – Open a pull request against `main`. Describe your changes clearly.

## Code style

- Follow [EditorConfig](.editorconfig) settings; Rust code uses 4-space indentation.
- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages:
  - Format: `<type>[optional scope]: <description>`
  - Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`
  - Example: `feat(mempool): improve batch handling`

## Pull request process

- PRs require review before merge where branch protection applies.
- Squash commits when merging if that is the org default.

## Questions?

Open an issue or see [lineage.foundation](https://lineage.foundation).
