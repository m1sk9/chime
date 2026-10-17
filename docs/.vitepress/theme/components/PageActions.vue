<script setup lang="ts">
import { useData } from 'vitepress';
import { computed, ref } from 'vue';

const { page, lang } = useData();

const labels = computed(() =>
  lang.value.startsWith('ja')
    ? {
        copy: 'Markdown をコピー',
        copied: 'コピーしました',
        failed: 'コピーに失敗しました',
        view: 'Markdown で開く',
      }
    : {
        copy: 'Copy as Markdown',
        copied: 'Copied',
        failed: 'Copy failed',
        view: 'View as Markdown',
      },
);

// The llms plugin emits every page as <path>.md next to the HTML.
const markdownUrl = computed(
  () => `/${page.value.relativePath.replace(/\.md$/, '')}.md`,
);

const state = ref<'idle' | 'copied' | 'failed'>('idle');

async function copy() {
  try {
    const res = await fetch(markdownUrl.value);
    if (!res.ok) throw new Error(String(res.status));
    // Drop the `url:` frontmatter the plugin prepends; it is noise for a reader.
    const text = (await res.text()).replace(/^---\n[\s\S]*?\n---\n+/, '');
    await navigator.clipboard.writeText(text);
    state.value = 'copied';
  } catch {
    state.value = 'failed';
  }
  setTimeout(() => {
    state.value = 'idle';
  }, 2000);
}
</script>

<template>
  <div class="page-actions">
    <button type="button" class="page-actions-button" @click="copy">
      {{ state === 'idle' ? labels.copy : labels[state] }}
    </button>
    <a class="page-actions-button" :href="markdownUrl" target="_blank" rel="noopener">
      {{ labels.view }}
    </a>
  </div>
</template>
