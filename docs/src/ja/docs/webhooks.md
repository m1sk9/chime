---
layout: doc
---

# Webhook

`webhook` フィールドは URL ではなく，論理名です．起動時に chime はこの名前から env キーを導出します．

| `webhook` の値 | env キー                  |
| --------------- | ------------------------ |
| `team`          | `CHIME_WEBHOOK_TEAM`     |
| `on-call`       | `CHIME_WEBHOOK_ON_CALL`  |
| `ops.alpha`     | `CHIME_WEBHOOK_OPS_ALPHA` |

英数字以外の文字は `_` に置き換えられ，結果は大文字になります．各 env キーについて，chime はまず `<KEY>_FILE` (Docker secrets 用) を試し，なければ `<KEY>` にフォールバックします．値は前後の空白を取り除いた後，有効な URL である必要があります．

webhook URL が `config.toml` に現れることはありません．

## スレッドへの投稿

`thread_id` を設定したエントリは，チャンネルの代わりにそのスレッドへ投稿します．chime は解決した webhook URL に `thread_id=<id>` を追加します．これは Discord の [Execute Webhook の `thread_id` パラメータ](https://docs.discord.com/developers/resources/webhook)です．

- スレッドは webhook のチャンネルに属している必要があります．アーカイブ済みのスレッドは，メッセージが届いた時点で Discord によってアーカイブ解除されます．
- env の URL に含まれていた他のクエリパラメータは保持されます．すでに含まれている `thread_id` はエントリのものに置き換えられるため，1 つの webhook で異なるスレッドに投稿する複数のエントリをまかなえます．
- 値は snowflake を数字の**引用符付き**文字列で書いたものです．`thread_id = "1234567890123456789"` と書き，`thread_id = 1234567890123456789` とは書きません．
