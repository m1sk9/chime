---
layout: doc
---

# Health Check

`docker ps` only tells you the process hasn't crashed — a hung scheduler loop looks identical to a healthy one. chime exposes a liveness signal that answers **"is the scheduler actually ticking?"**, not "did the last Discord send succeed?".

## How it works

- On every tick the daemon writes the current timestamp to a heartbeat file (default `/tmp/chime.heartbeat`, override with `CHIME_HEARTBEAT_PATH`). The write happens **before** any Discord request, so the signal is independent of network reachability, and is repeated after every Discord request, so a tick that sends several messages to a slow Discord is not mistaken for a hung one.
- The `chime health` subcommand reads that file's mtime and exits `0` when it is fresh — `now - mtime <= 2 * tick_interval_sec` — and non-zero with a one-line stderr message otherwise (stale, missing, or unreadable). It reads the tick interval from the same `CHIME_CONFIG`, and does **not** require any webhook env var.

The container image already wires this into a `HEALTHCHECK` (exec-form, since distroless has no shell), so `docker ps` / `docker inspect` report health automatically. To set it explicitly in `docker/compose.yml`:

```yaml
healthcheck:
  test: ["CMD", "/usr/local/bin/chime", "health"]
  interval: 30s
  timeout: 5s
  start_period: 10s
  retries: 3
```

Without Docker you can call `chime health` from any supervisor or monitoring probe — its exit code is the contract.
