<script setup lang="ts">
import { GITHUB_URL, GITHUB_RELEASES_URL } from '../config'
import { useI18n } from '../i18n'

const { t } = useI18n()
</script>

<template>
  <footer class="footer">
    <div class="container footer-inner">
      <span class="mono">© {{ new Date().getFullYear() }} Muffin · swoop.muffin.xin</span>
      <nav class="f-links mono">
        <a :href="GITHUB_URL" target="_blank" rel="noopener">github</a>
        <a :href="GITHUB_RELEASES_URL" target="_blank" rel="noopener">releases</a>
        <span>{{ t().footer.license }}</span>
      </nav>
    </div>
  </footer>
</template>

<style scoped>
.footer {
  position: relative; z-index: 1;
  border-top: 1px solid var(--hairline);
}
.footer-inner {
  height: 76px;
  display: flex; align-items: center; justify-content: space-between;
  font-size: 12.5px; color: var(--dim);
}
.f-links { display: flex; gap: 26px; }
.f-links a { color: var(--dim); text-decoration: none; transition: color var(--transition); }
.f-links a:hover { color: var(--accent); }
@media (max-width: 560px) {
  .footer-inner { flex-direction: column; justify-content: center; gap: 8px; height: auto; padding: 20px 32px; }
}
</style>
