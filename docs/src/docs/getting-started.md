---
layout: doc
---

# Getting Started

chime reads a TOML schedule, runs as a long-lived process, and posts to Discord webhooks at the configured times. This page explains how to run it.

```sh
docker pull ghcr.io/m1sk9/chime:v0
```

## With Docker Compose (recommended)

1. Copy the example files from the repository:

   ```sh
   cp config/config.toml.example config/config.toml
   cp config/.env.example config/.env
   ```

2. Edit `config/config.toml` with your reminders and `config/.env` with your webhook URLs.
3. Start it:

   ```sh
   docker compose -f docker/compose.yml up -d
   ```

Compose pulls `ghcr.io/m1sk9/chime:latest` by default. To pin a specific version, change the `image:` tag in `docker/compose.yml` (any of `vX`, `vX.Y`, `vX.Y.Z`, `latest`, or a commit SHA are published per release).

To build the image locally instead, uncomment the `build:` block in `docker/compose.yml` and remove the `image:` line.

### Docker secrets (alternative to `.env`)

Swap `env_file:` for the `secrets:` block shown commented in `docker/compose.yml`, and reference each secret via the `_FILE` convention:

```yaml
environment:
  CHIME_WEBHOOK_TEAM_FILE: /run/secrets/chime_webhook_team
```

chime reads the file path from `<KEY>_FILE` first; if unset it falls back to `<KEY>`. Whitespace around the value is trimmed. See [Webhooks](./webhooks) for how a `webhook` name becomes an env key.

## Without Docker

You will need to keep chime alive yourself — see [How It Works](./how-it-works) for what that entails.

1. Build the binary (see [Building from Source](./build-from-source)).
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
