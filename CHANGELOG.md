# Changelog

## [0.3.2](https://github.com/m1sk9/chime/compare/chime-v0.3.1...chime-v0.3.2) (2026-07-31)


### Miscellaneous

* **deps:** update docker/dockerfile docker tag to v1.26 ([#36](https://github.com/m1sk9/chime/issues/36)) ([956a296](https://github.com/m1sk9/chime/commit/956a2961f8599bcfed68eef762e54a0f7323ad5f))
* **deps:** update rust crate tokio to v1.53.1 ([#32](https://github.com/m1sk9/chime/issues/32)) ([6e4dc15](https://github.com/m1sk9/chime/commit/6e4dc15d9974ae4b365cedc188b7bfa803d0bc0e))
* **deps:** update rust crate toml to v1.1.4 ([#35](https://github.com/m1sk9/chime/issues/35)) ([9d52f71](https://github.com/m1sk9/chime/commit/9d52f71593e4ac6710bea41200171db166d14e5a))

## [0.3.1](https://github.com/m1sk9/chime/compare/chime-v0.3.0...chime-v0.3.1) (2026-07-19)


### Miscellaneous

* **deps:** update rust crate anyhow to v1.0.104 ([#29](https://github.com/m1sk9/chime/issues/29)) ([686faa8](https://github.com/m1sk9/chime/commit/686faa83f11feb149c2f5956cded410a76bac98c))
* **deps:** update rust crate serde to v1.0.229 ([#30](https://github.com/m1sk9/chime/issues/30)) ([a0096e8](https://github.com/m1sk9/chime/commit/a0096e88e673611c2129ddb12b117f6a4b69d001))
* **deps:** update rust crate thiserror to v2.0.19 ([#31](https://github.com/m1sk9/chime/issues/31)) ([2b44586](https://github.com/m1sk9/chime/commit/2b44586ee6a3b761dfdb71c77870f6f9dc7c2fc1))
* **deps:** update rust crate tokio to v1.52.4 ([#27](https://github.com/m1sk9/chime/issues/27)) ([1b695de](https://github.com/m1sk9/chime/commit/1b695de0b23ec7dc029f508e0768f80b5742524d))
* **deps:** update rust crate tokio to v1.53.0 ([#28](https://github.com/m1sk9/chime/issues/28)) ([7377c5f](https://github.com/m1sk9/chime/commit/7377c5fe1fc06e6515f77c9e188c6d876b3c1312))
* **deps:** update rust crate toml to v1.1.3 ([#25](https://github.com/m1sk9/chime/issues/25)) ([fe62cef](https://github.com/m1sk9/chime/commit/fe62cef81057f48d5d2122858038418968dc1f2b))

## [0.3.0](https://github.com/m1sk9/chime/compare/chime-v0.2.2...chime-v0.3.0) (2026-07-13)


### Features

* support day-of-month reminder scheduling ([#23](https://github.com/m1sk9/chime/issues/23)) ([dbd2665](https://github.com/m1sk9/chime/commit/dbd26657718ef929188ede49799197c0d04bf28d))

## [0.2.2](https://github.com/m1sk9/chime/compare/chime-v0.2.1...chime-v0.2.2) (2026-06-25)


### Miscellaneous

* **deps:** update docker/dockerfile docker tag to v1.25 ([#21](https://github.com/m1sk9/chime/issues/21)) ([8139c85](https://github.com/m1sk9/chime/commit/8139c852f2964a45df36c61b3ed5cf15d759e6fe))
* **deps:** update rust crate anyhow to v1.0.103 ([#22](https://github.com/m1sk9/chime/issues/22)) ([09906aa](https://github.com/m1sk9/chime/commit/09906aa57e2462ce707e045bfd8a212ccfd95be3))


### CI

* bump release-please-action to v5 for Node.js 24 runtime ([#18](https://github.com/m1sk9/chime/issues/18)) ([018bee4](https://github.com/m1sk9/chime/commit/018bee4bc4763e1814e61bc56ae0a300524a6171))

## [0.2.1](https://github.com/m1sk9/chime/compare/chime-v0.2.0...chime-v0.2.1) (2026-06-16)


### Bug Fixes

* **docker:** link statically against musl to drop glibc dependency ([#16](https://github.com/m1sk9/chime/issues/16)) ([875bb53](https://github.com/m1sk9/chime/commit/875bb536484dbc543422e5de3352f099663220a6)), closes [#15](https://github.com/m1sk9/chime/issues/15)

## [0.2.0](https://github.com/m1sk9/chime/compare/chime-v0.1.0...chime-v0.2.0) (2026-06-16)


### Features

* add liveness heartbeat and health subcommand ([#13](https://github.com/m1sk9/chime/issues/13)) ([cb635ff](https://github.com/m1sk9/chime/commit/cb635ff5cc3c47709859ccb4b2d9792f242f32a1))


### Bug Fixes

* **deps:** migrate reqwest to 0.13 (rustls/aws-lc-rs) ([#12](https://github.com/m1sk9/chime/issues/12)) ([18d2fbd](https://github.com/m1sk9/chime/commit/18d2fbd5451b0b23119fd66428012cdc0d447e5e))


### Miscellaneous

* **deps:** update codecov/codecov-action digest to 0fb7174 ([#11](https://github.com/m1sk9/chime/issues/11)) ([4f7c414](https://github.com/m1sk9/chime/commit/4f7c414af293c8c280f23dc9417cac4ad9b4a0d4))
* **deps:** update docker/dockerfile docker tag to v1.24 ([#7](https://github.com/m1sk9/chime/issues/7)) ([b21c4df](https://github.com/m1sk9/chime/commit/b21c4df726f7ba5742021c738aeb4340a3e84ce3))
* **deps:** update taiki-e/install-action digest to 15449e3 ([#6](https://github.com/m1sk9/chime/issues/6)) ([9f555d1](https://github.com/m1sk9/chime/commit/9f555d1ffa8ec4e3bf5eebe84762cecdfb8d70d2))
* remove clap in favor of hand-rolled arg parsing ([#14](https://github.com/m1sk9/chime/issues/14)) ([2b34d34](https://github.com/m1sk9/chime/commit/2b34d34bc224d73a6544df1fea44c47838b1320c))

## 0.1.0 (2026-06-07)


### CI

* fix CI failures from initial run ([7e96d58](https://github.com/m1sk9/chime/commit/7e96d58b7ca610db5bff63d24dd4fe2dfd3df70c))
* force JavaScript actions onto Node.js 24 ([78bedc1](https://github.com/m1sk9/chime/commit/78bedc1fc9d8122463fb11e9fd742da30ef4da58))
* use client-id for create-github-app-token ([#2](https://github.com/m1sk9/chime/issues/2)) ([9f06a9e](https://github.com/m1sk9/chime/commit/9f06a9e0153a33810ae508652a1b0ee03da20a23))
