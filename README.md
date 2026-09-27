# chime

IaC-managed Discord webhook reminder. Reads a TOML schedule, runs as a long-lived process, and posts to Discord webhooks at the configured times. It can also watch Atlassian Statuspage instances and forward incident updates to the same webhooks, watch the Firefox and Chrome release feeds — or any JSON endpoint — to announce new versions, and subscribe to RSS, Atom or JSON feeds to post new entries.

## Features

- Single static binary, no runtime dependencies.
- Strict, fail-fast config validation (unknown fields, bad timezones, missing secrets — all rejected at startup).
- Webhook URLs are kept out of `config.toml` and injected via environment variables (or `*_FILE` paths for Docker secrets).
- Forwards Atlassian Statuspage incidents (Claude, Proton, GitHub, Discord, Cloudflare, …) to Discord as colour-coded embeds. Pull-based: no inbound port, no public ingress.
- Watches release feeds — Firefox, Chrome, or any JSON endpoint — and posts a new version as an embed. Pull-based, same as status pages.
- Subscribes to RSS, Atom and JSON feeds and posts each new entry as an embed. Pull-based, same as the others.
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

[[watches]]
name = "firefox"            # non-empty, unique within [[watches]]
webhook = "team"            # logical name — resolved via env, same as reminders
source = { kind = "firefox" }
                            # channel: release | esr | beta | devedition | nightly
                            # (default: release)
display_name = "Firefox Releases"
                            # optional; the Discord username on the message
                            # (default: the source label, e.g. `Firefox`)
poll_interval_sec = 3600    # optional; 60..=3600 (default: 3600)
avatar_url = "https://example.com/firefox.png"
                            # optional; https only

[[watches]]
name = "chrome"
webhook = "team"
source = { kind = "chrome", platform = "win", channel = "stable", rollout = "started" }
                            # platform: win | win64 | mac | mac_arm64 | linux |
                            #           android | webview | ios (default: win)
                            # channel: stable | extended | beta | dev | canary
                            #          (default: stable)
                            # rollout: started | complete (default: started)

[[watches]]
name = "node"
webhook = "team"
source = { kind = "json", url = "https://nodejs.org/dist/index.json", pointer = "/0/version", link = "https://nodejs.org/en/blog/release" }
                            # url: https only. pointer: RFC 6901, must start with `/`.
                            # link: optional; https only; put on the embed as-is.

[[rss]]
name = "claude-code-changelog"
                            # non-empty, unique within [[rss]]
url = "https://code.claude.com/docs/en/changelog/rss.xml"
                            # https only; the feed URL itself
