<div id="top"></div>

<!-- PROJECT LOGO -->
<br />

<div align="center">
  <a>
    <img src="https://raw.githubusercontent.com/lineage-foundation/prime/main/assets/hero.jpg" alt="Logo" style="width:100%;max-width:700px">
  </a>

  <h2 align="center">Lineage Prime</h2> <div style="height:30px"></div>
  <p align="center"><em>Two-way chain core library</em></p>

  <div>
  <img src="https://img.shields.io/crates/v/prime" alt="Cargo Crates Version" style="display:inline-block" />
  </div>

  <p align="center">
    The blockchain layer for the Lineage stack.
    <br />
    <br />
    <a href="https://lineage.foundation"><strong>Lineage Foundation »</strong></a>
    <br />
    <br />
  </p>
</div>

**Repository:** [lineage-foundation/prime](https://github.com/lineage-foundation/prime) — migrated from [AIBlockOfficial/Chain](https://github.com/AIBlockOfficial/Chain). The Rust crate was historically published as **`tw_chain`** on crates.io; this repo uses the package name **`prime`**.

[简体中文](https://github.com/lineage-foundation/prime/blob/main/readmes/README.zhs.md) | [Español](https://github.com/lineage-foundation/prime/blob/main/readmes/README.es.md) | [عربي ](https://github.com/lineage-foundation/prime/blob/main/readmes/README.ar.md)| [Deutsch](https://github.com/lineage-foundation/prime/blob/main/readmes/README.de.md) | [Français](https://github.com/lineage-foundation/prime/blob/main/readmes/README.fr.md)

..

## Getting Started

Running Prime assumes you have Rust installed and are using a Unix system. You can clone this repo and run the `Makefile` to set everything up for a development environment:

```
make
cargo build
cargo test
```

..

## Use

Add the crate to your project:

```toml
[dependencies]
prime = "1.1.3"
```

Or from git before crates.io publication:

```toml
[dependencies]
prime = { git = "https://github.com/lineage-foundation/prime" }
```

Command line:

```
cargo add prime
```

**Downstream note:** Projects that depended on **`tw_chain`** from crates.io should switch to **`prime`** (this repo) via git/path until a crates.io release under the new name.

## Links

- [Lineage Foundation](https://lineage.foundation)
- [lineage-foundation on GitHub](https://github.com/lineage-foundation)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0 — see [LICENSE](LICENSE).
