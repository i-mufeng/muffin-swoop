<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from '../i18n'

const { t } = useI18n()

const icons = [
  // rust / gear
  '<circle cx="12" cy="12" r="3.2" /><path d="M12 2.5v2.6M12 18.9v2.6M2.5 12h2.6M18.9 12h2.6M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M18.7 5.3l-1.8 1.8M7.1 16.9l-1.8 1.8" />',
  // threads
  '<path d="M4 6h16M4 12h10M4 18h16" /><path d="M18 10l3 2-3 2" />',
  // sniffer
  '<path d="M2.5 12s3.5-6.5 9.5-6.5S21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" /><circle cx="12" cy="12" r="2.8" />',
  // resume
  '<path d="M3 12a9 9 0 1 0 3-6.7" /><path d="M3 4v5h5" /><path d="M12 8v5l3.5 2" />',
  // media
  '<rect x="2.5" y="5" width="19" height="14" rx="2.5" /><path d="M10 9.5l5 2.5-5 2.5z" />',
  // mirror
  '<circle cx="12" cy="12" r="9" /><path d="M3.5 9h17M3.5 15h17M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18" />',
]

const items = computed(() =>
  t().features.items.map((f, i) => ({ ...f, icon: icons[i] })),
)
</script>

<template>
  <section id="features" class="features">
    <div class="container">
      <div class="section-label">{{ t().features.label }}</div>
      <h2 class="section-title">{{ t().features.title }}</h2>
      <div class="grid">
        <div v-for="f in items" :key="f.title" class="card">
          <div class="card-head">
            <span class="icon" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor"
                stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" v-html="f.icon" />
            </span>
            <span class="tag mono">{{ f.tag }}</span>
          </div>
          <h3>{{ f.title }}</h3>
          <p>{{ f.desc }}</p>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.features { position: relative; z-index: 1; padding: 110px 0 40px; }
.grid {
  margin-top: 48px;
  display: grid; grid-template-columns: repeat(3, 1fr); gap: 18px;
}
.card {
  border: 1px solid var(--hairline); border-radius: var(--radius);
  background: var(--card-bg);
  padding: 28px 26px;
  transition: background var(--transition), border-color var(--transition), transform var(--transition);
}
.card:hover {
  background: var(--card-bg-hover);
  border-color: var(--accent-border);
  transform: translateY(-3px);
}
.card-head { display: flex; align-items: center; justify-content: space-between; }
.icon {
  width: 40px; height: 40px; border-radius: 10px;
  display: grid; place-items: center;
  color: var(--accent); background: var(--accent-soft);
  border: 1px solid var(--accent-border);
}
.tag { font-size: 11px; color: var(--dim); letter-spacing: 0.5px; }
h3 { margin-top: 20px; font-size: 17.5px; font-weight: 650; }
p { margin-top: 10px; font-size: 14px; color: var(--dim); line-height: 1.85; }
@media (max-width: 900px) { .grid { grid-template-columns: repeat(2, 1fr); } }
@media (max-width: 600px) { .grid { grid-template-columns: 1fr; } }
</style>