webhook = "team"            # logical name — resolved via env, same as reminders
display_name = "Claude Code Changelog"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
poll_interval_sec = 900     # optional; 60..=3600 (default: 900)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
```

Each reminder schedules by **either** `days` (weekdays) **or** `day_of_month` (days of the month) — exactly one of the two, never both. `day_of_month` accepts a list of days in `1..=31`; a day that does not exist in a given month (e.g. `31` in February) is simply skipped that month.

A config must define at least one `[[reminders]]`, `[[status_pages]]`, `[[watches]]` **or** `[[rss]]`; any section alone is fine.

### Status pages

`[[status_pages]]` watches an [Atlassian Statuspage](https://www.atlassian.com/software/statuspage) instance — the software behind `status.claude.com`, `status.proton.me`, `www.githubstatus.com`, `discordstatus.com`, `www.cloudflarestatus.com` and many others. `url` is the page's base URL; chime appends `/api/v2/incidents.json` itself.

chime **polls** that endpoint — it does not receive an inbound webhook. Nothing needs to be exposed, no ports are opened, and no subscription has to be registered out-of-band, so a page is added by editing `config.toml` alone. Requests are conditional (`If-None-Match`), so an unchanged page costs a `304` and no body.

Only Atlassian Statuspage is supported. A URL that is not a Statuspage instance fails at the first poll with `response is not an Atlassian Statuspage incidents feed` — this is logged, not fatal.

A feed is read up to a hard 8 MB ceiling and refused past it with `status page body exceeded 8388608 bytes`. Real feeds measure 40-300 KB, so this only fires on a page that has gone wrong: responses are gzip-encoded, and gzip lets a small download expand into an arbitrarily large buffer, so the limit is on what is decompressed rather than on what is transferred. Like every other polling failure it is logged and retried on the next interval.

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

### Watches

`[[watches]]` polls a release feed and posts when the version it reads there changes. `source` picks the feed; `source = { kind = "firefox" }` and a `[watches.source]` sub-table mean the same thing.

| `kind` | Feed |
|---|---|
| `firefox` | Mozilla product-details, `https://product-details.mozilla.org/1.0/firefox_versions.json` |
| `chrome` | Chrome [VersionHistory API](https://developer.chrome.com/docs/web-platform/versionhistory/reference), `https://versionhistory.googleapis.com/v1/chrome/platforms/{platform}/channels/{channel}/versions/all/releases` |
| `json` | Any https JSON endpoint you name, read through a JSON pointer |

Safari is not supported: Apple publishes no machine-readable release feed.

#### `firefox`

`channel` selects which product-details key is watched, and where the embed links:

| `channel` | Key | Link |
|---|---|---|
| `release` (default) | `LATEST_FIREFOX_VERSION` | `https://www.firefox.com/firefox/{version}/releasenotes/` |
| `esr` | `FIREFOX_ESR` | same, with the trailing `esr` dropped (`140.16.0esr` → `140.16.0`) |
| `beta` | `LATEST_FIREFOX_RELEASED_DEVEL_VERSION` | same, with `b5` rewritten to `beta` (`157.0b5` → `157.0beta`) |
| `devedition` | `FIREFOX_DEVEDITION` | as for `beta` |
| `nightly` | `FIREFOX_NIGHTLY` | `https://www.firefox.com/firefox/nightly/notes/` |

The feed supports conditional requests, so an unchanged feed costs a `304`. An empty value for the selected channel is a polling failure, not a version.

#### `chrome`

`platform` and `channel` are inserted into the API path as-is. `rollout` decides which version counts as released:

- `started` (default) — the top version being served, as soon as its staged rollout begins, even at 0.5%. A pulled rollout makes the top version go *down*, and that is reported too.
- `complete` — only the top version served to 100% of users.

A change in rollout fraction alone is not a new release and posts nothing. The VersionHistory API returns no `ETag`, so every poll downloads the response — only the top release is requested, well under a kilobyte — and is counted as `updated` in the [summary](#knowing-the-poller-is-alive). The embed links to the channel's label on the Chrome Releases blog (canary, which is not announced there, links to the blog itself).

#### `json`

chime does not know the shape of the document: `pointer` ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)) names the one value that is the version. Segments are separated by `/`, array elements are addressed by index, and `~0` / `~1` escape `~` / `/`. The pointer is used exactly as written — whitespace is part of a key, so `/v ` and `/v` are different pointers. The value must be a string or an integer; a fractional number is refused, because `1.10` would read back as `1.1` — serve such a version as a string. Searching inside an array or combining several fields is not possible.

| Feed | `url` | `pointer` |
|---|---|---|
| Node.js | `https://nodejs.org/dist/index.json` | `/0/version` |
| A GitHub repository | `https://api.github.com/repos/{owner}/{repo}/releases/latest` | `/tag_name` |

A pointer that does not match the document is not caught at startup: like a status page URL that is not a Statuspage instance, it fails at the first poll with a `warn` and keeps failing each interval.

#### Notification semantics

- **The first poll after startup is silent** — it records the current version as a baseline. A restart re-baselines.
- A post is sent whenever the version string **differs** from the last one seen, including when it goes down. There is no version ordering: `140.16.0esr`, `157.0b5` and `v24.9.0` do not share one.
- The last-seen version is updated **before** the Discord request, so a failed send is not retried.
- Polling failures (unreachable feed, non-JSON body, pointer that matches nothing) are logged at `warn` and never posted.
- **Watches that read the same URL share one request** — several Firefox channels, or two `json` watches with different pointers on one endpoint. The shared request runs at the shortest `poll_interval_sec` among them, and one watch failing to read the body does not stop the others from reporting.

#### What it looks like in Discord

Each new version is one embed, attributed to `display_name`, with the feed's host as the footer. The colour is fixed (Discord blurple) — a release has no severity.

```
Firefox 156.0.1                          ← links to the release notes
Previous: 156.0        Channel: Release
Next release: 2026-10-09                  ← release channel only
Channels: Release 156.0.1 · ESR 140.16.0esr · Beta 157.0b5 · Developer Edition 157.0b5 · Nightly 159.0a1
product-details.mozilla.org · <time chime saw it>
```

```
Chrome Stable 155.0.8059.12              ← links to the Chrome Releases blog
Previous: 154.0.8037.58   Channel: Stable   Platform: win
Rollout: 0.5%             Milestone: 155   Serving since: 2026-09-23 18:50 UTC
versionhistory.googleapis.com · <time chime saw it>
```

