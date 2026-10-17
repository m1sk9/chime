---
layout: doc
---

# Building from Source

Requires a Rust stable toolchain (see `rust-toolchain.toml`).

```sh
cargo build --release
# Binary at: target/release/chime
```

The release profile enables LTO (`lto = true`, `codegen-units = 1`) to keep the shipped binary small. The trade-off is that `cargo build --release` takes noticeably longer than a debug build — expect several minutes on a cold cache. Use `cargo build` (debug) during development; only the release build needs to wait on LTO.

Container image:

```sh
docker build -f docker/Dockerfile -t chime:dev .
```
