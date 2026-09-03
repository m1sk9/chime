# chime

IaC-managed Discord webhook reminder. Reads a TOML schedule, runs as a long-lived process, and posts to Discord webhooks at the configured times. It can also watch Atlassian Statuspage instances and forward incident updates to the same webhooks.

## Features

- Single static binary, no runtime dependencies.
- Strict, fail-fast config validation (unknown fields, bad timezones, missing secrets — all rejected at startup).
- Webhook URLs are kept out of `config.toml` and injected via environment variables (or `*_FILE` paths for Docker secrets).
- Forwards Atlassian Statuspage incidents (Claude, Proton, GitHub, Discord, Cloudflare, …) to Discord as colour-coded embeds. Pull-based: no inbound port, no public ingress.
- Ships as a distroless container image to `ghcr.io/m1sk9/chime`.
- Built-in liveness check (`chime health`) usable from a distroless `HEALTHCHECK` — no shell or extra client needed.

## Setup

### With Docker Compose (recommended)

1. Copy the example files:

    ```sh
    cp config.toml.example config.toml
    cp .env.example .env
    ```

2. Edit `config.toml` with your reminders and `.env` with your webhook URLs.
3. Start it:

    ```sh
    docker compose up -d
    ```

Compose pulls `ghcr.io/m1sk9/chime:latest` by default. To pin a specific version, change the `image:` tag in `docker-compose.yml` (any of `vX`, `vX.Y`, `vX.Y.Z`, `latest`, or a commit SHA are published per release).

To build the image locally instead, uncomment the `build:` block in `docker-compose.yml` and remove the `image:` line.

#### Docker secrets (alternative to `.env`)

Swap `env_file:` for the `secrets:` block shown commented in `docker-compose.yml`, and reference each secret via the `_FILE` convention:

```yaml
environment:
  CHIME_WEBHOOK_TEAM_FILE: /run/secrets/chime_webhook_team
```

chime reads the file path from `<KEY>_FILE` first; if unset it falls back to `<KEY>`. Whitespace around the value is trimmed.

### Without Docker

