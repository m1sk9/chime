---
layout: doc
---

# Configuration

Config is a single TOML file. The default path is `/etc/chime/config.toml`; override with the `CHIME_CONFIG` env var. [`config/config.toml.example`](https://github.com/m1sk9/chime/blob/main/config/config.toml.example) in the repository is a complete starting point.

A config must define at least one `[[reminders]]`, `[[status_pages]]`, `[[watches]]` **or** `[[rss]]`; any section alone is fine. Unknown keys are rejected, and every problem is reported at startup — see [Validation](./validation).

## `[system]`

```toml
[system]
log_level = "info"          # debug | info | warn | error (default: info)
tick_interval_sec = 30      # 1..=60
timezone = "Asia/Tokyo"     # any IANA name
```

## `[[reminders]]`

```toml
[[reminders]]
name = "daily-standup"      # non-empty, unique within the file
time = "09:30"              # HH:MM, 24-hour
days = ["mon", "tue", "wed", "thu", "fri"]
                            # sun/mon/tue/wed/thu/fri/sat, or ["every"]
message = "Time for standup."
webhook = "team"            # logical name — resolved via env (see Webhooks)
thread_id = "1234567890123456789"
                            # optional; post into this thread of the webhook's
                            # channel (quoted digits)

[[reminders]]
name = "salary-day"
time = "15:00"
day_of_month = [18]         # 1..=31; e.g. [1, 15] for multiple days each month
message = "Payday is here."
webhook = "team"
```

Each reminder schedules by **either** `days` (weekdays) **or** `day_of_month` (days of the month) — exactly one of the two, never both. `day_of_month` accepts a list of days in `1..=31`; a day that does not exist in a given month (e.g. `31` in February) is simply skipped that month.

`time` is wall-clock time in `system.timezone`. On a daylight-saving change, a time that occurs twice (the hour that falls back) fires once, at its first occurrence, and a time that does not exist that day (the hour that springs forward) does not fire.

`webhook` and `thread_id` are explained in [Webhooks](./webhooks).

## `[[status_pages]]`

```toml
[[status_pages]]
name = "claude"             # non-empty, unique within the file
kind = "statuspage"         # optional; statuspage | instatus | incidentio | datadog |
                            # custom (default: statuspage)
url = "https://status.claude.com"
                            # https only; the status page's base URL
                            # (`custom`: the JSON document itself)
webhook = "team"
thread_id = "1234567890123456789"
                            # optional
display_name = "Claude Status"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
poll_interval_sec = 300     # optional; 60..=3600 (default: 300)
min_impact = "minor"        # optional; none | maintenance | minor | major | critical
                            # (default: none — forward everything)
```

See [Status Pages](./features/status-pages).

## `[[watches]]`

```toml
[[watches]]
name = "firefox"            # non-empty, unique within [[watches]]
webhook = "team"
thread_id = "1234567890123456789"
                            # optional
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
```

See [Release Watches](./features/watches).

## `[[rss]]`

```toml
[[rss]]
name = "claude-code-changelog"
                            # non-empty, unique within [[rss]]
url = "https://code.claude.com/docs/en/changelog/rss.xml"
                            # https only; the feed URL itself
webhook = "team"
thread_id = "1234567890123456789"
                            # optional
display_name = "Claude Code Changelog"
                            # optional; the Discord username on the message
                            # (default: the `name` above)
poll_interval_sec = 900     # optional; 60..=3600 (default: 900)
avatar_url = "https://example.com/claude.png"
                            # optional; https only
```

See [RSS Feeds](./features/rss).
