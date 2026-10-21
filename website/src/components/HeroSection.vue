<script setup lang="ts">
import { computed } from 'vue'
import { DOWNLOADS, GITHUB_RELEASES_URL, MICROSOFT_STORE_URL } from '../config'
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
        <div class="cta-row">
          <a class="btn btn-primary" :href="MICROSOFT_STORE_URL" target="_blank" rel="noopener">
            <svg viewBox="0 0 23 23" width="16" height="16" fill="currentColor" aria-hidden="true">
              <path d="M0 0h10.98v10.98H0zM12.02 0H23v10.98H12.02zM0 12.02h10.98V23H0zM12.02 12.02H23V23H12.02z" />
            </svg>
            {{ t().hero.ctaStore }}
          </a>
          <div class="dl-direct">
            <button class="btn btn-ghost dl-trigger" type="button">
              {{ t().hero.ctaDirect }}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M18 15l-6-6-6 6" />
              </svg>
            </button>
            <div class="dl-pop">
              <a :href="DOWNLOADS.windows">{{ t().hero.ctaWin }}</a>
              <a :href="DOWNLOADS.macos">{{ t().hero.ctaMac }}</a>
            </div>
          </div>
        </div>
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
.cta { margin-top: 46px; display: flex; flex-direction: column; align-items: center; gap: 4px; }
.cta-row { display: flex; gap: 14px; flex-wrap: wrap; justify-content: center; align-items: center; }

/* 直接下载：悬停上浮平台选择浮层 */
.dl-direct { position: relative; }
.dl-trigger svg { transition: transform var(--transition); }
.dl-direct:hover .dl-trigger svg,
.dl-direct:focus-within .dl-trigger svg { transform: rotate(180deg); }
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
/* 悬停桥：填平触发器与浮层之间的空隙，鼠标移上去不会断 */
.dl-pop::before {
  content: ''; position: absolute; left: 0; right: 0; bottom: -12px; height: 12px;
}
.dl-direct:hover .dl-pop,
.dl-direct:focus-within .dl-pop {
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
