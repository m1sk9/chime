---
layout: doc
---

# ステータスページ

`[[status_pages]]` は公開ステータスページを監視します．`kind` はそのページの背後にあるソフトウェアを指し，chime がどのドキュメントを取得するかを決めます．デフォルトは `statuspage` なので，`kind` が存在しなかった頃に書かれた設定もそのまま動作します．キーの一覧は [設定](../configuration#status-pages) にあります．

| `kind`                 | ソフトウェア                                                                                                                                                                                   | 取得先                        | `ETag`                  | 重大度                |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------- | ----------------------- | --------------------- |
| `statuspage` (デフォルト) | [Atlassian Statuspage](https://www.atlassian.com/software/statuspage) — `status.claude.com`，`status.proton.me`，`www.githubstatus.com`，`discordstatus.com`，`www.cloudflarestatus.com`，… | `<url>/api/v2/incidents.json` | あり                    | あり                  |
| `instatus`             | [Instatus](https://instatus.com/help/api/public-data)                                                                                                                                          | `<url>/v3/summary.json`       | なし                    | あり                  |
| `incidentio`           | [incident.io](https://docs.incident.io/status-pages/subscriptions)                                                                                                                             | `<url>/feed.atom`             | なし                    | **なし**              |
| `datadog`              | [Datadog Status Pages](https://docs.datadoghq.com/service_management/status_pages/) — `<prefix>.statuspage.datadoghq.com`                                                                     | `<url>/config.json`           | あり                    | あり                  |
| `custom`               | インシデントを JSON で提供するもの全般                                                                                                                                                         | `url`，書かれたとおり         | サーバーが送る場合のみ  | `mapping.impact` を設定した場合のみ |

`custom` 以外のすべての kind では，`url` はページのベース URL であり，chime がパスを付け足します．`https://example.com/status` は，末尾の `/` の有無にかかわらず `https://example.com/status/api/v2/incidents.json` になります．`custom` では，`url` は JSON ドキュメントそのものであり，書かれたとおりにリクエストされます．

chime はそのドキュメントを**ポーリング**します．受信 webhook を受け取るわけではありません．何も公開する必要はなく，ポートも開かず，サブスクリプションを事前に登録する必要もないため，`config.toml` を編集するだけでページを追加できます．リクエストは条件付き (`If-None-Match`) なので，サーバーが `ETag` を送る場合，更新のないページは `304` で本文なしで済みます．Instatus と incident.io は `ETag` を送らないため，それらはポーリングのたびにドキュメントをダウンロードし，[サマリー](../how-it-works#knowing-the-poller-is-alive) で `updated` として数えられます．

`kind` に一致しないドキュメントは，`response does not decode as <kind>: …` (`Atlassian Statuspage`，`Instatus`，`incident.io`，`Datadog Status Pages`，`custom JSON` のいずれか) でポーリングに失敗します．これはログに残りますが致命的ではなく，次の間隔で再試行されます．

予定メンテナンスは，どの kind でも転送されません．Statuspage の `incidents.json` には含まれておらず，他のドキュメントのメンテナンスのセクションは読み取られないためです．

フィードは 8 MB という上限まで読み取られ，それを超えると `status page body exceeded 8388608 bytes` で拒否されます．実際のフィードは 40-300 KB なので，これはページに異常が起きた場合にしか発動しません．レスポンスは gzip エンコードされており，gzip では小さなダウンロードが任意の大きさのバッファに展開されうるため，上限は転送量ではなく展開後のサイズに対してかかります．他のポーリング失敗と同様に，ログに残り，次の間隔で再試行されます．

## kind ごとの注意

- **`statuspage`** — `incidents.json` には，直近 50 件のインシデントがすべての更新とともに含まれます．
- **`instatus`** — v3 の summary には，アクティブなインシデントと最近解決されたインシデントだけが，それぞれの最新の更新のみ含まれます．そのため，2 回のポーリングの間に複数の更新があった場合は，最後の 1 件として届きます．最近解決されたインシデントは，本文なしの `Resolved` として投稿されます．Instatus の重大度は `MAJOROUTAGE` → `critical`，`PARTIALOUTAGE` → `major`，`DEGRADEDPERFORMANCE` → `minor`，`UNDERMAINTENANCE` → `maintenance` に対応づけられます．
- **`incidentio`** — ページの公開 Atom フィードで，インシデントごとに最新の更新を 1 エントリとして持ちます．フィードには**重大度がない**ため，`Impact` は常に `Unknown` となり，**`min_impact` は効果を持ちません**．重大度が不明なものは常に転送されます．各エントリの `Status: …` 行と `Affected components` リストは，`Status` と `Components` フィールドになります．incident.io の widget API は使用しません．ページごとにオプトインが必要で，その URL とスキーマはページの管理設定にしか表示されないためです．
- **`datadog`** — `config.json` は，Datadog がホストするページ自身が描画に使うドキュメントで，Datadog はこれに対する公開 API を文書化していません．ページのインシデント履歴の**すべて**を含むため，1 MB 以上になることもあります (8 MB の上限の範囲内)．条件付きリクエストに対応しているので，そのコストを払うのはページが変わったときだけです．重大度は，インシデント中に記録されたコンポーネントの最悪の状態です (`major_outage` → `critical`，`partial_outage` → `major`，`degraded` → `minor`)．そのため，解決済みのインシデントでもその深刻さが分かります．embed は `<url>/<incident id>` にリンクします．
- **`custom`** — 以下を参照してください．

::: info UptimeRobot には対応していません
UptimeRobot のステータスページには，公開されたインシデントフィードがありません．API にはアカウントキーが必要で，お知らせは購読者にメールでしか届かず，ページのスクリプトが読むイベントフィードは文書化されておらず，モニターの up/down イベントしか含まず，安定した id もなく，時刻もロケール形式です．
:::

## `custom`

`mapping` は，[RFC 6901](https://www.rfc-editor.org/rfc/rfc6901) の JSON pointer で，インシデントがどこにあり，各インシデントのどのフィールドを chime が読むかを指定します．すべての pointer は `/` で始まる必要があり，`mapping` は `kind = "custom"` では必須で，他の kind では拒否されます．

```toml
[[status_pages]]
name = "my-service"
kind = "custom"
url = "https://status.example.com/incidents.json"
webhook = "team"
mapping = { incidents = "/incidents", id = "/id", title = "/title", status = "/state", body = "/message", updated_at = "/updated_at", link = "/url", impact = "/severity" }
```

| キー         |          | 意味                                                                                                                       |
| ------------ | -------- | -------------------------------------------------------------------------------------------------------------------------- |
| `incidents`  | 任意     | ドキュメントのルートから見たインシデントの配列．ドキュメント自体が配列の場合は省略します．                                 |
| `id`         | 必須     | インシデントの id．                                                                                                        |
| `title`      | 必須     | embed のタイトル．                                                                                                         |
| `status`     | 任意     | `investigating` / `identified` / `monitoring` / `resolved` / `postmortem`．大文字小文字は問わず，それ以外は `Update` と表示されます． |
| `body`       | 任意     | embed のテキスト．                                                                                                         |
| `updated_at` | 任意     | インシデントが最後に変化した時刻．RFC 3339 のタイムスタンプ．                                                              |
| `link`       | 任意     | embed のタイトルがリンクする URL．                                                                                         |
| `impact`     | 任意     | `none` / `maintenance` / `minor` / `major` / `critical`．大文字小文字は問わず，それ以外は `Unknown` になります．           |

`incidents` 以外のすべてのキーは，**1 件のインシデントの中**を指す pointer です．そのため `/id` は各要素の `id` を読み，`/meta/0/id` はさらに深くに到達します．値は，[`json` watch](./watches#json) と同様に，文字列または整数でなければなりません．インシデントは，`updated_at` が変わるたびに再び報告されます．`updated_at` がない場合は `status` が変わるたびに，どちらもない場合は最初に現れたときだけ報告されます．`updated_at` がない場合，embed の時刻は chime が変化を確認した時刻になります．各インシデントは 1 つの更新として扱われ，インシデントの中に入れ子になった更新の配列は読み取られません．

ドキュメントに合わないマッピング (`id` や `title` が存在しない，または空，値がオブジェクトまたは小数，`incidents` の pointer が配列を指していない，`updated_at` が RFC 3339 ではない) は，そのインシデントをスキップするのではなく，ポーリング全体を `warn` とともに失敗させます．これは，マッピングの誤りが黙ってインシデントを取りこぼすのではなく，ポーリングのたびに見えるようにするためです．1 件のインシデントでフィールドが存在しない，`null`，または空である場合は，単に存在しないものとして扱われます．

## 通知のセマンティクス

通知の単位はインシデントではなく**インシデントの更新**です．`Investigating → Identified → Monitoring → Resolved` は 4 つのメッセージを生成し，それぞれ最初のメッセージの編集ではなく，別々の投稿になります．`min_impact` は，指定した重大度を下回るインシデントを除外します．重大度が欠けている，または chime が認識しない値で報告されたインシデントは，黙って除外されるのではなく，常に転送されます．

## Discord での表示

各更新は 1 つの embed になります．色のバーが重大度をひと目で示します．

| 条件                                | 色     | 絵文字              |
| ----------------------------------- | ------ | ------------------- |
| Resolved (重大度を問わない)        | 緑     | ✅                  |
| `critical`                          | 赤     | 状態に応じて 🔍 / 🎯 / 👀 |
| `major`                             | オレンジ | ↑                 |
| `minor`                             | 黄     | ↑                   |
| `none`，または認識できない重大度    | 灰     | ↑                   |

解決済みのインシデントは，重大度にかかわらず緑になるため，「修正された」という通知が赤いバーで届くことはありません．embed には，インシデントのタイトル (インシデントページへのリンク．例えば Statuspage の短縮リンク)，最新の更新のテキスト，`Status` / `Impact` / `Components` フィールド，フッターとしてステータスページのホスト，そして更新自身のタイムスタンプが含まれます．このタイムスタンプは chime が投稿した時刻ではなく，ページがそれを公開した時刻です．例外は，ドキュメントが時刻を持たない更新 (`updated_at` のない `custom` ページ，日付のない incident.io のエントリ) で，その場合は chime が最初に確認した時刻になります．

実際のインシデントの 1 つでは，`Status` フィールドは `✅ Resolved`，`Impact` は `Minor` と表示され，メッセージは `display_name` の名前で投稿されます．そのため，複数のステータスページが 1 つの Discord チャンネルを共有しても区別できます．

長い本文は切り詰められます (postmortem は数千文字に及びます)．リンク先のインシデントページが正本です．
