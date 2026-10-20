import { ref, watch } from 'vue'

export type Theme = 'dark' | 'light'

const STORAGE_KEY = 'swoop-theme'

function initialTheme(): Theme {
  const saved = localStorage.getItem(STORAGE_KEY)
  if (saved === 'dark' || saved === 'light') return saved
  // 默认暗色，用户切换后记忆选择
  return 'dark'
}

const theme = ref<Theme>(initialTheme())

watch(
  theme,
  (t) => {
    document.documentElement.dataset.theme = t
    localStorage.setItem(STORAGE_KEY, t)
  },
  { immediate: true },
)

/** View Transitions API 的类型补充（TS 内置 lib 尚未收录） */
interface ViewTransition {
  ready: Promise<void>
}
interface DocumentWithVT extends Document {
  startViewTransition?: (callback: () => void) => ViewTransition
}

export function useTheme() {
  /**
   * 切换主题。传入点击坐标时，从该点圆形蔓延展开新主题；
   * 浏览器不支持 View Transitions 时静默降级为直接切换。
   */
  function toggle(x?: number, y?: number) {
    const cx = x ?? window.innerWidth - 60
    const cy = y ?? 40
    const apply = () => {
      theme.value = theme.value === 'dark' ? 'light' : 'dark'
    }

    const doc = document as DocumentWithVT
    if (!doc.startViewTransition || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      apply()
      return
    }

    const transition = doc.startViewTransition(apply)
    transition.ready.then(() => {
      const radius = Math.hypot(
        Math.max(cx, window.innerWidth - cx),
        Math.max(cy, window.innerHeight - cy),
      )
      document.documentElement.animate(
        {
          clipPath: [
            `circle(0px at ${cx}px ${cy}px)`,
            `circle(${radius}px at ${cx}px ${cy}px)`,
          ],
        },
        {
          duration: 550,
          easing: 'cubic-bezier(0.4, 0, 0.2, 1)',
          pseudoElement: '::view-transition-new(root)',
        },
      )
    })
  }

  return { theme, toggle }
}
