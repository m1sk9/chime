import type { DefaultTheme } from 'vitepress';

export const en: DefaultTheme.Config = {
  editLink: {
    pattern: 'https://github.com/m1sk9/chime/edit/main/docs/src/:path',
    text: 'Edit this page on GitHub',
  },
  sidebar: {
    '/docs/': [
      {
        text: 'Introduction',
        items: [
          { text: 'Getting Started', link: '/docs/getting-started' },
          { text: 'Configuration', link: '/docs/configuration' },
          { text: 'Webhooks', link: '/docs/webhooks' },
        ],
      },
      {
        text: 'Features',
        items: [
          { text: 'Status Pages', link: '/docs/features/status-pages' },
          { text: 'Release Watches', link: '/docs/features/watches' },
          { text: 'RSS Feeds', link: '/docs/features/rss' },
        ],
      },
      {
        text: 'Operation',
        items: [
          { text: 'How It Works', link: '/docs/how-it-works' },
          { text: 'Health Check', link: '/docs/health-check' },
          { text: 'Validation', link: '/docs/validation' },
          { text: 'Building from Source', link: '/docs/build-from-source' },
        ],
      },
    ],
  },
};
