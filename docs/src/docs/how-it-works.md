---
layout: doc
---

# How It Works

chime is a long-running process, not a one-shot cron job. The main loop:

1. Tick on `tick_interval_sec` (with `MissedTickBehavior::Skip` — overdue ticks are collapsed, not replayed).
2. Compute the current local time in the configured timezone.
3. For each reminder, fire if the current hour and minute match and today matches its schedule — one of `days` (weekday), or one of `day_of_month` (day of the current month).
4. Per-minute deduplication: each reminder fires at most once per matching minute, even if the tick interval is shorter than 60 seconds (e.g. with `tick_interval_sec = 30` you get exactly one POST per scheduled minute). The dedup record is updated **before** the HTTP request, so a send failure does not cause a retry within the same minute.
5. Poll **at most one** status page, watch or feed — the one most overdue among those whose `poll_interval_sec` has elapsed — and forward incident updates, versions or entries not seen before.
6. SIGINT and SIGTERM both trigger a clean shutdown.

## Polling

Status page polling follows the same rules as reminders:

- **The first poll after startup is silent.** Every document lists incidents that are already known — Statuspage's `incidents.json` the 50 most recent, Datadog's `config.json` the whole history, Instatus the active and recently resolved ones — so chime records them as a baseline and reports nothing. Only what changes *afterwards* is forwarded. A restart therefore re-baselines — it never replays history into the channel, in the same spirit as "a missed minute is a missed notification".
- The seen-record is written **before** the Discord request, so a failed send is not retried on the next poll.
- A status page being unreachable is logged at `warn` and retried on its own interval. chime never posts about its own polling failures.
- Because polling happens on the tick, an update is forwarded up to `poll_interval_sec` after the page published it. The embed timestamp shows the real publication time wherever the document carries one.
- **One page, watch or feed is polled per tick**, so a tick costs a single request no matter how many are configured — a set of unreachable endpoints cannot stall the loop long enough for `chime health` to call the heartbeat stale. Each entry takes `tick_interval_sec / poll_interval_sec` of that budget — with `tick_interval_sec = 60`, a page at 300 takes a fifth, a feed at 900 a fifteenth and a watch at 3600 a sixtieth — and every one stays on its nominal interval as long as the shares of all pages, watches and feeds add up to at most 1; beyond that they simply poll less often. Watches sharing a URL count once, at their shared interval.
- Requests are conditional (`If-None-Match`) and compressed (`Accept-Encoding: gzip`), so a page with no news usually costs a 304 with no body at all. An instance that returns **no `ETag`** — Instatus and incident.io pages, and some Statuspage-compatible feeds served from other infrastructure — cannot be validated, so every poll downloads the whole feed — as do Chrome watches and most RSS feeds. gzip keeps that in the single-digit kilobytes; nothing else is needed.

## Knowing the poller is alive

Once every hour the daemon logs one `status poll summary` line at `info`:

```json
{"timestamp":"2026-06-05T09:00:00.000000Z","level":"INFO","fields":{"message":"status poll summary","window_sec":3600,"pages":5,"polls":60,"not_modified":55,"updated":4,"failed":1,"forwarded":3,"send_failed":1},"target":"chime::scheduler"}
```

Without it, a working poller is silent: a page with no news answers 304, that path only logs at `debug`, and quiet status pages can go days without an incident. The summary makes "nothing is happening" distinguishable from "the poller is dead" without reading the container's network counters.

- `not_modified` and `updated` are HTTP outcomes — 304 and 200 — not a count of incidents that moved, so an instance that returns no `ETag` reports every poll as `updated` even when the feed is unchanged.
- `failed` counts fetch failures in the window (each is also logged at `warn` as it happens).
- `forwarded` counts Discord posts that succeeded, and `send_failed` counts those Discord rejected (each also logged at `error`) — a nonzero `send_failed` is the difference between a page with no news and a webhook that stopped accepting posts.

Set `log_level = "debug"` for the per-poll detail.

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

::: warning Implications for non-Docker users

- chime does not daemonize itself, does not write a PID file, and does not fork. Run it under a supervisor (`systemd`, `launchd`, `runit`, ...) that restarts it on crash and exit.
- The process is single-threaded (`tokio` current-thread runtime). It is cheap to leave running.
- A missed minute is a missed notification — there is no catch-up. If the host is asleep at 09:30 the 09:30 reminder will not fire when it wakes. This matches a cron-style mental model.
- Logs are line-delimited JSON on stdout. Capture them with whatever your supervisor exposes (`journalctl -u chime`, container log drivers, etc.).

:::
