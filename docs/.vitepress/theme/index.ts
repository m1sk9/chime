import { inBrowser, type Theme } from 'vitepress';
import DefaultTheme from 'vitepress/theme';
import { h } from 'vue';
import PageActions from './components/PageActions.vue';
import './custom.css';

const LOCALE_REDIRECT_FLAG = 'chime-locale-redirected';

function redirectByBrowserLanguage() {
  if (!inBrowser) return;
  if (localStorage.getItem(LOCALE_REDIRECT_FLAG)) return;
  localStorage.setItem(LOCALE_REDIRECT_FLAG, '1');

  const { pathname, search, hash } = window.location;
  const isJaPath = pathname === '/ja' || pathname.startsWith('/ja/');
  const prefersJa = navigator.language.toLowerCase().startsWith('ja');

  if (!isJaPath && prefersJa) {
    window.location.replace(`/ja${pathname}${search}${hash}`);
  }
}

export default {
  extends: DefaultTheme,
  Layout: () =>
    h(DefaultTheme.Layout, null, {
      'doc-before': () => h(PageActions),
    }),
  enhanceApp() {
    redirectByBrowserLanguage();
  },
} satisfies Theme;
