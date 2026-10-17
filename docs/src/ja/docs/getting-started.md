---
layout: doc
---

# はじめる

chime は TOML のスケジュールを読み込み，長時間動作するプロセスとして，設定した時刻に Discord webhook へ投稿します．このページでは，その実行方法を説明します．

```sh
docker pull ghcr.io/m1sk9/chime:v0
```

## Docker Compose を使う (推奨)

1. リポジトリからサンプルファイルをコピーします．

   ```sh
   cp config/config.toml.example config/config.toml
   cp config/.env.example config/.env
   ```

2. `config/config.toml` にリマインダーを，`config/.env` に webhook URL を記述します．
3. 起動します．

   ```sh
   docker compose -f docker/compose.yml up -d
   ```

Compose はデフォルトで `ghcr.io/m1sk9/chime:latest` を取得します．特定のバージョンに固定するには，`docker/compose.yml` の `image:` タグを変更してください (リリースごとに `vX`，`vX.Y`，`vX.Y.Z`，`latest`，コミット SHA のいずれかが公開されます)．

イメージをローカルでビルドする場合は，`docker/compose.yml` の `build:` ブロックのコメントを外し，`image:` の行を削除してください．

### Docker secrets (`.env` の代替)

`env_file:` を，`docker/compose.yml` にコメントアウトされている `secrets:` ブロックに置き換え，各 secret を `_FILE` 規約で参照します．

```yaml
environment:
  CHIME_WEBHOOK_TEAM_FILE: /run/secrets/chime_webhook_team
```

chime はまず `<KEY>_FILE` からファイルパスを読み，未設定であれば `<KEY>` にフォールバックします．値の前後の空白は取り除かれます．`webhook` の名前がどのように env キーになるかは [Webhook](./webhooks) を参照してください．

## Docker を使わない場合

chime を動かし続ける仕組みは自分で用意する必要があります．必要になる内容は [動作の仕組み](./how-it-works) を参照してください．

1. バイナリをビルドします ([ソースからビルド](./build-from-source) を参照)．
2. バイナリを `PATH` の通った場所 (例: `/usr/local/bin/chime`) に配置します．
3. 設定ファイルを `/etc/chime/config.toml` に置きます (または `CHIME_CONFIG` で別のパスを指定します)．
4. webhook の env 変数を export します (`CHIME_WEBHOOK_<NAME>=https://...`)．
5. プロセススーパーバイザの下で実行します．systemd unit の例:

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

   その後，`systemctl daemon-reload && systemctl enable --now chime` を実行します．
