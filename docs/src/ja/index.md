---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: 'chime'
  tagline: Discord webhook によるリマインダー，ステータスページの転送，フィード監視を行うツールです．
  actions:
    - theme: brand
      text: ドキュメント
      link: /ja/docs/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/m1sk9/chime

features:
  - title: リマインダー
    details: TOML のスケジュールを読み込み，設定した時刻に Discord webhook へ投稿します．
    icon: ⏰
  - title: ステータスページ
    details: Statuspage，Instatus，incident.io，Datadog，または任意の JSON エンドポイントからインシデントの更新を転送します．
    icon: 🚨
  - title: リリース監視
    details: Firefox と Chrome の新バージョン，または JSON エンドポイントが返す任意のバージョンを通知します．
    icon: 🆕
  - title: RSS フィード
    details: RSS，Atom，JSON フィードの新しいエントリを 1 件ずつ embed として投稿します．
    icon: 📰
---
