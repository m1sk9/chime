---
layout: doc
---

# ソースからビルド

Rust の stable ツールチェーンが必要です (`rust-toolchain.toml` を参照)．

```sh
cargo build --release
# バイナリの出力先: target/release/chime
```

リリースプロファイルでは，配布するバイナリを小さく保つために LTO (`lto = true`，`codegen-units = 1`) を有効にしています．その代わり，`cargo build --release` はデバッグビルドよりかなり時間がかかります．キャッシュがない状態では数分かかると考えてください．開発中は `cargo build` (デバッグ) を使い，LTO を待つのはリリースビルドだけにしてください．

コンテナイメージ:

```sh
docker build -f docker/Dockerfile -t chime:dev .
```
