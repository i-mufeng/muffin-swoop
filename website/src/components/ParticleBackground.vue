<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { tsParticles } from '@tsparticles/engine'
import { loadSlim } from '@tsparticles/slim'
import type { Container, ISourceOptions } from '@tsparticles/engine'
import { useTheme } from '../composables/useTheme'

const { theme } = useTheme()
const el = ref<HTMLElement | null>(null)
let container: Container | undefined

function particleColors() {
  const styles = getComputedStyle(document.documentElement)
  return {
    color: styles.getPropertyValue('--particle-color').trim() || '#34d399',
    link: styles.getPropertyValue('--particle-link').trim() || '#1f8f68',
  }
}

function buildOptions(): ISourceOptions {
  const { color, link } = particleColors()
  return {
    fullScreen: false,
    fpsLimit: 60,
    detectRetina: true,
    background: { color: { value: 'transparent' } },
    particles: {
      number: { value: 70, density: { enable: true, width: 1400, height: 900 } },
      color: { value: color },
      shape: { type: 'circle' },
      opacity: { value: { min: 0.15, max: 0.5 } },
      size: { value: { min: 1, max: 2.4 } },
      links: {
        enable: true,
        distance: 130,
        color: link,
        opacity: 0.25,
        width: 1,
      },
      move: {
        enable: true,
        speed: 0.7,
        direction: 'none',
        random: true,
        straight: false,
        outModes: { default: 'out' },
      },
    },
    interactivity: {
      detectsOn: 'window',
      events: {
        onHover: { enable: true, mode: 'grab' },
        onClick: { enable: true, mode: 'push' },
      },
      modes: {
        grab: { distance: 180, links: { opacity: 0.55, color } },
        push: { quantity: 3 },
      },
    },
  }
}

async function init() {
  if (!el.value) return
  await loadSlim(tsParticles)
  container = await tsParticles.load({ element: el.value, options: buildOptions() })
}

async function refreshColors() {
  if (!container) return
  const { color, link } = particleColors()
  container.options.particles!.color!.value = color
  if (container.options.particles!.links) container.options.particles!.links.color = link
  if (container.options.interactivity?.modes?.grab?.links) {
    container.options.interactivity.modes.grab.links.color = color
  }
  await container.refresh()
}

onMounted(init)
watch(theme, () => setTimeout(refreshColors, 50))
onBeforeUnmount(() => container?.destroy())
</script>

<template>
  <div ref="el" class="particle-bg" aria-hidden="true"></div>
</template>

<style scoped>
.particle-bg {
  position: fixed;
  inset: 0;
  z-index: 0;
  pointer-events: none;
}
</style>
