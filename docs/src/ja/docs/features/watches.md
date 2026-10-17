---
layout: doc
---

# リリース監視

`[[watches]]` はリリースフィードをポーリングし，そこから読み取ったバージョンが変わったときに投稿します．`source` がフィードを選びます．`source = { kind = "firefox" }` と `[watches.source]` サブテーブルは同じ意味です．キーの一覧は [設定](../configuration#watches) にあります．

| `kind`    | フィード                                                                                                                                                                                                |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `firefox` | Mozilla product-details，`https://product-details.mozilla.org/1.0/firefox_versions.json`                                                                                                                |
| `chrome`  | Chrome [VersionHistory API](https://developer.chrome.com/docs/web-platform/versionhistory/reference)，`https://versionhistory.googleapis.com/v1/chrome/platforms/{platform}/channels/{channel}/versions/all/releases` |
| `json`    | 指定した任意の https JSON エンドポイント．JSON pointer で読み取る                                                                                                                                       |

Safari には対応していません．Apple は機械可読なリリースフィードを公開していないためです．

## `firefox`

`channel` は監視する product-details のキーと，embed のリンク先を選択します．

| `channel`           | キー                                    | リンク                                                            |
| ------------------- | --------------------------------------- | ----------------------------------------------------------------- |
| `release` (デフォルト) | `LATEST_FIREFOX_VERSION`                | `https://www.firefox.com/firefox/{version}/releasenotes/`         |
| `esr`               | `FIREFOX_ESR`                           | 同じ．ただし末尾の `esr` を除く (`140.16.0esr` → `140.16.0`)      |
| `beta`              | `LATEST_FIREFOX_RELEASED_DEVEL_VERSION` | 同じ．ただし `b5` を `beta` に書き換える (`157.0b5` → `157.0beta`) |
| `devedition`        | `FIREFOX_DEVEDITION`                    | `beta` と同じ                                                     |
| `nightly`           | `FIREFOX_NIGHTLY`                       | `https://www.firefox.com/firefox/nightly/notes/`                  |

このフィードは条件付きリクエストに対応しているため，変更がなければ `304` で済みます．選択したチャンネルの値が空の場合は，バージョンではなくポーリングの失敗として扱われます．

## `chrome`

`platform` と `channel` は API のパスにそのまま挿入されます．`rollout` は，どのバージョンをリリース済みとみなすかを決めます．

- `started` (デフォルト) — 配信されている最上位のバージョンで，段階的ロールアウトが (0.5% であっても) 始まった時点で対象になります．ロールアウトが取り下げられると最上位のバージョンが*下がり*，それも報告されます．
- `complete` — 100% のユーザーに配信されている最上位のバージョンのみ．

ロールアウト割合だけの変化は新しいリリースではなく，何も投稿されません．VersionHistory API は `ETag` を返さないため，ポーリングのたびにレスポンスをダウンロードし (最上位のリリースだけをリクエストするので 1 キロバイトを大きく下回ります)，[サマリー](../how-it-works#knowing-the-poller-is-alive) で `updated` として数えられます．embed は Chrome Releases ブログのそのチャンネルのラベルにリンクします (そこでは告知されない canary は，ブログ自体にリンクします)．

## `json`

chime はドキュメントの形を知りません．`pointer` ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)) が，バージョンとなる 1 つの値を指定します．セグメントは `/` で区切られ，配列の要素はインデックスで指定し，`~0` / `~1` は `~` / `/` のエスケープです．pointer は書かれたとおりに使われます．空白もキーの一部なので，`/v ` と `/v` は別の pointer です．値は文字列または整数でなければなりません．小数は拒否されます．`1.10` が `1.1` として読み戻されてしまうためで，そのようなバージョンは文字列として提供してください．配列の中を検索したり，複数のフィールドを組み合わせたりすることはできません．

| フィード            | `url`                                                         | `pointer`      |
| ------------------- | ------------------------------------------------------------- | -------------- |
| Node.js             | `https://nodejs.org/dist/index.json`                          | `/0/version`   |
| GitHub リポジトリ   | `https://api.github.com/repos/{owner}/{repo}/releases/latest` | `/tag_name`    |

ドキュメントに一致しない pointer は，起動時には検出されません．`kind` に一致しないステータスページの URL と同様に，最初のポーリングで `warn` とともに失敗し，その後も間隔ごとに失敗し続けます．

## 通知のセマンティクス

- **起動後の最初のポーリングは通知しません．** 現在のバージョンをベースラインとして記録します．再起動するとベースラインを取り直します．
- バージョン文字列が最後に確認したものと**異なる**たびに投稿されます．バージョンが下がった場合も同様です．バージョンの順序づけはありません．`140.16.0esr`，`157.0b5`，`v24.9.0` は共通の順序を持たないためです．
- 最後に確認したバージョンは Discord へのリクエストの**前**に更新されるため，送信に失敗しても再試行されません．
- ポーリングの失敗 (フィードに到達できない，JSON ではない本文，どこにも一致しない pointer) は `warn` でログに残り，投稿されることはありません．
- **同じ URL を読む watch は 1 つのリクエストを共有します．** 複数の Firefox チャンネルや，1 つのエンドポイントで pointer が異なる 2 つの `json` watch がその例です．共有されるリクエストは，それらの中で最短の `poll_interval_sec` で実行され，1 つの watch が本文を読み取れなくても，他の watch の報告は止まりません．

## Discord での表示

新しいバージョンは 1 件ごとに 1 つの embed になり，`display_name` の名前で投稿され，フッターにはフィードのホストが表示されます．色は固定です (Discord blurple)．リリースに重大度はありません．

```
Firefox 156.0.1                          ← リリースノートへのリンク
Previous: 156.0        Channel: Release
Next release: 2026-10-09                  ← release チャンネルのみ
Channels: Release 156.0.1 · ESR 140.16.0esr · Beta 157.0b5 · Developer Edition 157.0b5 · Nightly 159.0a1
product-details.mozilla.org · <time chime saw it>
```

```
Chrome Stable 155.0.8059.12              ← Chrome Releases ブログへのリンク
Previous: 154.0.8037.58   Channel: Stable   Platform: win
Rollout: 0.5%             Milestone: 155   Serving since: 2026-09-23 18:50 UTC
versionhistory.googleapis.com · <time chime saw it>
```

```
node v24.9.0                             ← `link` が設定されていればそこへのリンク
Previous: v24.8.0
nodejs.org · <time chime saw it>
```

タイムスタンプは常に chime が変化を検知した時刻であり，リリースから最大 `poll_interval_sec` 後になります．product-details と任意の JSON には公開時刻がまったくありません．Chrome にはありますが，ロールアウトが取り下げられた後に配信されるバージョンは古いリリースであり，その開始時刻を使うと投稿が数日前の日付になってしまうため，代わりに `Serving since` として表示されます．
