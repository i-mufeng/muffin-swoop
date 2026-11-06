<script setup lang="ts">
import { computed } from 'vue'
import { DOWNLOADS, GITHUB_RELEASES_URL, MICROSOFT_STORE_URL } from '../config'
import { useLatestRelease } from '../composables/useLatestRelease'
import { useI18n } from '../i18n'
import ScreenshotSection from './ScreenshotSection.vue'

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
      <div class="hero-copy">
      <a class="ver mono" :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">
        {{ versionText }}
      </a>
      <h1>{{ t().hero.titleA }}<span>{{ t().hero.titleB }}{{ t().hero.titleEnd }}</span></h1>
      <p class="tagline">{{ t().hero.tagline }}</p>
      <div class="cta">
        <div class="cta-row">
          <a class="btn btn-primary" :href="MICROSOFT_STORE_URL" target="_blank" rel="noopener">
            <svg viewBox="0 0 23 23" width="16" height="16" fill="currentColor" aria-hidden="true">
              <path d="M0 0h10.98v10.98H0zM12.02 0H23v10.98H12.02zM0 12.02h10.98V23H0zM12.02 12.02H23V23H12.02z" />
            </svg>
            {{ t().hero.ctaStore }}
          </a>
          <details class="dl-direct" @keydown.esc="($event.currentTarget as HTMLDetailsElement).open = false">
            <summary class="btn btn-ghost dl-trigger">
              {{ t().hero.ctaDirect }}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M18 15l-6-6-6 6" />
              </svg>
            </summary>
            <div class="dl-pop">
              <a :href="DOWNLOADS.windows">{{ t().hero.ctaWin }}</a>
              <a :href="DOWNLOADS.macos">{{ t().hero.ctaMac }}</a>
            </div>
          </details>
        </div>
      </div>
      <ul class="highlights" :aria-label="t().hero.highlightsLabel">
        <li v-for="highlight in t().hero.highlights" :key="highlight">{{ highlight }}</li>
      </ul>

          <details class="memory-details">
            <summary>{{ t().hero.memoryBadge }}</summary>
            <p>{{ t().hero.memoryNote }} <a href="https://github.com/i-mufeng/muffin-swoop/blob/fc172bbb4980c18ad3f0bc10ed4442e130eac600/docs/msstore.md#内存验证" target="_blank" rel="noopener">{{ t().hero.memorySource }}</a></p>
          </details>


      <div class="meta mono">
        <a :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">{{ t().hero.releases }}</a>
        <span>MIT</span>
      </div>
      </div>
      <div class="hero-preview"><ScreenshotSection /></div>
    </div>
  </section>
</template>

<style scoped>
.hero { position: relative; z-index: 1; border-bottom: 1px solid var(--hairline); }
.hero-inner {
  max-width: 1320px; display: grid; grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.25fr);
  align-items: center; gap: 56px; padding-block: 100px 96px;
}
.hero-copy { min-width: 0; }
.ver { display: inline-flex; align-items: center; gap: 10px; font-size: 12px; color: var(--dim); text-decoration: none; }
.ver::before { content: ''; width: 6px; height: 6px; background: var(--accent); border-radius: 50%; }
.ver:hover { color: var(--ink); }
h1 { margin-top: 24px; font-size: clamp(40px, 4.1vw, 56px); font-weight: 650; letter-spacing: -2px; line-height: 1.28; text-wrap: balance; }
h1 span { display: block; }
.tagline { margin-top: 22px; max-width: 380px; color: var(--dim); font-size: 16px; line-height: 1.9; text-wrap: pretty; }
.cta { margin-top: 32px; }
.cta-row { display: flex; gap: 20px; flex-wrap: wrap; align-items: center; }
.cta .btn-primary { padding: 14px 20px; border-radius: 8px; font-size: 14px; }
.cta .btn-ghost { font-size: 14px; }
.highlights { display: flex; flex-wrap: wrap; gap: 8px; list-style: none; margin-top: 28px; }
.highlights li { padding: 5px 10px; background: var(--card-bg-hover); border-radius: 5px; color: var(--dim); font-size: 12px; line-height: 1.5; white-space: nowrap; }
.memory-details { margin-top: 16px; font-size: 12px; color: var(--dim); }
.memory-details summary { display: list-item; cursor: pointer; width: fit-content; line-height: 1.8; }
.memory-details summary:hover { color: var(--ink); }
.memory-details summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; border-radius: 2px; }
.memory-details p { margin-top: 12px; line-height: 1.9; text-wrap: pretty; }
.memory-details a { color: var(--ink); }
.hero-preview { position: relative; min-width: 0; }
.hero-preview::before { content: ''; position: absolute; inset: 0; background: radial-gradient(ellipse at center, var(--accent-soft), transparent 70%); pointer-events: none; }

/* 平台选择使用原生 details，支持触控、键盘展开与 Esc 收起。 */
.dl-direct { position: relative; }
.dl-trigger { list-style: none; }
.dl-trigger::-webkit-details-marker { display: none; }
.dl-trigger svg { transition: transform var(--transition); }
.dl-direct[open] .dl-trigger svg { transform: rotate(180deg); }
.dl-pop {
  position: absolute; left: 50%; bottom: calc(100% + 10px);
  transform: translate(-50%, 6px);
  display: flex; flex-direction: column; gap: 2px; min-width: 156px;
  padding: 6px; border-radius: var(--radius);
  background: var(--bg); border: 1px solid var(--hairline);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.12);
  opacity: 0; visibility: hidden; pointer-events: none;
  transition: opacity var(--transition), transform var(--transition), visibility var(--transition);
  z-index: 20;
}
.dl-direct[open] .dl-pop {
  opacity: 1; visibility: visible; pointer-events: auto;
  transform: translate(-50%, 0);
}
.dl-pop a {
  display: flex; align-items: center; justify-content: space-between; gap: 16px;
  padding: 9px 14px; border-radius: 8px;
  font-size: 14px; font-weight: 500; color: var(--ink);
  text-decoration: none; white-space: nowrap;
  transition: background var(--transition), color var(--transition);
}
.dl-pop a::after {
  content: '↓'; font-size: 12px; color: var(--dim);
  transition: color var(--transition);
}
.dl-pop a:hover { background: var(--accent-soft); color: var(--accent); }
.dl-pop a:hover::after { color: var(--accent); }
.meta { margin-top: 20px; display: flex; gap: 20px; font-size: 11px; color: var(--dim); }
.meta a { color: inherit; text-decoration: none; }
.meta a:hover { color: var(--ink); }
@media (max-width: 1000px) {
  .hero-inner { gap: 32px; padding-block: 72px; grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr); }
  h1 { font-size: 42px; }
  .cta-row { gap: 12px; }
}
@media (max-width: 900px) {
  .hero-inner { grid-template-columns: minmax(0, 1fr); gap: 44px; padding: 48px 24px 48px; }
  .hero-copy { max-width: 520px; }
  h1 { font-size: clamp(34px, 7vw, 48px); letter-spacing: -1.5px; }
  .tagline { margin-top: 16px; }
  .hero-preview::before { inset: 0; }
  .cta { margin-top: 24px; }
}
@media (prefers-reduced-motion: reduce) {
  .dl-pop, .dl-trigger svg { transition: none; }
}
</style>
