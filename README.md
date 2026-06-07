# chime

IaC-managed Discord webhook reminder. Reads a TOML schedule, runs as a long-lived process, and posts to Discord webhooks at the configured times.

## Features

- Single static binary, no runtime dependencies.
- Strict, fail-fast config validation (unknown fields, bad timezones, missing secrets — all rejected at startup).
- Webhook URLs are kept out of `config.toml` and injected via environment variables (or `*_FILE` paths for Docker secrets).
- Ships as a distroless container image to `ghcr.io/m1sk9/chime`.

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
```

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
- Empty `message`
- Webhook env var unset, empty, or not a valid URL
- Zero reminders defined

## How it works

chime is a long-running process, not a one-shot cron job. The main loop:

1. Tick on `tick_interval_sec` (with `MissedTickBehavior::Skip` — overdue ticks are collapsed, not replayed).
2. Compute the current local time in the configured timezone.
3. For each reminder, fire if the current hour and minute match and today is one of `days`.
4. Per-minute deduplication: each reminder fires at most once per matching minute, even if the tick interval is shorter than 60 seconds (e.g. with `tick_interval_sec = 30` you get exactly one POST per scheduled minute). The dedup record is updated **before** the HTTP request, so a send failure does not cause a retry within the same minute.
5. SIGINT and SIGTERM both trigger a clean shutdown.

> [!IMPORTANT]
>
> Implications for non-Docker users:
> 
> - chime does not daemonize itself, does not write a PID file, and does not fork. Run it under a supervisor (`systemd`, `launchd`, `runit`, ...) that restarts it on crash and exit.
> - The process is single-threaded (`tokio` current-thread runtime). It is cheap to leave running.
> - A missed minute is a missed notification — there is no catch-up. If the host is asleep at 09:30 the 09:30 reminder will not fire when it wakes. This matches a cron-style mental model.
> - Logs are line-delimited JSON on stdout. Capture them with whatever your supervisor exposes (`journalctl -u chime`, container log drivers, etc.).

## LICENSE

chime is published under [Apache License 2.0](./LICENSE).

<sub>
    © 2026 m1sk9
</sub>
