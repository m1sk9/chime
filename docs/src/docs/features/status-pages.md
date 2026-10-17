---
layout: doc
---

# Status Pages

`[[status_pages]]` watches a public status page. `kind` names the software behind it and decides which document chime fetches; it defaults to `statuspage`, so a config written before `kind` existed keeps working unchanged. The keys are listed in [Configuration](../configuration#status-pages).

| `kind`                 | Software                                                                                                                                                                                       | Fetched                       | `ETag`                  | Severity              |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------- | ----------------------- | --------------------- |
| `statuspage` (default) | [Atlassian Statuspage](https://www.atlassian.com/software/statuspage) — `status.claude.com`, `status.proton.me`, `www.githubstatus.com`, `discordstatus.com`, `www.cloudflarestatus.com`, … | `<url>/api/v2/incidents.json` | yes                     | yes                   |
| `instatus`             | [Instatus](https://instatus.com/help/api/public-data)                                                                                                                                          | `<url>/v3/summary.json`       | no                      | yes                   |
| `incidentio`           | [incident.io](https://docs.incident.io/status-pages/subscriptions)                                                                                                                             | `<url>/feed.atom`             | no                      | **no**                |
| `datadog`              | [Datadog Status Pages](https://docs.datadoghq.com/service_management/status_pages/) — `<prefix>.statuspage.datadoghq.com`                                                                     | `<url>/config.json`           | yes                     | yes                   |
| `custom`               | anything that serves incidents as JSON                                                                                                                                                         | `url`, as written             | if the server sends one | if `mapping.impact` is set |

For every kind but `custom`, `url` is the page's base URL and chime appends the path itself — `https://example.com/status` becomes `https://example.com/status/api/v2/incidents.json`, with or without a trailing `/`. For `custom`, `url` is the JSON document and is requested exactly as written.

chime **polls** that document — it does not receive an inbound webhook. Nothing needs to be exposed, no ports are opened, and no subscription has to be registered out-of-band, so a page is added by editing `config.toml` alone. Requests are conditional (`If-None-Match`), so an unchanged page costs a `304` and no body — where the server sends an `ETag`. Instatus and incident.io do not, so every poll of those downloads the document and is counted as `updated` in the [summary](../how-it-works#knowing-the-poller-is-alive).

A document that does not match its `kind` fails the poll with `response does not decode as <kind>: …` (`Atlassian Statuspage`, `Instatus`, `incident.io`, `Datadog Status Pages` or `custom JSON`) — this is logged, not fatal, and retried on the next interval.

Scheduled maintenances are never forwarded, for any kind: Statuspage's `incidents.json` does not list them, and the maintenance sections of the other documents are not read.

A feed is read up to a hard 8 MB ceiling and refused past it with `status page body exceeded 8388608 bytes`. Real feeds measure 40-300 KB, so this only fires on a page that has gone wrong: responses are gzip-encoded, and gzip lets a small download expand into an arbitrarily large buffer, so the limit is on what is decompressed rather than on what is transferred. Like every other polling failure it is logged and retried on the next interval.

## Per-kind notes

- **`statuspage`** — `incidents.json` holds the 50 most recent incidents with every update.
- **`instatus`** — the v3 summary lists only active and recently resolved incidents, and only each incident's latest update, so several updates between two polls arrive as the last one. A recently resolved incident is posted as `Resolved` with no text. Instatus severities map as `MAJOROUTAGE` → `critical`, `PARTIALOUTAGE` → `major`, `DEGRADEDPERFORMANCE` → `minor`, `UNDERMAINTENANCE` → `maintenance`.
- **`incidentio`** — the page's public Atom feed, one entry per incident with its latest update. The feed carries **no severity**, so `Impact` is always `Unknown` and **`min_impact` has no effect**: an unknown severity is always forwarded. The `Status: …` line and the `Affected components` list in each entry become the `Status` and `Components` fields. incident.io's widget API is not used: it is opt-in per page, and its URL and schema are shown only in the page's admin settings.
- **`datadog`** — `config.json` is the document the Datadog-hosted page itself renders from; Datadog documents no public API for it. It holds the page's **entire** incident history, so it can reach a megabyte or more (within the 8 MB ceiling); it supports conditional requests, so that is paid only when the page changes. The severity is the worst component state recorded during the incident (`major_outage` → `critical`, `partial_outage` → `major`, `degraded` → `minor`), so a resolved incident still shows how bad it was. The embed links to `<url>/<incident id>`.
- **`custom`** — see below.

::: info UptimeRobot is not supported
Its status pages publish no public incident feed. The API needs an account key, announcements reach subscribers by e-mail only, and the event feed its page script reads is undocumented and carries only monitor up/down events, with no stable id and locale-formatted times.
:::

## `custom`

`mapping` names, by [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901) JSON pointers, where the incidents are and which fields of each one chime reads. Every pointer must start with `/`, and `mapping` is required with `kind = "custom"` and rejected with any other kind.

```toml
[[status_pages]]
name = "my-service"
kind = "custom"
url = "https://status.example.com/incidents.json"
webhook = "team"
mapping = { incidents = "/incidents", id = "/id", title = "/title", status = "/state", body = "/message", updated_at = "/updated_at", link = "/url", impact = "/severity" }
```

| Key          |          | Meaning                                                                                                                    |
| ------------ | -------- | -------------------------------------------------------------------------------------------------------------------------- |
| `incidents`  | optional | The array of incidents, from the document root. Omit it when the document itself is the array.                             |
| `id`         | required | The incident's id.                                                                                                         |
| `title`      | required | The embed title.                                                                                                           |
| `status`     | optional | `investigating` / `identified` / `monitoring` / `resolved` / `postmortem`, any case; anything else shows as `Update`.      |
| `body`       | optional | The embed text.                                                                                                            |
| `updated_at` | optional | When the incident last changed, as an RFC 3339 timestamp.                                                                  |
| `link`       | optional | The URL the embed title links to.                                                                                          |
| `impact`     | optional | `none` / `maintenance` / `minor` / `major` / `critical`, any case; anything else is `Unknown`.                             |

Every key but `incidents` is a pointer **into one incident**, so `/id` reads the `id` of each element and `/meta/0/id` reaches deeper. Values must be strings or integers, as for a [`json` watch](./watches#json). An incident is reported again whenever its `updated_at` changes; without `updated_at`, whenever its `status` changes; without either, only when it first appears. Without `updated_at` the embed is timed when chime saw the change. Each incident is one update: an array of updates nested inside an incident is not read.

A mapping that does not fit the document — an `id` or `title` that is missing or empty, a value that is an object or a fractional number, an `incidents` pointer that is not an array, an `updated_at` that is not RFC 3339 — fails the whole poll with a `warn` rather than skipping the incident, so a mistake in the mapping is visible on every poll instead of silently dropping incidents. A field that is absent, `null` or empty in one incident is just absent.

## Notification semantics

The unit of notification is an **incident update**, not an incident: `Investigating → Identified → Monitoring → Resolved` produces four messages, each a separate post rather than an edit of the first. `min_impact` drops incidents below the given severity; an incident whose severity is missing, or reported with a value chime does not recognise, is always forwarded rather than silently dropped.

## What it looks like in Discord

Each update is one embed. The colour bar is the severity at a glance:

| Condition                           | Colour | Emoji               |
| ----------------------------------- | ------ | ------------------- |
| Resolved (any severity)             | green  | ✅                  |
| `critical`                          | red    | 🔍 / 🎯 / 👀 by state |
| `major`                             | orange | ↑                   |
| `minor`                             | yellow | ↑                   |
| `none`, or an unrecognised severity | grey   | ↑                   |

A resolved incident is green regardless of how severe it was, so "this is fixed" never arrives wearing a red bar. The embed carries the incident title (linked to the incident page — the Statuspage short link, for instance), the latest update's text, `Status` / `Impact` / `Components` fields, the status page host as the footer, and the update's own timestamp — which is when the page published it, not when chime posted it. The exceptions are an update whose document carries no time (a `custom` page without `updated_at`, an undated incident.io entry), which is timed when chime first saw it.

The `Status` field of one real incident renders as `✅ Resolved`, `Impact` as `Minor`, and the message is attributed to `display_name` so several status pages can share one Discord channel and still be told apart.

Long bodies are truncated (postmortems run to thousands of characters); the linked incident page is the authoritative copy.
