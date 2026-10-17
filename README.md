# chime

[![CI](https://github.com/m1sk9/chime/actions/workflows/ci.yaml/badge.svg)](https://github.com/m1sk9/chime/actions/workflows/ci.yaml)
[![Release chime](https://github.com/m1sk9/chime/actions/workflows/release.yaml/badge.svg)](https://github.com/m1sk9/chime/actions/workflows/release.yaml)
[![Apache License 2.0](https://img.shields.io/github/license/m1sk9/chime?color=%239944ee)](https://github.com/m1sk9/chime/blob/main/LICENSE)
[![codecov](https://codecov.io/gh/m1sk9/chime/graph/badge.svg?token=7FMYFTZ2XU)](https://codecov.io/gh/m1sk9/chime)


IaC-managed Discord webhook reminder. Reads a TOML schedule, runs as a long-lived process, and posts to Discord webhooks at the configured times. 

```sh
docker pull ghcr.io/m1sk9/chime:v0
```

- [Documentation](https://chime.m1sk9.dev)
- [llms.txt](https://chime.m1sk9.dev/llms.txt) — the documentation as plain Markdown, for LLMs

## Features

- Single static binary, no runtime dependencies.
- Strict, fail-fast config validation (unknown fields, bad timezones, missing secrets — all rejected at startup).
- Webhook URLs are kept out of `config.toml` and injected via environment variables (or `*_FILE` paths for Docker secrets).
- Forwards status page incidents — Atlassian Statuspage (Claude, Proton, GitHub, Discord, Cloudflare, …), Instatus, incident.io, Datadog Status Pages, or any JSON endpoint — to Discord as colour-coded embeds. Pull-based: no inbound port, no public ingress.
- Watches release feeds — Firefox, Chrome, or any JSON endpoint — and posts a new version as an embed. Pull-based, same as status pages.
- Subscribes to RSS, Atom and JSON feeds and posts each new entry as an embed. Pull-based, same as the others.
- Ships as a distroless container image to `ghcr.io/m1sk9/chime`.
- Built-in liveness check (`chime health`) usable from a distroless `HEALTHCHECK` — no shell or extra client needed.

chime can also watch status pages (Atlassian Statuspage, Instatus, incident.io, Datadog Status Pages, or any JSON endpoint) and forward incident updates to the same webhooks, watch the Firefox and Chrome release feeds — or any JSON endpoint — to announce new versions, and subscribe to RSS, Atom or JSON feeds to post new entries.

## Installation

See [Getting Started](https://chime.m1sk9.dev/docs/getting-started).

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
thread_id = "1234567890123456789"
                            # optional; post into this thread of the webhook's
                            # channel (quoted digits, see below)

[[status_pages]]
name = "claude"             # non-empty, unique within the file
kind = "statuspage"         # optional; statuspage | instatus | incidentio | datadog |
                            # custom (default: statuspage)
url = "https://status.claude.com"
                            # https only; the status page's base URL
                            # (`custom`: the JSON document itself)
webhook = "team"            # logical name — resolved via env, same as reminders
thread_id = "1234567890123456789"
                            # optional; same as for reminders
display_name = "Claude Status"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
poll_interval_sec = 300     # optional; 60..=3600 (default: 300)
min_impact = "minor"        # optional; none | maintenance | minor | major | critical
                            # (default: none — forward everything)

[[watches]]
name = "firefox"            # non-empty, unique within [[watches]]
webhook = "team"            # logical name — resolved via env, same as reminders
thread_id = "1234567890123456789"
                            # optional; same as for reminders
source = { kind = "firefox" }
                            # channel: release | esr | beta | devedition | nightly
                            # (default: release)
display_name = "Firefox Releases"
                            # optional; the Discord username on the message
                            # (default: the source label, e.g. `Firefox`)
poll_interval_sec = 3600    # optional; 60..=3600 (default: 3600)
avatar_url = "https://example.com/firefox.png"
                            # optional; https only

[[rss]]
name = "claude-code-changelog"
                            # non-empty, unique within [[rss]]
url = "https://code.claude.com/docs/en/changelog/rss.xml"
                            # https only; the feed URL itself
webhook = "team"            # logical name — resolved via env, same as reminders
thread_id = "1234567890123456789"
                            # optional; same as for reminders
display_name = "Claude Code Changelog"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
poll_interval_sec = 900     # optional; 60..=3600 (default: 900)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
```

See [Configuration](https://chime.m1sk9.dev/docs/configuration).

## LICENSE

chime is published under [Apache License 2.0](./LICENSE).

<sub>
    © 2026 m1sk9
</sub>
