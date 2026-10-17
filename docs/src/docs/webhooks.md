---
layout: doc
---

# Webhooks

The `webhook` field is a logical name, not a URL. At startup chime derives an env key from it:

| `webhook` value | env key                  |
| --------------- | ------------------------ |
| `team`          | `CHIME_WEBHOOK_TEAM`     |
| `on-call`       | `CHIME_WEBHOOK_ON_CALL`  |
| `ops.alpha`     | `CHIME_WEBHOOK_OPS_ALPHA` |

Non-alphanumeric characters are mapped to `_` and the result is uppercased; two different names that end up as the same key are rejected at startup. For each env key chime tries `<KEY>_FILE` first (for Docker secrets) and falls back to `<KEY>`. The value must be a valid `https` URL after trimming. Redirects are followed only to `https` URLs, for webhooks and polled endpoints alike.

Webhook URLs never appear in `config.toml`.

## Posting into a thread

An entry that sets `thread_id` posts into that thread instead of the channel: chime adds `thread_id=<id>` to the resolved webhook URL, Discord's [Execute Webhook `thread_id` parameter](https://docs.discord.com/developers/resources/webhook).

- The thread must belong to the webhook's channel, and an archived thread is unarchived by Discord when the message arrives.
- Other query parameters already in the env URL are kept; a `thread_id` already in it is replaced by the entry's, so one webhook can serve entries that post to different threads.
- The value is a snowflake written as a **quoted** string of digits — `thread_id = "1234567890123456789"`, not `thread_id = 1234567890123456789`.
