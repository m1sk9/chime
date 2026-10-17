import { execSync } from 'node:child_process';
import { defineConfig } from 'vitepress';
import llmstxt from 'vitepress-plugin-llms';
import { en } from './config/en';
import { ja } from './config/ja';

const gitRoot = execSync('git rev-parse --show-toplevel').toString().trim();
const commitHash = execSync(`git log -1 --format=%H -- ${gitRoot}/docs/`)
  .toString()
  .trim()
  .slice(0, 7);

const footer = {
  copyright: 'Copyright © 2026 m1sk9',
  message: `<a href="https://github.com/m1sk9/chime/commit/${commitHash}">chime/docs@${commitHash}</a>`,
};

export default defineConfig({
  cleanUrls: true,
  description:
    'A Discord webhook reminder, status page forwarder and feed watcher.',
  outDir: './dist',
  srcDir: 'src',
  locales: {
    root: {
      label: 'English',
      lang: 'en-US',
      themeConfig: { ...en, footer },
    },
    ja: {
      label: '日本語',
      lang: 'ja-JP',
      link: '/ja/',
      themeConfig: { ...ja, footer },
    },
  },
  title: 'chime',
  titleTemplate: 'chime',
  vite: {
    plugins: [
      llmstxt({
        domain: 'https://chime.m1sk9.dev',
        ignoreFiles: ['index.md', 'ja/index.md'],
      }),
    ],
  },
  themeConfig: {
    socialLinks: [{ icon: 'github', link: 'https://github.com/m1sk9/chime' }],
  },
});
