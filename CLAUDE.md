# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`chime` is a long-running Rust process that reads a TOML schedule, posts to Discord webhooks at the configured times, polls Atlassian Statuspage instances to forward incident updates to the same webhooks, and polls release feeds (Firefox, Chrome, arbitrary JSON) to announce new versions. Distributed as a single static binary and a distroless container at `ghcr.io/m1sk9/chime`. See `README.md` for the user-facing manual (config schema, webhook resolution, runtime semantics).

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

Modules wired together in `src/main.rs`:

- **`config`** — TOML parsing and the type-driven validation layer. Domain types (`TimeOfDay`, `WeekdaySet`, `WebhookRef`, `ReminderName`, `Message`, `TickInterval`, `LogLevel`, `StatusUrl`, `HttpsUrl` (avatars and watch URLs), `JsonPointer`, `PollInterval`, `Impact`, `WatchName`, and the watch enums `FirefoxChannel` / `ChromePlatform` / `ChromeChannel` / `ChromeRollout`) implement `serde::Deserialize` via `try_from`, so every invariant (non-empty strings, `1..=60` interval, `HH:MM` form, known weekday, known IANA tz, https-only URLs, `/`-prefixed JSON pointer, `60..=3600` poll interval) is enforced at deserialization time. `Config::from_toml` adds the cross-cutting checks (at least one reminder, status page *or* watch, names unique within each list, no Chrome `extended` channel on a platform VersionHistory rejects it for). `#[serde(deny_unknown_fields)]` is set on every struct — unknown TOML keys are rejected. `Impact` is the one type with two parsers: derived `Deserialize` for config (strict, rejects typos) and `from_wire` for API responses (lenient, unknown → `None`). **Add new config fields here, never as free-form strings parsed later.**
- **`runtime`** — bridges parsed `Config` → `RunConfig`. The key step is `resolve_webhook`: turns the logical `webhook` name into an env key (`CHIME_WEBHOOK_<UPPER>`, non-alnum → `_`), reads `<KEY>_FILE` first (Docker secrets), falls back to `<KEY>`, trims, then `Url::parse`. Status pages additionally get their `api_url` joined here and their `display_name` defaulted; watches get their source URL, label and `Extractor` from `watch::resolve_source`, their `display_name` defaulted to that label, and are grouped by URL into `RunFeed`s (one request per URL, at the shortest member interval). After `resolve`, the runtime config holds real `Url`s and is ready to execute.
- **`scheduler`** — owns the main loop. `tokio::time::interval` with `MissedTickBehavior::Skip` (overdue ticks are collapsed, not replayed). On each tick: write the heartbeat, fire due reminders, then poll **exactly one** status page or watch feed (the most overdue that is due, across both lists; `Job` is `Page(i) | Feed(i)` and its derived `Ord` is the tie-break, so pages win exact ties). One fetch per tick is deliberate — the heartbeat is written at the top of each tick and `chime health` calls it stale past `2 * tick_interval`, so polling every due entry would let a third-party outage restart the container. Sends are not bounded the same way (a shared feed or a burst of incident updates can post several times in one tick), so every send goes through `Heartbeating`, which rewrites the heartbeat after each Discord request. Reminders de-duplicate via `last_fired: HashMap<reminder_name, minute_truncated_datetime>`; status pages and watches are gated by `last_polled` (keyed by `Job`) + `poll_interval`. **Every dedup record is updated *before* the HTTP send** — a network failure must not cause a retry in the same minute/poll. `tokio::select!` against `SIGINT` / `SIGTERM` for clean shutdown. Each poll returns a `PollOutcome` that folds into `PollStats`, emitted once per `STATUS_SUMMARY_INTERVAL` (1h, hardcoded) as a `status poll summary` INFO line (and a separate `watch poll summary` line from `watch_stats` over the same window; each line only when its list is non-empty) — `not_modified`/`updated` are the 304/200 split, and `forwarded`/`send_failed` split the Discord posts, so a broken webhook is visible in the summary and not only in the per-event `error!` — the only positive liveness signal the poller has, since a 304 poll logs at `debug` and quiet pages produce no INFO for days. Window arithmetic lives in `take_due_summary` so it is testable without a tracing subscriber; keep the logging out of it.
- **`fetch`** — the conditional JSON GET shared by `status` and `watch`: `fetch_json` sends `Accept: application/json` + `If-None-Match`, applies `FETCH_TIMEOUT` (5s), and streams the body through `read_capped` against `MAX_BODY` (8MB). One copy so the header, the timeout and the cap cannot drift between the two pollers. `Fetched<T>` is generic: `status` maps it to normalized incidents, `watch` keeps raw bytes. `StatusError` keeps its own variants/messages (documented in README) via `From<FetchError>`.
- **`watch`** — release feeds. `config::WatchKind` (`firefox` / `chrome` / `json`, a nested `source` table because `flatten` would disable `deny_unknown_fields`) → `resolve_source` → `SourceSpec { label, url, extractor }` at resolve time. **`Extractor` is the extension point**: each kind has its own wire struct (`FirefoxVersions`, `ChromeReleases`, no `deny_unknown_fields`), and `json` alone reads a `serde_json::Value` through an RFC 6901 pointer. `Release` holds only what the body said; channel, platform and link come from `RunWatch` when `build_message` runs. The embed timestamp is always the detection time — Chrome's `serving.startTime` is a field, because after a pulled rollout it belongs to an older release. The `json` extractor accepts strings and integers only: fractional numbers go through `f64` and `1.10` would equal `1.1`. `Extractor::extract`, `diff` and `read_feed` are pure. `diff` keys on the exact version string (no semver: formats differ and Chrome rollbacks must report), baseline on first call, records before send. `read_feed` runs every watch of a feed over one body and owns the ETag rule — stored **only when every watch could read the body**, otherwise a broken pointer would hide behind 304s — so the scheduler never touches the ETag beyond sending it.
- **`status`** — Atlassian Statuspage only. `StatusSource` trait + `Statuspage` impl fetches `/api/v2/incidents.json` through `fetch::fetch_json` (5s timeout — shorter than Discord's on purpose, it runs inside the tick — and a conditional `If-None-Match`). **The `Accept` header is load-bearing**: the CDN sends `Vary: Accept, Accept-Encoding` and answers 200 to every conditional request that omits it, silently disabling 304s. reqwest's `gzip` feature is enabled for the same `Vary` reason and because the feeds compress ~6-10x (measured: 296KB → 52KB for githubstatus); it matters most for instances that return **no `ETag`** at all — a Statuspage-compatible feed served from other infrastructure cannot be validated, so every poll downloads the full body. **Because gzip decouples decompressed size from transferred size, the body is never read with `Response::json`/`bytes`**: `read_capped` streams chunks and refuses past `MAX_BODY` (8MB, against 40-300KB real feeds), then `serde_json::from_slice` parses the capped buffer — hence `serde_json` is a direct dependency and `StatusError::Decode` wraps `serde_json::Error`, not `reqwest::Error`. The failure path uses `notifier::read_error_body`, shared with Discord, which stops at `MAX_ERROR_BODY` instead of buffering a whole HTML error page. Wire structs deliberately **do not** use `deny_unknown_fields` — they mirror a third-party API. `diff()` is a pure function over normalized `Incident`s and is where all the behaviour lives (cold-start baseline, per-update dedup, `min_impact` filter, pruning); keep HTTP out of it. `build_message` maps an event to the Discord embed — resolved is always green, whatever the impact.
- **`notifier`** — `Notifier` trait + `Discord` impl. `Discord::send` POSTs a `DiscordMessage` (content and/or embeds, plus `username`/`avatar_url`) with a 10s reqwest timeout. Reminders serialize to exactly `{"content": …}` as before. All Discord length limits are enforced in the `Embed`/`DiscordMessage` constructors, counted in `chars()` not bytes, so an over-long message cannot be built. On non-2xx, the body is read through `read_error_body`, which stops copying at `MAX_ERROR_BODY` (512 bytes) rather than buffering the response and truncating afterwards — `fetch::fetch_json` calls the same helper, so keep the cap in one place; both Discord and Statuspage answer failures with large HTML pages, and with the client's `gzip` feature on, the transferred size no longer bounds what `Response::bytes` would cost. The trait exists so scheduler tests can inject a counting fake.

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
