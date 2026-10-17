---
layout: doc
---

# RSS Feeds

`[[rss]]` subscribes to a syndication feed and posts every entry that was not in it before. RSS 0.9x / 1.0 / 2.0, Atom and JSON Feed are all accepted; the format is detected from the body, so there is no `kind` to set. `url` is the feed itself and is requested as written — chime appends nothing. A body that is none of these fails the poll with `response is not an RSS, Atom or JSON feed: …` — this is logged, not fatal. The keys are listed in [Configuration](../configuration#rss).

## Notification semantics

- **The first poll after startup is silent** — it records the entries currently in the feed as a baseline. A restart re-baselines.
- An entry is posted when its **id** has not been seen: `<guid>` in RSS, `<id>` in Atom, `id` in JSON Feed, or, when the feed gives none, a hash of the entry's link and title (the feed URL and title when it has no link). Editing an entry that was already seen does not post it again — unless it has no id of its own: then a new title or link is a new id, and the entry is posted again. Two such entries with the same link and title are one entry.
- The id is recorded **before** the Discord request, so a failed send is not retried.
- An entry that drops out of the feed is forgotten; if it comes back, it is reported as new. A feed that answers with no entries at all keeps the baseline.
- An entry with no id of its own and neither a link nor a title is ignored: it has nothing stable to be recognised by. So is an entry with no title, link or body.
- Several new entries in one poll are posted **oldest first**, by the entry's own date; entries without a date come last, in feed order. There is no cap on how many are posted.
- Polling failures (unreachable feed, a body that is not a feed) are logged at `warn` and never posted.

Many feeds — the Claude Code changelog among them — send `cache-control: no-cache` and no `ETag`, so they cannot be validated and every poll downloads the whole feed and is counted as `updated` in the [summary](../how-it-works#knowing-the-poller-is-alive), as for Chrome. gzip keeps that small, and the same 8 MB ceiling as for status pages applies.

## What it looks like in Discord

Each new entry is one embed, attributed to `display_name`. The colour is fixed (Discord fuchsia).

```
2.1.283                                  ← the entry title, linked to the entry
- Added x-claude-code-prompt-id to the gateway hint headers …
- Fixed MCP progress notifications being discarded …
Published: 2026-09-25 22:00 UTC          ← if the entry is dated
code.claude.com · <time chime saw it>
```

The body is the entry's summary, or its content when there is no summary, rendered from HTML to plain text: list items become `- ` lines, paragraphs and line breaks become newlines, `<script>`, `<style>` and comments are dropped with their content, every other tag is dropped and its text kept. Atom and JSON Feed text that the feed marks as plain text is shown as it is. An Atom title marked as HTML is rendered the same way. Long bodies are truncated; the linked page is the authoritative copy. The timestamp is when chime noticed the entry, not the entry's own date.
