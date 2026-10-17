---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: 'chime'
  tagline: A Discord webhook reminder, status page forwarder and feed watcher.
  actions:
    - theme: brand
      text: Documentation
      link: /docs/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/m1sk9/chime

features:
  - title: Reminders
    details: Reads a TOML schedule and posts to Discord webhooks at the configured times.
    icon: ⏰
  - title: Status pages
    details: Forwards incident updates from Statuspage, Instatus, incident.io, Datadog or any JSON endpoint.
    icon: 🚨
  - title: Release watches
    details: Announces new Firefox and Chrome versions, or any version served by a JSON endpoint.
    icon: 🆕
  - title: RSS feeds
    details: Posts each new entry of an RSS, Atom or JSON feed as an embed.
    icon: 📰
---
