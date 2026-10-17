---
layout: doc
---

# ヘルスチェック

`docker ps` で分かるのはプロセスがクラッシュしていないことだけで，スケジューラのループがハングしていても正常時と見分けがつきません．chime は，「直近の Discord への送信が成功したか」ではなく，**「スケジューラが実際に tick しているか」** に答える liveness シグナルを提供します．

## 仕組み

- デーモンは tick ごとに現在のタイムスタンプを heartbeat ファイルに書き込みます (デフォルトは `/tmp/chime.heartbeat`，`CHIME_HEARTBEAT_PATH` で変更可能)．書き込みは Discord へのリクエストより**前**に行われるため，シグナルはネットワークの到達性に左右されません．また Discord へのリクエストのたびに再度書き込まれるため，遅い Discord に複数のメッセージを送る tick がハングと誤認されることもありません．
- `chime health` サブコマンドはそのファイルの mtime を読み，新しければ (`now - mtime <= 2 * tick_interval_sec`)`0` で終了します．それ以外 (古い，存在しない，読み取れない) の場合は，stderr に 1 行のメッセージを出して非 0 で終了します．tick 間隔は同じ `CHIME_CONFIG` から読み取り，webhook の env 変数は**不要**です．

コンテナイメージにはすでにこれが `HEALTHCHECK` (distroless にはシェルがないため exec 形式) として組み込まれており，`docker ps` / `docker inspect` で自動的にヘルス状態が報告されます．`docker/compose.yml` で明示的に設定するには次のようにします．

```yaml
healthcheck:
  test: ["CMD", "/usr/local/bin/chime", "health"]
  interval: 30s
  timeout: 5s
  start_period: 10s
  retries: 3
```

Docker を使わない場合は，任意のスーパーバイザや監視プローブから `chime health` を呼び出せます．契約となるのは終了コードです．
