<script setup lang="ts">
import { computed } from 'vue'
import { DOWNLOADS, EDGE_ADDON_URL, EXTENSION_DOWNLOADS, GITHUB_RELEASES_URL, PLATFORM_KEYS } from '../config'
import { useLatestRelease } from '../composables/useLatestRelease'
import { useI18n } from '../i18n'

const { release } = useLatestRelease()
const { t } = useI18n()
const version = computed(() => release.value?.version ?? null)

/** 下载地址优先取 CDN feed 中的产物地址，未发布时回退到 CDN latest 直链 */
function artifact(platform: keyof typeof PLATFORM_KEYS, fallback: string) {
  const a = release.value?.platforms?.[PLATFORM_KEYS[platform]]
  return { url: a?.url ?? fallback, size: a?.size ?? 0 }
}

function formatSize(bytes: number): string {
  if (!bytes) return ''
  return `${(bytes / 1024 / 1024).toFixed(0)} MB`
}

const platforms = computed(() => [
  { name: 'Windows', arch: t().download.winX64, ...artifact('windows', DOWNLOADS.windows) },
  { name: 'Windows', arch: t().download.winArm, ...artifact('windowsArm', DOWNLOADS.windowsArm) },
  { name: 'macOS', arch: t().download.macArm, ...artifact('macos', DOWNLOADS.macos) },
])
</script>

<template>
  <section id="download" class="download">
    <div class="container">
      <div class="section-label">{{ t().download.label }}</div>
      <h2 class="section-title">{{ t().download.title }}</h2>
      <p class="sub">
        {{ t().download.latest }}
        <b class="mono">{{ version ?? t().download.unreleased }}</b>
        <template v-if="release?.date"> · {{ release.date }}</template>
      </p>

      <div class="platforms">
        <a
          v-for="p in platforms"
          :key="p.name + p.arch"
          class="platform"
          :href="p.url"
        >
          <span class="p-name">{{ p.name }}</span>
          <span class="p-arch mono">{{ p.arch }}<template v-if="p.size"> · {{ formatSize(p.size) }}</template></span>
          <span class="p-action">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
            </svg>
            {{ t().download.action }}
          </span>
        </a>
      </div>

      <div class="ext-label">{{ t().download.extName }}</div>
      <div class="platforms ext-grid">
        <a class="platform" :href="EDGE_ADDON_URL" target="_blank" rel="noopener">
          <span class="p-name">Edge</span>
          <span class="p-arch mono">{{ t().download.extEdge }}</span>
          <span class="p-action">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
            </svg>
            {{ t().download.extAction }}
          </span>
        </a>
        <a class="platform" :href="EXTENSION_DOWNLOADS.chromium" :download="true">
          <span class="p-name">Chromium</span>
          <span class="p-arch mono">{{ t().download.extChromium }}</span>
          <span class="p-action">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
            </svg>
            {{ t().download.action }}
          </span>
        </a>
        <a class="platform" :href="EXTENSION_DOWNLOADS.chromiumUnpacked" :download="true">
          <span class="p-name">Chromium</span>
          <span class="p-arch mono">{{ t().download.extUnpacked }}</span>
          <span class="p-action">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
            </svg>
            {{ t().download.action }}
          </span>
        </a>
        <a class="platform" :href="EXTENSION_DOWNLOADS.firefox" :download="true">
          <span class="p-name">Firefox</span>
          <span class="p-arch mono">{{ t().download.extFirefox }}</span>
          <span class="p-action">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12m0 0l-4-4m4 4l4-4M4 21h16" />
            </svg>
            {{ t().download.action }}
          </span>
        </a>
      </div>

      <p class="all-releases">
        {{ t().download.history }}
        <a :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">{{ t().download.allReleases }}</a>
      </p>
    </div>
  </section>
</template>

<style scoped>
.download { position: relative; z-index: 1; padding: 110px 0 90px; }
.sub { margin-top: 20px; font-size: 15.5px; color: var(--dim); }
.sub b { color: var(--ink); font-weight: 600; }
.platforms {
  margin-top: 44px;
  display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px;
}
.platform {
  display: flex; flex-direction: column; gap: 6px;
  border: 1px solid var(--hairline); border-radius: var(--radius);
  background: var(--card-bg);
  padding: 26px 24px;
  text-decoration: none;
  transition: border-color var(--transition), background var(--transition), transform var(--transition);
}
.platform:hover {
  border-color: var(--accent-border);
  background: var(--card-bg-hover);
  transform: translateY(-3px);
}
.p-name { font-size: 18px; font-weight: 650; }
.p-arch { font-size: 12px; color: var(--dim); }
.p-action {
  margin-top: 16px; display: inline-flex; align-items: center; gap: 7px;
  font-size: 13.5px; font-weight: 600; color: var(--accent);
}
.all-releases { margin-top: 36px; font-size: 14px; color: var(--dim); }
.all-releases a {
  color: var(--ink); text-decoration: none;
  border-bottom: 1px solid var(--hairline); padding-bottom: 1px;
  transition: color var(--transition), border-color var(--transition);
}
.all-releases a:hover { color: var(--accent); border-bottom-color: var(--accent); }
.ext-label { margin-top: 44px; font-size: 13px; font-weight: 650; letter-spacing: .08em; text-transform: uppercase; color: var(--dim); }
.ext-grid { grid-template-columns: repeat(4, 1fr); }
@media (max-width: 900px) { .ext-grid { grid-template-columns: repeat(2, 1fr); } }
@media (max-width: 760px) { .platforms { grid-template-columns: 1fr; } }
</style>
