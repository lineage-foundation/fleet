# Contributing

Contributions to Lineage Fleet are welcome, whether that's a bug report, a feature, or a fix.

## Getting started

1. Open an issue for bugs, features, or questions.
2. Fork and clone the repository.
3. Create a branch with a descriptive name (`feat/add-x`, `fix/issue-y`).
4. Make your changes. Run `cargo fmt` and `cargo clippy`, and follow the style in [.editorconfig](.editorconfig) (Rust uses 4-space indentation).
5. Run `cargo test` (and `cargo build --release` for integration checks where relevant).
6. Open a pull request against `main` and describe what changed and why.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): description`, with types like `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, and `chore`. For example, `feat(mempool): improve batch handling`.

## Pull requests

PRs need review before merge where branch protection is enabled. Squash on merge if that's the org default.

Questions? Open an issue or see [lineage.foundation](https://lineage.foundation).
