<script setup lang="ts">
import { GITHUB_URL } from '../config'
import { useTheme } from '../composables/useTheme'
import { useI18n } from '../i18n'

const { theme, toggle } = useTheme()
const { locale, t, toggleLocale } = useI18n()
</script>

<template>
  <header class="nav">
    <div class="container nav-inner">
      <a class="logo mono" href="#top">swoop<span>.</span></a>
      <nav class="links">
        <a href="#features">{{ t().nav.features }}</a>
        <a href="#shots">{{ t().nav.shots }}</a>
        <a href="#download">{{ t().nav.download }}</a>
        <a :href="GITHUB_URL" target="_blank" rel="noopener">{{ t().nav.github }}</a>
        <div class="lang-seg mono" role="group" aria-label="language">
          <button :class="{ on: locale === 'zh' }" @click="locale === 'zh' || toggleLocale()">中</button>
          <button :class="{ on: locale === 'en' }" @click="locale === 'en' || toggleLocale()">EN</button>
        </div>
        <button class="theme-btn" :title="theme === 'dark' ? 'Light' : 'Dark'" @click="toggle($event.clientX, $event.clientY)">
          <svg v-if="theme === 'dark'" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
          </svg>
          <svg v-else viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
          </svg>
        </button>
      </nav>
    </div>
  </header>
</template>

<style scoped>
.nav {
  position: sticky; top: 0; z-index: 10;
  backdrop-filter: blur(12px);
  background: color-mix(in srgb, var(--bg) 75%, transparent);
  border-bottom: 1px solid var(--hairline);
}
.nav-inner {
  height: 68px;
  display: flex; align-items: center; justify-content: space-between;
}
.logo { font-size: 17px; font-weight: 700; letter-spacing: 1px; text-decoration: none; }
.logo span { color: var(--accent); }
.links { display: flex; gap: 28px; align-items: center; font-size: 14px; }
.links a { color: var(--dim); text-decoration: none; transition: color var(--transition); }
.links a:hover { color: var(--ink); }
.lang-seg {
  display: inline-flex;
  border: 1px solid var(--hairline); border-radius: 999px; overflow: hidden;
}
.lang-seg button {
  border: none; background: transparent; color: var(--dim);
  font-size: 12px; font-weight: 600; letter-spacing: 0.5px;
  padding: 7px 13px; cursor: pointer; font-family: var(--font-mono);
  transition: background var(--transition), color var(--transition);
}
.lang-seg button.on { background: var(--accent-soft); color: var(--accent); }
.lang-seg button:not(.on):hover { color: var(--ink); }
.theme-btn {
  width: 34px; height: 34px; border-radius: 50%;
  border: 1px solid var(--hairline); background: transparent;
  color: var(--ink); cursor: pointer;
  display: grid; place-items: center;
  transition: border-color var(--transition), background var(--transition);
}
.theme-btn:hover { border-color: var(--accent-border); background: var(--accent-soft); }
@media (max-width: 640px) {
  .links { gap: 16px; font-size: 13px; }
}
</style>