```
node v24.9.0                             ← links to `link`, if set
Previous: v24.8.0
nodejs.org · <time chime saw it>
```

The timestamp is always when chime noticed the change — up to `poll_interval_sec` after the release. product-details and arbitrary JSON carry no publication time at all. Chrome does, but after a pulled rollout the served version is an older release whose start time would date the post days in the past, so it is shown as `Serving since` instead.

### RSS feeds

`[[rss]]` subscribes to a syndication feed and posts every entry that was not in it before. RSS 0.9x / 1.0 / 2.0, Atom and JSON Feed are all accepted; the format is detected from the body, so there is no `kind` to set. `url` is the feed itself and is requested as written — chime appends nothing. A body that is none of these fails the poll with `response is not an RSS, Atom or JSON feed: …` — this is logged, not fatal.

#### Notification semantics

- **The first poll after startup is silent** — it records the entries currently in the feed as a baseline. A restart re-baselines.
- An entry is posted when its **id** has not been seen: `<guid>` in RSS, `<id>` in Atom, `id` in JSON Feed, or, when the feed gives none, a hash of the entry's link and title. Editing an entry that was already seen does not post it again.
- The id is recorded **before** the Discord request, so a failed send is not retried.
- An entry that drops out of the feed is forgotten; if it comes back, it is reported as new. A feed that answers with no entries at all keeps the baseline.
- An entry with neither a link nor a title is ignored: it has nothing stable to be recognised by.
- Several new entries in one poll are posted **oldest first**, by the entry's own date; entries without a date come last, in feed order. There is no cap on how many are posted.
- Polling failures (unreachable feed, a body that is not a feed) are logged at `warn` and never posted.

