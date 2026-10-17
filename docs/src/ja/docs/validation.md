---
layout: doc
---

# バリデーション

以下はすべて，起動時に説明的なエラーと非 0 の終了コードで拒否されます．chime が部分的に起動することはありません．

## 全般

- TOML 内の任意の場所にある未知のフィールド
- `1..=60` の範囲外の `tick_interval_sec`
- 未知の IANA タイムゾーン
- 未設定，空，または有効な `https` の URL ではない webhook env 変数
- 同じ env 変数に対応する 2 つの異なる `webhook` 名 (例: `team-a` と `team_a`)
- 空，数字のみでない，0，64 ビットを超える，または TOML の整数として書かれた `thread_id`
- リマインダー，ステータスページ，watch，rss フィードのいずれも定義されていない
- `60..=3600` の範囲外の `poll_interval_sec`

## リマインダー

- 重複した，または空のリマインダー `name`
- `HH:MM` 形式ではない `time`，または時が 23 超・分が 59 超
- 空の `days`，または未知の曜日文字列
- 空の `day_of_month`，または `1..=31` の範囲外の値
- `days` / `day_of_month` のどちらも指定していない，または両方を指定しているリマインダー
- 空の `message`

## ステータスページ

- 重複した，または空のステータスページ `name`
- `https` ではない，またはホストを持たないステータスページの `url` または `avatar_url`
- `statuspage` / `instatus` / `incidentio` / `datadog` / `custom` のいずれでもないステータスページの `kind`
- `mapping` のない `custom` ステータスページ，または他の kind のページにある `mapping`
- `id` または `title` のない `mapping`，未知のキーを含む `mapping`，`/` で始まらない pointer を含む `mapping`
- `none` / `maintenance` / `minor` / `major` / `critical` のいずれでもない `min_impact`

## Watch

- 重複した，または空の watch `name`
- `firefox` / `chrome` / `json` のいずれでもない `source.kind`，または `source` 内の未知のキー
- 列挙された値の範囲外の `firefox.channel` / `chrome.platform` / `chrome.channel` / `chrome.rollout`
- `linux`，`android`，`webview`，`ios` で `channel = "extended"` を指定した `chrome` watch — Extended Stable は `win`，`win64`，`mac`，`mac_arm64` にしか存在せず，API は他を拒否します
- `url` / `link` が https ではない，または `pointer` が `/` で始まらない `json` watch

## RSS フィード

- 重複した，または空の rss `name`
- `https` ではない，またはホストを持たない rss の `url` または `avatar_url`
