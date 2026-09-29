# Changelog

## [0.2.0](https://github.com/lineage-foundation/fleet/compare/v0.1.1...v0.2.0) (2026-09-29)


### Features

* **backup:** upload on-disk RocksDB backups to S3/R2 ([#64](https://github.com/lineage-foundation/fleet/issues/64)) ([055db0b](https://github.com/lineage-foundation/fleet/commit/055db0bbdec9418647c10f1e312c5be21209df1f))


### Bug Fixes

* **deploy:** actually deploy the new image tag + verify it per node ([d024587](https://github.com/lineage-foundation/fleet/commit/d024587bca0862915dc24b66ef556f555275a2ea))
* **deploy:** actually deploy the versioned tag (not redeploy latest) + verify per node ([#63](https://github.com/lineage-foundation/fleet/issues/63)) ([d024587](https://github.com/lineage-foundation/fleet/commit/d024587bca0862915dc24b66ef556f555275a2ea))
* **deploy:** resolve project with a workspace-token-compatible query ([#61](https://github.com/lineage-foundation/fleet/issues/61)) ([f436a73](https://github.com/lineage-foundation/fleet/commit/f436a73f24de5bd5cfb27b10d01e953da1169edc))
* **deploy:** resolve project with workspace-token-compatible query ([f436a73](https://github.com/lineage-foundation/fleet/commit/f436a73f24de5bd5cfb27b10d01e953da1169edc))

## [0.1.1](https://github.com/lineage-foundation/fleet/compare/v0.1.0...v0.1.1) (2026-09-28)


### Bug Fixes

* **deploy:** correct gql() variables default (bash brace-expansion bug) ([#56](https://github.com/lineage-foundation/fleet/issues/56)) ([cd81434](https://github.com/lineage-foundation/fleet/commit/cd81434ada2f941e5651d27bf8d15acca9c33cc3))
