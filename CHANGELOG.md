# Changelog

## [0.8.2](https://github.com/m1sk9/chime/compare/chime-v0.8.1...chime-v0.8.2) (2026-10-22)


### Miscellaneous

* **deps:** update dependency @biomejs/biome to v2.6.2 ([#96](https://github.com/m1sk9/chime/issues/96)) ([99d940f](https://github.com/m1sk9/chime/commit/99d940f9817b8b49e9a71b759e6a6b5cf9f5eda3))
* **deps:** update dependency bun to v1.4.4 ([#100](https://github.com/m1sk9/chime/issues/100)) ([3df882f](https://github.com/m1sk9/chime/commit/3df882faed662a47369cf9daf7291f5d4d375cc0))
* **deps:** update dependency wrangler to v4.152.0 ([#101](https://github.com/m1sk9/chime/issues/101)) ([246c6cd](https://github.com/m1sk9/chime/commit/246c6cdab4ece9d67b7d9cd4f17ff909da5a043f))
* **deps:** update rust crate tokio to v1.54.1 ([#99](https://github.com/m1sk9/chime/issues/99)) ([bb54c85](https://github.com/m1sk9/chime/commit/bb54c8535724d2e546b3e6db9b42dc8a81282d0a))

## [0.8.1](https://github.com/m1sk9/chime/compare/chime-v0.8.0...chime-v0.8.1) (2026-10-21)


### Bug Fixes

* **docker:** move base images to Debian 13 distroless ([#95](https://github.com/m1sk9/chime/issues/95)) ([d84e089](https://github.com/m1sk9/chime/commit/d84e0895fe756b982648558cd9a4f6094afc76ad))
* log the cause of request errors instead of only "error sending request" ([#89](https://github.com/m1sk9/chime/issues/89)) ([a39a077](https://github.com/m1sk9/chime/commit/a39a0770cede3efda41616235de71e813fce7eab))


### Miscellaneous

* **deps:** update dependency wrangler to v4.151.2 ([#91](https://github.com/m1sk9/chime/issues/91)) ([2149e40](https://github.com/m1sk9/chime/commit/2149e404cbbe4eb33a691f566f651c819f23811c))
* **deps:** update dependency wrangler to v4.151.3 ([#92](https://github.com/m1sk9/chime/issues/92)) ([d166630](https://github.com/m1sk9/chime/commit/d166630e5e1b8057b750dbe9d5095e6999cea2ee))
* **deps:** update rust crate tokio to v1.54.0 ([#94](https://github.com/m1sk9/chime/issues/94)) ([3699718](https://github.com/m1sk9/chime/commit/3699718eba0c2d6d7da14fe979f508ec308b7ef8))
* **deps:** update swatinem/rust-cache action to v2.9.3 ([#93](https://github.com/m1sk9/chime/issues/93)) ([fcea15f](https://github.com/m1sk9/chime/commit/fcea15f57659392e28ef543da429b35f83c9078f))

## [0.8.0](https://github.com/m1sk9/chime/compare/chime-v0.7.0...chime-v0.8.0) (2026-10-17)


### ⚠ BREAKING CHANGES

* a status page, watch or feed URL that answers 3xx now fails every poll with a warn log instead of being followed. Configure the final URL the redirect points to.

### Bug Fixes

* stop following redirects when polling and keep credentials out of logs and embeds ([#87](https://github.com/m1sk9/chime/issues/87)) ([4a4ceed](https://github.com/m1sk9/chime/commit/4a4ceed13ad586a21859931ac5d36370b6987c7e))

## [0.7.0](https://github.com/m1sk9/chime/compare/chime-v0.6.0...chime-v0.7.0) (2026-10-17)


### Features

* support Instatus, incident.io, Datadog Status Pages and custom JSON feeds in [[status_pages]] ([#67](https://github.com/m1sk9/chime/issues/67)) ([3d4fd46](https://github.com/m1sk9/chime/commit/3d4fd46132fe6db92a3c6c5166db975d3b8c786d))


### Bug Fixes

* **deps:** bump rustls to 0.23.45 ([58a914e](https://github.com/m1sk9/chime/commit/58a914eed53b4ffaf595d439438d52582463abca))
* do not take the rss baseline from a feed with no entries ([#75](https://github.com/m1sk9/chime/issues/75)) ([7cda22e](https://github.com/m1sk9/chime/commit/7cda22e73f02fc00f035208c585f9bebe1d8a4f2))
* keep text the rss html renderer used to drop or mangle ([#80](https://github.com/m1sk9/chime/issues/80)) ([a62b67a](https://github.com/m1sk9/chime/commit/a62b67a78ae07890272bdb394eb6528f4811620a))
* pace posts to a webhook by Discord's rate-limit headers ([#77](https://github.com/m1sk9/chime/issues/77)) ([c9e968a](https://github.com/m1sk9/chime/commit/c9e968abd2da3b30d74f958c38ab2a72395e3fad))
* reject webhook names that resolve to the same env var ([#81](https://github.com/m1sk9/chime/issues/81)) ([95dd6e1](https://github.com/m1sk9/chime/commit/95dd6e146d3ee637bd0972db5b1727f68d675c64))
* render rss entry titles on one line ([#83](https://github.com/m1sk9/chime/issues/83)) ([38687ab](https://github.com/m1sk9/chime/commit/38687abf85670c46b1c61f805bde5fd1daf290ff))
* require https for webhook URLs and redirect targets ([#78](https://github.com/m1sk9/chime/issues/78)) ([d36b0bf](https://github.com/m1sk9/chime/commit/d36b0bf619448d89d81a15f5e7a8189f22cecb2c))
* send a reminder once when its time repeats on a DST fall-back ([#74](https://github.com/m1sk9/chime/issues/74)) ([91989fd](https://github.com/m1sk9/chime/commit/91989fdaf10585516548e18311cfc31da87f076e))
* shut down on SIGTERM even while a tick is running ([#79](https://github.com/m1sk9/chime/issues/79)) ([eaf2055](https://github.com/m1sk9/chime/commit/eaf2055e60d629201fe6232d97e8b1456d5669cf))
* stop re-posting status incidents that share an id ([#82](https://github.com/m1sk9/chime/issues/82)) ([6fdae04](https://github.com/m1sk9/chime/commit/6fdae043d2e0d09ce395391aa0ce0e0435e848f6))


### Performance

* render RSS entries only when they are new and streamline the polling loop ([#68](https://github.com/m1sk9/chime/issues/68)) ([0e08c19](https://github.com/m1sk9/chime/commit/0e08c19449695081d590ffea47da6af7d475b975))


### Documentation

* add VitePress documentation site at chime.m1sk9.dev ([#71](https://github.com/m1sk9/chime/issues/71)) ([02bd382](https://github.com/m1sk9/chime/commit/02bd3820d0f513707bdceb2386d702b6989eda55))
* slim the README and point to the docs site and llms.txt ([774d018](https://github.com/m1sk9/chime/commit/774d018f39707ad9920180aef60f6f8d72c32295))


### Miscellaneous

* **deps:** update dependency wrangler to v4.151.1 ([#72](https://github.com/m1sk9/chime/issues/72)) ([a480dc9](https://github.com/m1sk9/chime/commit/a480dc93adfc818c627207052a99dab3b80e7d2c))
* **deps:** update docker/dockerfile docker tag to v1.28 ([#62](https://github.com/m1sk9/chime/issues/62)) ([6daf786](https://github.com/m1sk9/chime/commit/6daf78634b5b1caf05c25f82addaa7e727202aed))
* **deps:** update rust crate serde_json to v1.0.152 ([#64](https://github.com/m1sk9/chime/issues/64)) ([975ab3c](https://github.com/m1sk9/chime/commit/975ab3c623960cc55ee8eb3572ef29d34ebb6694))
* **deps:** update rust crate serde_json to v1.0.154 ([#65](https://github.com/m1sk9/chime/issues/65)) ([562b8e2](https://github.com/m1sk9/chime/commit/562b8e2a3ea1f6e6d482fb71607e58f2e27c776f))
* **deps:** update rust crate tokio to v1.53.2 ([#59](https://github.com/m1sk9/chime/issues/59)) ([2f942f5](https://github.com/m1sk9/chime/commit/2f942f57b5d061bf7be2b19a3eecd9edaa3bd4c6))
* **deps:** update rust crate tokio to v1.53.3 ([#66](https://github.com/m1sk9/chime/issues/66)) ([b315c05](https://github.com/m1sk9/chime/commit/b315c050715a5fc3898de895653f522c6f74f7f6))
* **deps:** update rust crate toml to v1.1.7 ([#61](https://github.com/m1sk9/chime/issues/61)) ([bf83c07](https://github.com/m1sk9/chime/commit/bf83c07dd8c76d97bd33099b934d430a444896f7))
* **deps:** update rust crate toml to v1.1.8 ([#63](https://github.com/m1sk9/chime/issues/63)) ([2662a98](https://github.com/m1sk9/chime/commit/2662a98f19b6f158898789b23c99ac8848a19056))
* **deps:** update taiki-e/install-action digest to 8cca091 ([#58](https://github.com/m1sk9/chime/issues/58)) ([6c96e64](https://github.com/m1sk9/chime/commit/6c96e6459dcdab97992e92f6725ada9c53ad982e))
* list perf and docs commits in the changelog ([#85](https://github.com/m1sk9/chime/issues/85)) ([9b6b9aa](https://github.com/m1sk9/chime/commit/9b6b9aa181e0aae2bd16c5902409c30c16c63bd8))


### CI

* drop the unused cross install and build the image on PRs ([#84](https://github.com/m1sk9/chime/issues/84)) ([fcd6518](https://github.com/m1sk9/chime/commit/fcd651879335fabcb0bb5f8b765c6208517fa1a8))

## [0.6.0](https://github.com/m1sk9/chime/compare/chime-v0.5.0...chime-v0.6.0) (2026-09-27)


### Features

* subscribe to RSS, Atom and JSON feeds and post new entries to Discord ([#57](https://github.com/m1sk9/chime/issues/57)) ([8077dd8](https://github.com/m1sk9/chime/commit/8077dd83b6daed6b5df87967986f0c5f4ebba431))
* watch Firefox, Chrome and arbitrary JSON release feeds and post new versions to Discord ([#55](https://github.com/m1sk9/chime/issues/55)) ([101931a](https://github.com/m1sk9/chime/commit/101931a35d00ac81b178497359a242f3cf8a7d0e))


### Miscellaneous

* **deps:** update rust crate reqwest to v0.13.5 ([#50](https://github.com/m1sk9/chime/issues/50)) ([536d0f4](https://github.com/m1sk9/chime/commit/536d0f47f8cb0b0eab9e515b5259b0c83bba7c8a))
* **deps:** update rust crate thiserror to v2.0.21 ([#54](https://github.com/m1sk9/chime/issues/54)) ([33249e8](https://github.com/m1sk9/chime/commit/33249e84d4bb4beedb91cbcdaf7f5d98844c356a))
* **deps:** update rust crate toml to v1.1.6 ([#52](https://github.com/m1sk9/chime/issues/52)) ([85c4cf5](https://github.com/m1sk9/chime/commit/85c4cf542b74e018b4fc8da6808234297c5adbdf))
* **deps:** update taiki-e/install-action digest to 3f74d7c ([#47](https://github.com/m1sk9/chime/issues/47)) ([bb460e6](https://github.com/m1sk9/chime/commit/bb460e67d93fe7327ce212b2b09e43c3b82da409))
* **deps:** update taiki-e/install-action digest to 4cef141 ([#53](https://github.com/m1sk9/chime/issues/53)) ([ccb0dfd](https://github.com/m1sk9/chime/commit/ccb0dfd7fb7058dfc5920990249894d479b7d7a2))

## [0.5.0](https://github.com/m1sk9/chime/compare/chime-v0.4.0...chime-v0.5.0) (2026-09-07)


### Features

* log an hourly status poll summary and request gzip-compressed feeds ([#48](https://github.com/m1sk9/chime/issues/48)) ([49fec19](https://github.com/m1sk9/chime/commit/49fec1916536cc872b6474db0ef6e67b0375ce1a))

## [0.4.0](https://github.com/m1sk9/chime/compare/chime-v0.3.4...chime-v0.4.0) (2026-09-03)


### Features

* forward Atlassian Statuspage incidents to Discord webhooks ([#41](https://github.com/m1sk9/chime/issues/41)) ([0f7d1ba](https://github.com/m1sk9/chime/commit/0f7d1ba95b692812fd4a5bab4d334036d85a0f44))


### Miscellaneous

* **deps:** update docker/dockerfile docker tag to v1.27 ([#43](https://github.com/m1sk9/chime/issues/43)) ([f0b6aef](https://github.com/m1sk9/chime/commit/f0b6aefbff977aa328334ae210393a9b52c7bc78))
* **deps:** update rust crate serde_json to v1.0.151 ([#45](https://github.com/m1sk9/chime/issues/45)) ([873dbf8](https://github.com/m1sk9/chime/commit/873dbf89e96cb690ea0789ecc412509efd475bc4))
* **deps:** update rust crate toml to v1.1.5 ([#42](https://github.com/m1sk9/chime/issues/42)) ([f59f48e](https://github.com/m1sk9/chime/commit/f59f48eaf8beaa872b1448e3fbd10e52183dece7))
* **deps:** update taiki-e/install-action digest to e67fa11 ([#20](https://github.com/m1sk9/chime/issues/20)) ([fbcf4ff](https://github.com/m1sk9/chime/commit/fbcf4ff632dc8f2af7cfda274c4fc59d28da68b3))

## [0.3.4](https://github.com/m1sk9/chime/compare/chime-v0.3.3...chime-v0.3.4) (2026-08-19)


### Miscellaneous

* **deps:** Bump quinn-proto from 0.11.14 to 0.11.16 ([#34](https://github.com/m1sk9/chime/issues/34)) ([5b031db](https://github.com/m1sk9/chime/commit/5b031dbf2d377023662409c752fecbf8ec675d06))

## [0.3.3](https://github.com/m1sk9/chime/compare/chime-v0.3.2...chime-v0.3.3) (2026-08-08)


### Miscellaneous

* **deps:** update rust crate thiserror to v2.0.20 ([#39](https://github.com/m1sk9/chime/issues/39)) ([e8107a3](https://github.com/m1sk9/chime/commit/e8107a38948bad9155b4088d82b8a72fe8801d3b))
* **deps:** update swatinem/rust-cache action to v2.9.2 ([#37](https://github.com/m1sk9/chime/issues/37)) ([b972f1f](https://github.com/m1sk9/chime/commit/b972f1f02a58eee07c7602178ae1413c4f35439f))

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
