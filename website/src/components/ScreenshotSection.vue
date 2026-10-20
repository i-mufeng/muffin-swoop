<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { SCREENSHOTS, SCREENSHOT_FALLBACK } from '../config'
import { useI18n } from '../i18n'
import { useTheme } from '../composables/useTheme'

const { t, locale } = useI18n()
const { theme } = useTheme()

/**
 * 截图的明暗与语言默认跟随网站本身；
 * 只有用户点击过对应的分段按钮后才脱离跟随、独立固定；
 * 一旦顶部全局切换了语言/主题，则忽略分段按钮的本地状态、恢复跟随。
 */
const themeOverridden = ref(false)
const langOverridden = ref(false)
const shotThemeManual = ref<'dark' | 'light'>('dark')
const shotLangManual = ref<'zh' | 'en'>('zh')

const shotTheme = computed<'dark' | 'light'>(() =>
  themeOverridden.value ? shotThemeManual.value : theme.value,
)
const shotLang = computed<'zh' | 'en'>(() =>
  langOverridden.value ? shotLangManual.value : locale.value,
)

// 顶部分别切换了全局语言/主题 → 对应维度恢复跟随全局
watch(locale, () => { langOverridden.value = false })
watch(theme, () => { themeOverridden.value = false })

function pickTheme(v: 'dark' | 'light') {
  shotThemeManual.value = v
  themeOverridden.value = true
}
function pickLang(v: 'zh' | 'en') {
  shotLangManual.value = v
  langOverridden.value = true
}

const src = computed(() => {
  const key = `${shotTheme.value}-${shotLang.value}`
  return SCREENSHOTS[key] ?? SCREENSHOT_FALLBACK
})

/** 图片加载失败时回退到已有截图（素材未补齐时也能正常展示） */
function onError(e: Event) {
  const img = e.target as HTMLImageElement
  if (!img.src.endsWith(SCREENSHOT_FALLBACK)) img.src = SCREENSHOT_FALLBACK
}

/** 预加载全部截图，切换时不等待网络/解码，避免闪白 */
onMounted(() => {
  for (const url of Object.values(SCREENSHOTS)) {
    const img = new Image()
    img.src = url
  }
})
</script>

<template>
  <section id="shots" class="shots">
    <div class="container">
      <div class="section-label">{{ t().shots.label }}</div>
      <h2 class="section-title">{{ t().shots.title }}</h2>

      <div class="controls">
        <div class="seg mono">
          <button :class="{ on: shotTheme === 'dark' }" @click="pickTheme('dark')">
            {{ t().shots.themeDark }}
          </button>
          <button :class="{ on: shotTheme === 'light' }" @click="pickTheme('light')">
            {{ t().shots.themeLight }}
          </button>
        </div>
        <div class="seg mono">
          <button :class="{ on: shotLang === 'zh' }" @click="pickLang('zh')">
            {{ t().shots.langZh }}
          </button>
          <button :class="{ on: shotLang === 'en' }" @click="pickLang('en')">
            {{ t().shots.langEn }}
          </button>
        </div>
      </div>

      <div class="frame">
        <transition name="fade">
          <img :key="src" :src="src" :alt="t().shots.alt" loading="eager" @error="onError" />
        </transition>
      </div>
    </div>
  </section>
</template>

<style scoped>
.shots { position: relative; z-index: 1; padding: 90px 0 40px; }
.controls { margin-top: 32px; display: flex; gap: 14px; flex-wrap: wrap; }
.seg {
  display: inline-flex;
  border: 1px solid var(--hairline); border-radius: 999px; overflow: hidden;
}
.seg button {
  border: none; background: transparent; color: var(--dim);
  font-size: 12.5px; font-weight: 600; letter-spacing: 0.5px;
  padding: 9px 20px; cursor: pointer;
  transition: background var(--transition), color var(--transition);
}
.seg button.on { background: var(--accent-soft); color: var(--accent); }
.seg button:not(.on):hover { color: var(--ink); }
.frame {
  position: relative;
  margin-top: 28px;
  border: 1px solid var(--hairline); border-radius: var(--radius);
  overflow: hidden;
  background: var(--card-bg);
  box-shadow: 0 24px 80px rgba(0, 0, 0, 0.25);
}
.frame img { display: block; width: 100%; height: auto; }
/* 交叉淡入：旧图绝对定位覆盖在新图上方淡出，无空白帧 */
.fade-enter-active { transition: opacity 0.3s ease; }
.fade-leave-active {
  transition: opacity 0.3s ease;
  position: absolute; inset: 0;
}
.fade-enter-from, .fade-leave-to { opacity: 0; }
</style>
