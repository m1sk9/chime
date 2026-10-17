import type { DefaultTheme } from 'vitepress';

export const ja: DefaultTheme.Config = {
  editLink: {
    pattern: 'https://github.com/m1sk9/chime/edit/main/docs/src/:path',
    text: 'GitHub で編集',
  },
  sidebar: {
    '/ja/docs/': [
      {
        text: '導入',
        items: [
          { text: 'はじめる', link: '/ja/docs/getting-started' },
          { text: '設定', link: '/ja/docs/configuration' },
          { text: 'Webhook', link: '/ja/docs/webhooks' },
        ],
      },
      {
        text: '機能',
        items: [
          { text: 'ステータスページ', link: '/ja/docs/features/status-pages' },
          { text: 'リリース監視', link: '/ja/docs/features/watches' },
          { text: 'RSS フィード', link: '/ja/docs/features/rss' },
        ],
      },
      {
        text: '運用',
        items: [
          { text: '動作の仕組み', link: '/ja/docs/how-it-works' },
          { text: 'ヘルスチェック', link: '/ja/docs/health-check' },
          { text: 'バリデーション', link: '/ja/docs/validation' },
          { text: 'ソースからビルド', link: '/ja/docs/build-from-source' },
        ],
      },
    ],
  },
};