Many feeds — the Claude Code changelog among them — send `cache-control: no-cache` and no `ETag`, so they cannot be validated and every poll downloads the whole feed and is counted as `updated` in the [summary](#knowing-the-poller-is-alive), as for Chrome. gzip keeps that small, and the same 8 MB ceiling as for status pages applies.

#### What it looks like in Discord

Each new entry is one embed, attributed to `display_name`. The colour is fixed (Discord fuchsia).

```
2.1.283                                  ← the entry title, linked to the entry
- Added x-claude-code-prompt-id to the gateway hint headers …
- Fixed MCP progress notifications being discarded …
Published: 2026-09-25 22:00 UTC          ← if the entry is dated
code.claude.com · <time chime saw it>
```

The body is the entry's summary, or its content when there is no summary, rendered from HTML to plain text: list items become `- ` lines, paragraphs and line breaks become newlines, every other tag is dropped and its text kept. Long bodies are truncated; the linked page is the authoritative copy. The timestamp is when chime noticed the entry, not the entry's own date.

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
- Neither reminders, status pages, watches nor rss feeds defined
- Duplicate or empty status page `name`
- Status page `url` or `avatar_url` that is not `https`, or has no host
- `poll_interval_sec` outside `60..=3600`
- `min_impact` that is not one of `none` / `maintenance` / `minor` / `major` / `critical`
- Duplicate or empty watch `name`
- `source.kind` that is not `firefox` / `chrome` / `json`, or an unknown key inside `source`
- `firefox.channel` / `chrome.platform` / `chrome.channel` / `chrome.rollout` outside their listed values
- `chrome` watch with `channel = "extended"` on `linux`, `android`, `webview` or `ios` — Extended Stable exists only for `win`, `win64`, `mac` and `mac_arm64`, and the API rejects the rest
- `json` watch whose `url` / `link` is not https, or whose `pointer` does not start with `/`
- Duplicate or empty rss `name`
- rss `url` or `avatar_url` that is not `https`, or has no host

## How it works

chime is a long-running process, not a one-shot cron job. The main loop:

1. Tick on `tick_interval_sec` (with `MissedTickBehavior::Skip` — overdue ticks are collapsed, not replayed).
2. Compute the current local time in the configured timezone.
3. For each reminder, fire if the current hour and minute match and today matches its schedule — one of `days` (weekday), or one of `day_of_month` (day of the current month).
4. Per-minute deduplication: each reminder fires at most once per matching minute, even if the tick interval is shorter than 60 seconds (e.g. with `tick_interval_sec = 30` you get exactly one POST per scheduled minute). The dedup record is updated **before** the HTTP request, so a send failure does not cause a retry within the same minute.
5. Poll **at most one** status page, watch or feed — the one most overdue among those whose `poll_interval_sec` has elapsed — and forward incident updates, versions or entries not seen before.
6. SIGINT and SIGTERM both trigger a clean shutdown.

Status page polling follows the same rules as reminders:

- **The first poll after startup is silent.** `incidents.json` returns the 50 most recent incidents, so chime records them as a baseline and reports nothing. Only what changes *afterwards* is forwarded. A restart therefore re-baselines — it never replays history into the channel, in the same spirit as "a missed minute is a missed notification".
- The seen-record is written **before** the Discord request, so a failed send is not retried on the next poll.
- A status page being unreachable is logged at `warn` and retried on its own interval. chime never posts about its own polling failures.
- Because polling happens on the tick, an update is forwarded up to `poll_interval_sec` after Statuspage published it. The embed timestamp always shows the real publication time.
- **One page, watch or feed is polled per tick**, so a tick costs a single request no matter how many are configured — a set of unreachable endpoints cannot stall the loop long enough for `chime health` to call the heartbeat stale. Each entry takes `tick_interval_sec / poll_interval_sec` of that budget — with `tick_interval_sec = 60`, a page at 300 takes a fifth, a feed at 900 a fifteenth and a watch at 3600 a sixtieth — and every one stays on its nominal interval as long as the shares of all pages, watches and feeds add up to at most 1; beyond that they simply poll less often. Watches sharing a URL count once, at their shared interval.
- Requests are conditional (`If-None-Match`) and compressed (`Accept-Encoding: gzip`), so a page with no news usually costs a 304 with no body at all. An instance that returns **no `ETag`** — some Statuspage-compatible feeds are served from other infrastructure and do not — cannot be validated, so every poll downloads the whole feed — as do Chrome watches and most RSS feeds. gzip keeps that in the single-digit kilobytes; nothing else is needed.

### Knowing the poller is alive

Once every hour the daemon logs one `status poll summary` line at `info`:

```json
{"timestamp":"2026-06-05T09:00:00.000000Z","level":"INFO","fields":{"message":"status poll summary","window_sec":3600,"pages":5,"polls":60,"not_modified":55,"updated":4,"failed":1,"forwarded":3,"send_failed":1},"target":"chime::scheduler"}
```

Without it, a working poller is silent: a page with no news answers 304, that path only logs at `debug`, and quiet status pages can go days without an incident. The summary makes "nothing is happening" distinguishable from "the poller is dead" without reading the container's network counters. `not_modified` and `updated` are HTTP outcomes — 304 and 200 — not a count of incidents that moved, so an instance that returns no `ETag` reports every poll as `updated` even when the feed is unchanged. `failed` counts fetch failures in the window (each is also logged at `warn` as it happens), `forwarded` counts Discord posts that succeeded, and `send_failed` counts those Discord rejected (each also logged at `error`) — a nonzero `send_failed` is the difference between a page with no news and a webhook that stopped accepting posts. Set `log_level = "debug"` for the per-poll detail.

Watches get their own `watch poll summary` line over the same window, with the same fields except that `pages` becomes `watches` (the number configured):

```json
{"timestamp":"2026-06-05T09:00:00.000000Z","level":"INFO","fields":{"message":"watch poll summary","window_sec":3600,"watches":2,"polls":2,"not_modified":1,"updated":1,"failed":0,"forwarded":0,"send_failed":0},"target":"chime::scheduler"}
```

As for status pages, `polls`, `not_modified` and `updated` count requests — watches sharing a URL are one request — and `updated` counts 200 responses, not versions that changed, so a Chrome watch reports every poll as `updated`. `failed` also covers a body that was fetched but that at least one watch on it could not read; the posts of the watches that could read it are still counted in `forwarded` / `send_failed`. 
RSS feeds get a third line, `rss poll summary`, with `feeds` (the number configured) in place of `pages`:

```json
{"timestamp":"2026-06-05T09:00:00.000000Z","level":"INFO","fields":{"message":"rss poll summary","window_sec":3600,"feeds":1,"polls":4,"not_modified":0,"updated":4,"failed":0,"forwarded":1,"send_failed":0},"target":"chime::scheduler"}
```

A feed that returns no `ETag` reports every poll as `updated`, and `failed` includes a body that was fetched but is not a feed. Each line is omitted when its list is empty.

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

- On every tick the daemon writes the current timestamp to a heartbeat file (default `/tmp/chime.heartbeat`, override with `CHIME_HEARTBEAT_PATH`). The write happens **before** any Discord request, so the signal is independent of network reachability, and is repeated after every Discord request, so a tick that sends several messages to a slow Discord is not mistaken for a hung one.
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
