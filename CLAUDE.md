# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`chime` is a long-running Rust process that reads a TOML schedule and posts to Discord webhooks at the configured times. Distributed as a single static binary and a distroless container at `ghcr.io/m1sk9/chime`. See `README.md` for the user-facing manual (config schema, webhook resolution, runtime semantics).

## Common commands

```sh
# Dev loop — use debug build; release LTO takes several minutes.
cargo build
cargo test --verbose                 # all tests
cargo test <name>                    # filter by test name substring
cargo test --lib config::tests       # run one module

cargo fmt --all -- --check           # CI gate: rustfmt
cargo clippy --all-targets --all-features -- -D warnings   # CI gate: clippy (warnings = errors)

# Release artifacts
cargo build --release                # binary at target/release/chime
docker build -f docker/Dockerfile -t chime:dev .

# Run locally — needs config + at least one webhook env var.
CHIME_CONFIG=./config/config.toml.example \
  CHIME_WEBHOOK_TEAM=https://discord.com/api/webhooks/... \
  cargo run

# Coverage (matches CI)
cargo llvm-cov --all-features --workspace
```

Toolchain pinned to stable via `rust-toolchain.toml`. Edition 2024.

## Architecture

Four modules wired together in `src/main.rs`:

- **`config`** — TOML parsing and the type-driven validation layer. Domain types (`TimeOfDay`, `WeekdaySet`, `WebhookRef`, `ReminderName`, `Message`, `TickInterval`, `LogLevel`) implement `serde::Deserialize` via `try_from`, so every invariant (non-empty strings, `1..=60` interval, `HH:MM` form, known weekday, known IANA tz) is enforced at deserialization time. `Config::from_toml` adds the cross-cutting checks (at least one reminder, names unique). `#[serde(deny_unknown_fields)]` is set on every struct — unknown TOML keys are rejected. **Add new config fields here, never as free-form strings parsed later.**
- **`runtime`** — bridges parsed `Config` → `RunConfig`. The key step is `resolve_webhook`: turns the logical `webhook` name into an env key (`CHIME_WEBHOOK_<UPPER>`, non-alnum → `_`), reads `<KEY>_FILE` first (Docker secrets), falls back to `<KEY>`, trims, then `Url::parse`. After `resolve`, the runtime config holds real `Url`s and is ready to execute.
- **`scheduler`** — owns the main loop. `tokio::time::interval` with `MissedTickBehavior::Skip` (overdue ticks are collapsed, not replayed). On each tick: compute "now" in the configured `Tz`, find reminders whose `(hour, minute, weekday)` matches, and de-duplicate via `last_fired: HashMap<reminder_name, minute_truncated_datetime>`. **The dedup record is updated *before* the HTTP send** — a network failure must not cause a retry in the same minute. `tokio::select!` against `SIGINT` / `SIGTERM` for clean shutdown.
- **`notifier`** — `Notifier` trait + `Discord` impl. `Discord::send` POSTs `{"content": message}` with a 10s reqwest timeout. On non-2xx, the response body is truncated to `MAX_ERROR_BODY` (512 bytes) before being attached to the error — keep this cap; Discord error pages can be large. The trait exists so scheduler tests can inject a counting fake.

Runtime is `#[tokio::main(flavor = "current_thread")]` — single-threaded by design. Don't reach for the multi-threaded runtime without a real reason.

## Conventions specific to this repo

- **Fail-fast at startup.** Any config or webhook problem must surface as an error from `Config::from_toml` or `runtime::resolve` and exit non-zero from `main`. The process never partially starts.
- **Errors are typed per layer** with `thiserror`. `anyhow` is only used in `main.rs` for top-level context. New error variants go on the existing per-module enum; don't introduce `Box<dyn Error>`.
- **No catch-up semantics.** A missed minute (host asleep, network down) is a lost notification. Don't add retry logic or backfill — that's the documented model.
- **Webhook URLs never appear in `config.toml`.** They are always resolved from env. Tests that need a URL build one inline (see `mk_reminder` in `runtime.rs`).
- **`unsafe { std::env::set_var(...) }` is required** when mutating env in tests on Rust 2024. The `EnvGuard` helper in `runtime.rs` is the canonical pattern — restore on `Drop` to keep tests parallel-safe.
- **Logging is structured JSON** via `tracing-subscriber` with the `json` formatter on stdout. New log sites should use `tracing` macros with key/value fields (`info!(reminder = %name, ...)`), not formatted strings.
- **Commit messages: Conventional Commits, English.** Releases are automated by release-please (`release-please-config.json`).

## CI gates

Three jobs in `.github/workflows/ci.yaml` must pass before merge:

1. `check` — `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --verbose`.
2. `coverage` — `cargo llvm-cov` → Codecov (`fail_ci_if_error: true`).
3. `build` — matrix build for `x86_64-unknown-linux-gnu` and `x86_64-unknown-linux-musl` (the musl build uses `cross`).

Run `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test` locally before pushing.
