---
layout: doc
---

# 設定

設定は単一の TOML ファイルです．デフォルトのパスは `/etc/chime/config.toml` で，`CHIME_CONFIG` env 変数で上書きできます．リポジトリの [`config/config.toml.example`](https://github.com/m1sk9/chime/blob/main/config/config.toml.example) は，そのまま出発点にできる完全な例です．

設定には，`[[reminders]]`，`[[status_pages]]`，`[[watches]]` **または** `[[rss]]` を少なくとも 1 つ定義する必要があります．どれか 1 つのセクションだけでも問題ありません．未知のキーは拒否され，すべての問題は起動時に報告されます．詳しくは [バリデーション](./validation) を参照してください．

## `[system]`

```toml
[system]
log_level = "info"          # debug | info | warn | error (デフォルト: info)
tick_interval_sec = 30      # 1..=60
timezone = "Asia/Tokyo"     # 任意の IANA 名
```

## `[[reminders]]`

```toml
[[reminders]]
name = "daily-standup"      # 空でないこと，ファイル内で一意
time = "09:30"              # HH:MM，24 時間表記
days = ["mon", "tue", "wed", "thu", "fri"]
                            # sun/mon/tue/wed/thu/fri/sat，または ["every"]
message = "Time for standup."
webhook = "team"            # 論理名 — env 経由で解決される (Webhook を参照)
thread_id = "1234567890123456789"
                            # 任意．webhook のチャンネル内のこのスレッドへ
                            # 投稿する (引用符付きの数字)

[[reminders]]
name = "salary-day"
time = "15:00"
day_of_month = [18]         # 1..=31．毎月複数日なら [1, 15] など
message = "Payday is here."
webhook = "team"
```

各リマインダーは `days` (曜日)**または** `day_of_month` (月の日付) のどちらか一方だけでスケジュールします．両方を指定することはできません．`day_of_month` には `1..=31` の日付のリストを指定でき，その月に存在しない日 (例: 2 月の `31`) は，その月だけ単にスキップされます．

`webhook` と `thread_id` については [Webhook](./webhooks) で説明しています．

## `[[status_pages]]`

```toml
[[status_pages]]
name = "claude"             # 空でないこと，ファイル内で一意
kind = "statuspage"         # 任意．statuspage | instatus | incidentio | datadog |
                            # custom (デフォルト: statuspage)
url = "https://status.claude.com"
                            # https のみ．ステータスページのベース URL
                            # (`custom` の場合: JSON ドキュメントそのもの)
webhook = "team"
thread_id = "1234567890123456789"
                            # 任意
display_name = "Claude Status"
                            # 任意．メッセージの Discord ユーザー名
                            # (デフォルト: 上記の `name`)
avatar_url = "https://example.com/claude.png"
                            # 任意．https のみ
poll_interval_sec = 300     # 任意．60..=3600 (デフォルト: 300)
min_impact = "minor"        # 任意．none | maintenance | minor | major | critical
                            # (デフォルト: none — すべて転送する)
```

[ステータスページ](./features/status-pages) を参照してください．

## `[[watches]]`

```toml
[[watches]]
name = "firefox"            # 空でないこと，[[watches]] 内で一意
webhook = "team"
thread_id = "1234567890123456789"
                            # 任意
source = { kind = "firefox" }
                            # channel: release | esr | beta | devedition | nightly
                            # (デフォルト: release)
display_name = "Firefox Releases"
                            # 任意．メッセージの Discord ユーザー名
                            # (デフォルト: source のラベル．例: `Firefox`)
poll_interval_sec = 3600    # 任意．60..=3600 (デフォルト: 3600)
avatar_url = "https://example.com/firefox.png"
                            # 任意．https のみ

[[watches]]
name = "chrome"
webhook = "team"
source = { kind = "chrome", platform = "win", channel = "stable", rollout = "started" }
                            # platform: win | win64 | mac | mac_arm64 | linux |
                            #           android | webview | ios (デフォルト: win)
                            # channel: stable | extended | beta | dev | canary
                            #          (デフォルト: stable)
                            # rollout: started | complete (デフォルト: started)

[[watches]]
name = "node"
webhook = "team"
source = { kind = "json", url = "https://nodejs.org/dist/index.json", pointer = "/0/version", link = "https://nodejs.org/en/blog/release" }
                            # url: https のみ．pointer: RFC 6901．`/` で始まること．
                            # link: 任意．https のみ．embed にそのまま設定される．
```

[リリース監視](./features/watches) を参照してください．

## `[[rss]]`

```toml
[[rss]]
name = "claude-code-changelog"
                            # 空でないこと，[[rss]] 内で一意
url = "https://code.claude.com/docs/en/changelog/rss.xml"
                            # https のみ．フィードの URL そのもの
webhook = "team"
thread_id = "1234567890123456789"
                            # 任意
display_name = "Claude Code Changelog"
                            # 任意．メッセージの Discord ユーザー名
                            # (デフォルト: 上記の `name`)
poll_interval_sec = 900     # 任意．60..=3600 (デフォルト: 900)
avatar_url = "https://example.com/claude.png"
                            # 任意．https のみ
```

[RSS フィード](./features/rss) を参照してください．
