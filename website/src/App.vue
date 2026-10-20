<script setup lang="ts">
import { onMounted } from 'vue'
import ParticleBackground from './components/ParticleBackground.vue'
import NavBar from './components/NavBar.vue'
import HeroSection from './components/HeroSection.vue'
import FeaturesSection from './components/FeaturesSection.vue'
import ScreenshotSection from './components/ScreenshotSection.vue'
import DownloadSection from './components/DownloadSection.vue'
import SiteFooter from './components/SiteFooter.vue'

/** 页面资源就绪后淡出开屏加载动画（最多等待 3 秒兜底） */
onMounted(() => {
  const splash = document.getElementById('splash')
  if (!splash) return
  const dismiss = () => {
    splash.classList.add('hide')
    setTimeout(() => splash.remove(), 500)
  }
  if (document.readyState === 'complete') {
    dismiss()
  } else {
    window.addEventListener('load', dismiss, { once: true })
    setTimeout(dismiss, 3000)
  }
})
</script>

<template>
  <ParticleBackground />
  <NavBar />
  <main>
    <HeroSection />
    <FeaturesSection />
    <ScreenshotSection />
    <DownloadSection />
  </main>
  <SiteFooter />
</template>