You will need to keep chime alive yourself — see [How it works](#how-it-works) below for what that entails.

1. Build the binary (see [Build](#build)).
2. Install the binary somewhere on your `PATH`, e.g. `/usr/local/bin/chime`.
3. Place your config at `/etc/chime/config.toml` (or set `CHIME_CONFIG` to another path).
4. Export the webhook env vars (`CHIME_WEBHOOK_<NAME>=https://...`).
5. Run it under a process supervisor. Example systemd unit:

    ```ini
    # /etc/systemd/system/chime.service
    [Unit]
    Description=chime Discord reminder
    After=network-online.target
    Wants=network-online.target

    [Service]
    ExecStart=/usr/local/bin/chime
    Environment=CHIME_CONFIG=/etc/chime/config.toml
    EnvironmentFile=/etc/chime/secrets.env
    Restart=on-failure
    RestartSec=5
    User=chime
    Group=chime

    [Install]
    WantedBy=multi-user.target
    ```

    Then `systemctl daemon-reload && systemctl enable --now chime`.

## Build

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

## Configuration

Config is a single TOML file. The default path is `/etc/chime/config.toml`; override with the `CHIME_CONFIG` env var.

```toml
[system]
log_level = "info"          # debug | info | warn | error (default: info)
tick_interval_sec = 30      # 1..=60
timezone = "Asia/Tokyo"     # any IANA name

[[reminders]]
name = "daily-standup"      # non-empty, unique within the file
time = "09:30"              # HH:MM, 24-hour
days = ["mon", "tue", "wed", "thu", "fri"]
                            # sun/mon/tue/wed/thu/fri/sat, or ["every"]
message = "Time for standup."
webhook = "team"            # logical name — resolved via env (see below)

[[reminders]]
name = "salary-day"
time = "15:00"
day_of_month = [18]         # 1..=31; e.g. [1, 15] for multiple days each month
message = "Payday is here."
webhook = "team"

[[status_pages]]
name = "claude"             # non-empty, unique within the file
url = "https://status.claude.com"
                            # https only; the status page's base URL
webhook = "team"            # logical name — resolved via env, same as reminders
display_name = "Claude Status"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
poll_interval_sec = 300     # optional; 60..=3600 (default: 300)
min_impact = "minor"        # optional; none | maintenance | minor | major | critical
                            # (default: none — forward everything)
```

Each reminder schedules by **either** `days` (weekdays) **or** `day_of_month` (days of the month) — exactly one of the two, never both. `day_of_month` accepts a list of days in `1..=31`; a day that does not exist in a given month (e.g. `31` in February) is simply skipped that month.

A config must define at least one `[[reminders]]` **or** one `[[status_pages]]`; either section alone is fine.

### Status pages

`[[status_pages]]` watches an [Atlassian Statuspage](https://www.atlassian.com/software/statuspage) instance — the software behind `status.claude.com`, `status.proton.me`, `www.githubstatus.com`, `discordstatus.com`, `www.cloudflarestatus.com` and many others. `url` is the page's base URL; chime appends `/api/v2/incidents.json` itself.

chime **polls** that endpoint — it does not receive an inbound webhook. Nothing needs to be exposed, no ports are opened, and no subscription has to be registered out-of-band, so a page is added by editing `config.toml` alone. Requests are conditional (`If-None-Match`), so an unchanged page costs a `304` and no body.

Only Atlassian Statuspage is supported. A URL that is not a Statuspage instance fails at the first poll with `response is not an Atlassian Statuspage incidents feed` — this is logged, not fatal.

The unit of notification is an **incident update**, not an incident: `Investigating → Identified → Monitoring → Resolved` produces four messages, each a separate post rather than an edit of the first. `min_impact` drops incidents below the given severity; an incident whose severity Statuspage reports with a value chime does not recognise is always forwarded rather than silently dropped.

#### What it looks like in Discord

Each update is one embed. The colour bar is the severity at a glance:

| Condition | Colour | Emoji |
|---|---|---|
| Resolved (any severity) | green | ✅ |
| `critical` | red | 🔍 / 🎯 / 👀 by state |
| `major` | orange | ↑ |
| `minor` | yellow | ↑ |
| `none`, or an unrecognised severity | grey | ↑ |

A resolved incident is green regardless of how severe it was, so "this is fixed" never arrives wearing a red bar. The embed carries the incident title (linked to the Statuspage short link), the latest update's text, `Status` / `Impact` / `Components` fields, the status page host as the footer, and the update's own timestamp — which is when Statuspage published it, not when chime posted it.

The `Status` field of one real incident renders as `✅ Resolved`, `Impact` as `Minor`, and the message is attributed to `display_name` so several status pages can share one Discord channel and still be told apart.

Long bodies are truncated (postmortems run to thousands of characters); the linked incident page is the authoritative copy.

### Webhook resolution

The `webhook` field is a logical name, not a URL. At startup chime derives an env key from it:

| `webhook` value | env key |
|---|---|
| `team` | `CHIME_WEBHOOK_TEAM` |
| `on-call` | `CHIME_WEBHOOK_ON_CALL` |
| `ops.alpha` | `CHIME_WEBHOOK_OPS_ALPHA` |

Non-alphanumeric characters are mapped to `_` and the result is uppercased. For each env key chime tries `<KEY>_FILE` first (for Docker secrets) and falls back to `<KEY>`. The value must be a valid URL after trimming.

### Validation

All of the following are rejected at startup with a descriptive error and a non-zero exit code — chime never partially starts:

- Unknown fields anywhere in the TOML
- `tick_interval_sec` outside `1..=60`
- Unknown IANA timezone
- Duplicate or empty reminder `name`
- `time` not in `HH:MM` form, or hour > 23 / minute > 59
- Empty `days`, or any unknown weekday string
- Empty `day_of_month`, or any value outside `1..=31`
- A reminder specifying neither or both of `days` / `day_of_month`
- Empty `message`
- Webhook env var unset, empty, or not a valid URL
- Neither reminders nor status pages defined
- Duplicate or empty status page `name`
- Status page `url` or `avatar_url` that is not `https`, or has no host
- `poll_interval_sec` outside `60..=3600`
- `min_impact` that is not one of `none` / `maintenance` / `minor` / `major` / `critical`

## How it works

chime is a long-running process, not a one-shot cron job. The main loop:

1. Tick on `tick_interval_sec` (with `MissedTickBehavior::Skip` — overdue ticks are collapsed, not replayed).
2. Compute the current local time in the configured timezone.
3. For each reminder, fire if the current hour and minute match and today matches its schedule — one of `days` (weekday), or one of `day_of_month` (day of the current month).
4. Per-minute deduplication: each reminder fires at most once per matching minute, even if the tick interval is shorter than 60 seconds (e.g. with `tick_interval_sec = 30` you get exactly one POST per scheduled minute). The dedup record is updated **before** the HTTP request, so a send failure does not cause a retry within the same minute.
5. Poll **at most one** status page — the one most overdue among those whose `poll_interval_sec` has elapsed — and forward incident updates not seen before.
6. SIGINT and SIGTERM both trigger a clean shutdown.

Status page polling follows the same rules as reminders:

- **The first poll after startup is silent.** `incidents.json` returns the 50 most recent incidents, so chime records them as a baseline and reports nothing. Only what changes *afterwards* is forwarded. A restart therefore re-baselines — it never replays history into the channel, in the same spirit as "a missed minute is a missed notification".
- The seen-record is written **before** the Discord request, so a failed send is not retried on the next poll.
- A status page being unreachable is logged at `warn` and retried on its own interval. chime never posts about its own polling failures.
- Because polling happens on the tick, an update is forwarded up to `poll_interval_sec` after Statuspage published it. The embed timestamp always shows the real publication time.
- **One page is polled per tick**, so a tick costs a single request no matter how many pages are configured — a set of unreachable pages cannot stall the loop long enough for `chime health` to call the heartbeat stale. Configure at most `poll_interval_sec / tick_interval_sec` pages to keep every page on its nominal interval; beyond that they simply poll less often.

> [!IMPORTANT]
>
> Implications for non-Docker users:
> 
> - chime does not daemonize itself, does not write a PID file, and does not fork. Run it under a supervisor (`systemd`, `launchd`, `runit`, ...) that restarts it on crash and exit.
> - The process is single-threaded (`tokio` current-thread runtime). It is cheap to leave running.
> - A missed minute is a missed notification — there is no catch-up. If the host is asleep at 09:30 the 09:30 reminder will not fire when it wakes. This matches a cron-style mental model.
> - Logs are line-delimited JSON on stdout. Capture them with whatever your supervisor exposes (`journalctl -u chime`, container log drivers, etc.).

## Health check

`docker ps` only tells you the process hasn't crashed — a hung scheduler loop looks identical to a healthy one. chime exposes a liveness signal that answers **"is the scheduler actually ticking?"**, not "did the last Discord send succeed?".

How it works:

- On every tick the daemon writes the current timestamp to a heartbeat file (default `/tmp/chime.heartbeat`, override with `CHIME_HEARTBEAT_PATH`). The write happens **before** any Discord request, so the signal is independent of network reachability.
- The `chime health` subcommand reads that file's mtime and exits `0` when it is fresh — `now - mtime <= 2 * tick_interval_sec` — and non-zero with a one-line stderr message otherwise (stale, missing, or unreadable). It reads the tick interval from the same `CHIME_CONFIG`, and does **not** require any webhook env var.

The container image already wires this into a `HEALTHCHECK` (exec-form, since distroless has no shell), so `docker ps` / `docker inspect` report health automatically. To set it explicitly in `docker-compose.yml`:

```yaml
healthcheck:
  test: ["CMD", "/usr/local/bin/chime", "health"]
  interval: 30s
  timeout: 5s
  start_period: 10s
  retries: 3
```

Without Docker you can call `chime health` from any supervisor or monitoring probe — its exit code is the contract.

## LICENSE

chime is published under [Apache License 2.0](./LICENSE).

<sub>
    © 2026 m1sk9
</sub>
