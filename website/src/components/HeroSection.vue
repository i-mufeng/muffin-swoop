<script setup lang="ts">
import { computed } from 'vue'
import { DOWNLOADS, GITHUB_RELEASES_URL } from '../config'
import { useLatestRelease } from '../composables/useLatestRelease'
import { useI18n } from '../i18n'

const { release } = useLatestRelease()
const { t } = useI18n()
const versionText = computed(() =>
  release.value
    ? `${release.value.version} · ${t().hero.latestRelease}`
    : t().hero.comingSoon,
)
</script>

<template>
  <section id="top" class="hero">
    <div class="container hero-inner">
      <a class="ver mono" :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">
        {{ versionText }}
      </a>
      <h1>{{ t().hero.titleA }}<span>{{ t().hero.titleB }}</span>{{ t().hero.titleEnd }}</h1>
      <p class="tagline">{{ t().hero.tagline }}</p>
      <div class="cta">
        <a class="btn btn-primary" :href="DOWNLOADS.windows">
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
          </svg>
          {{ t().hero.ctaWindows }}
        </a>
        <a class="btn btn-ghost" :href="DOWNLOADS.macos">{{ t().hero.ctaMac }}</a>
      </div>
      <div class="meta mono">
        <a :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">{{ t().hero.releases }}</a>
        <span>MIT</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.hero { position: relative; z-index: 1; }
.hero-inner {
  min-height: calc(100vh - 68px);
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  text-align: center;
  padding-top: 40px; padding-bottom: 60px;
}
.ver {
  font-size: 13px; color: var(--accent);
  border: 1px solid var(--accent-border); border-radius: 999px;
  padding: 7px 18px; background: var(--accent-soft);
  text-decoration: none; transition: filter var(--transition);
}
.ver:hover { filter: brightness(1.15); }
h1 { margin-top: 34px; font-size: 64px; font-weight: 700; letter-spacing: -2px; line-height: 1.15; }
h1 span { color: var(--dim); }
.tagline {
  margin-top: 24px; font-size: 16.5px; color: var(--dim);
  line-height: 1.9; max-width: 560px;
}
.cta { margin-top: 46px; display: flex; gap: 14px; flex-wrap: wrap; justify-content: center; }
.meta {
  margin-top: 60px; display: flex; gap: 36px; flex-wrap: wrap; justify-content: center;
  font-size: 12.5px; color: var(--dim);
}
.meta a { color: var(--accent); text-decoration: none; }
.meta a:hover { text-decoration: underline; }
@media (max-width: 640px) {
  h1 { font-size: 38px; letter-spacing: -1px; }
}
</style>
