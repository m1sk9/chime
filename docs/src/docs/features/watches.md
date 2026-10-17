---
layout: doc
---

# Release Watches

`[[watches]]` polls a release feed and posts when the version it reads there changes. `source` picks the feed; `source = { kind = "firefox" }` and a `[watches.source]` sub-table mean the same thing. The keys are listed in [Configuration](../configuration#watches).

| `kind`    | Feed                                                                                                                                                                                                    |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `firefox` | Mozilla product-details, `https://product-details.mozilla.org/1.0/firefox_versions.json`                                                                                                                |
| `chrome`  | Chrome [VersionHistory API](https://developer.chrome.com/docs/web-platform/versionhistory/reference), `https://versionhistory.googleapis.com/v1/chrome/platforms/{platform}/channels/{channel}/versions/all/releases` |
| `json`    | Any https JSON endpoint you name, read through a JSON pointer                                                                                                                                           |

Safari is not supported: Apple publishes no machine-readable release feed.

## `firefox`

`channel` selects which product-details key is watched, and where the embed links:

| `channel`           | Key                                     | Link                                                              |
| ------------------- | --------------------------------------- | ----------------------------------------------------------------- |
| `release` (default) | `LATEST_FIREFOX_VERSION`                | `https://www.firefox.com/firefox/{version}/releasenotes/`         |
| `esr`               | `FIREFOX_ESR`                           | same, with the trailing `esr` dropped (`140.16.0esr` → `140.16.0`) |
| `beta`              | `LATEST_FIREFOX_RELEASED_DEVEL_VERSION` | same, with `b5` rewritten to `beta` (`157.0b5` → `157.0beta`)     |
| `devedition`        | `FIREFOX_DEVEDITION`                    | as for `beta`                                                     |
| `nightly`           | `FIREFOX_NIGHTLY`                       | `https://www.firefox.com/firefox/nightly/notes/`                  |

The feed supports conditional requests, so an unchanged feed costs a `304`. An empty value for the selected channel is a polling failure, not a version.

## `chrome`

`platform` and `channel` are inserted into the API path as-is. `rollout` decides which version counts as released:

- `started` (default) — the top version being served, as soon as its staged rollout begins, even at 0.5%. A pulled rollout makes the top version go *down*, and that is reported too.
- `complete` — only the top version served to 100% of users.

A change in rollout fraction alone is not a new release and posts nothing. The VersionHistory API returns no `ETag`, so every poll downloads the response — only the top release is requested, well under a kilobyte — and is counted as `updated` in the [summary](../how-it-works#knowing-the-poller-is-alive). The embed links to the channel's label on the Chrome Releases blog (canary, which is not announced there, links to the blog itself).

## `json`

chime does not know the shape of the document: `pointer` ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)) names the one value that is the version. Segments are separated by `/`, array elements are addressed by index, and `~0` / `~1` escape `~` / `/`. The pointer is used exactly as written — whitespace is part of a key, so `/v ` and `/v` are different pointers. The value must be a string or an integer; a fractional number is refused, because `1.10` would read back as `1.1` — serve such a version as a string. Searching inside an array or combining several fields is not possible.

| Feed                | `url`                                                         | `pointer`      |
| ------------------- | ------------------------------------------------------------- | -------------- |
| Node.js             | `https://nodejs.org/dist/index.json`                          | `/0/version`   |
| A GitHub repository | `https://api.github.com/repos/{owner}/{repo}/releases/latest` | `/tag_name`    |

A pointer that does not match the document is not caught at startup: like a status page URL that does not match its `kind`, it fails at the first poll with a `warn` and keeps failing each interval.

## Notification semantics

- **The first poll after startup is silent** — it records the current version as a baseline. A restart re-baselines.
- A post is sent whenever the version string **differs** from the last one seen, including when it goes down. There is no version ordering: `140.16.0esr`, `157.0b5` and `v24.9.0` do not share one.
- The last-seen version is updated **before** the Discord request, so a failed send is not retried.
- Polling failures (unreachable feed, non-JSON body, pointer that matches nothing) are logged at `warn` and never posted.
- **Watches that read the same URL share one request** — several Firefox channels, or two `json` watches with different pointers on one endpoint. The shared request runs at the shortest `poll_interval_sec` among them, and one watch failing to read the body does not stop the others from reporting.

## What it looks like in Discord

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
