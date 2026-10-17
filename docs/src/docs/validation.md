---
layout: doc
---

# Validation

All of the following are rejected at startup with a descriptive error and a non-zero exit code — chime never partially starts.

## General

- Unknown fields anywhere in the TOML
- `tick_interval_sec` outside `1..=60`
- Unknown IANA timezone
- Webhook env var unset, empty, or not a valid `https` URL
- Two different `webhook` names that map to the same env var (e.g. `team-a` and `team_a`)
- `thread_id` that is empty, not all digits, zero or beyond 64 bits (or written as a TOML integer)
- Neither reminders, status pages, watches nor rss feeds defined
- `poll_interval_sec` outside `60..=3600`

## Reminders

- Duplicate or empty reminder `name`
- `time` not in `HH:MM` form, or hour > 23 / minute > 59
- Empty `days`, or any unknown weekday string
- Empty `day_of_month`, or any value outside `1..=31`
- A reminder specifying neither or both of `days` / `day_of_month`
- Empty `message`

## Status pages

- Duplicate or empty status page `name`
- Status page `url` or `avatar_url` that is not `https`, or has no host
- Status page `kind` that is not one of `statuspage` / `instatus` / `incidentio` / `datadog` / `custom`
- `custom` status page without `mapping`, or `mapping` on a page of any other kind
- `mapping` without `id` or `title`, with an unknown key, or with a pointer that does not start with `/`
- `min_impact` that is not one of `none` / `maintenance` / `minor` / `major` / `critical`

## Watches

- Duplicate or empty watch `name`
- `source.kind` that is not `firefox` / `chrome` / `json`, or an unknown key inside `source`
- `firefox.channel` / `chrome.platform` / `chrome.channel` / `chrome.rollout` outside their listed values
- `chrome` watch with `channel = "extended"` on `linux`, `android`, `webview` or `ios` — Extended Stable exists only for `win`, `win64`, `mac` and `mac_arm64`, and the API rejects the rest
- `json` watch whose `url` / `link` is not https, or whose `pointer` does not start with `/`

## RSS feeds

- Duplicate or empty rss `name`
- rss `url` or `avatar_url` that is not `https`, or has no host
