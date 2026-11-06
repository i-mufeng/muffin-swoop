import { ref, watch } from 'vue'

export type Locale = 'zh' | 'en'

const STORAGE_KEY = 'swoop-locale'

function initialLocale(): Locale {
  const saved = localStorage.getItem(STORAGE_KEY)
  if (saved === 'zh' || saved === 'en') return saved
  // 默认中文，用户切换后记忆选择
  return 'zh'
}

const locale = ref<Locale>(initialLocale())

watch(locale, (l) => {
  localStorage.setItem(STORAGE_KEY, l)
  document.documentElement.lang = l === 'zh' ? 'zh-CN' : 'en'
})

const messages = {
  zh: {
    nav: { features: '功能', shots: '截图', download: '下载', github: 'GitHub' },
    hero: {
      comingSoon: 'releases · coming soon',
      latestRelease: 'latest release',
      titleA: '轻量、快速的',
      titleB: '下载管理器',
      titleEnd: '。',
      tagline: 'Rust 原生构建，让下载更快，让日常更轻。',
      highlightsLabel: '产品特点',
      highlights: ['多线程加速', '断点续传', '浏览器接管'],
      memoryBadge: 'Windows 后台空闲约 3 MiB · 私有工作集',
      memoryNote: '2026-10-21 发布版测试：窗口隐藏、浏览器桥接开启，剪贴板监听、通知与浮窗关闭。后台空闲私有工作集为 2.32–3.29 MiB，总工作集为 6.91–9.53 MiB，Private Bytes 约 79–82 MiB。工作集表示物理驻留内存，不等于全部提交内存；实际占用随设备、配置和任务变化，不代表 macOS 或所有使用场景。',
      memorySource: '查看测试记录 ↗',
      ctaWindows: '下载 Windows 版',
      ctaMac: 'macOS 版',
      ctaStore: '在 Microsoft Store 安装',
      ctaDirect: '直接下载',
      ctaWin: 'Windows 版',
      releases: 'github releases →',
    },
    features: {
      label: 'features',
      title: '为什么选 Swoop',
      items: [
        { tag: 'core', title: 'Rust 原生', desc: '内存安全、零运行时依赖，一个小巧二进制即可运行，启动快、占用低。' },
        { tag: '1–32 conn', title: '多线程分段加速', desc: '默认 8 连接，最高 32 连接分段下载；动态分段再平衡，慢连接绝不拖垮整体速度。' },
        { tag: 'MV3', title: '浏览器接管 · 网页嗅探', desc: 'Chromium / Firefox 扩展在浏览器弹窗之前接管下载；被动嗅探页面音视频，随取随用。' },
        { tag: 'byte-exact', title: '断点续传', desc: '分段进度持续持久化，暂停、退出、重启后从断点字节精确恢复，ETag 变更自动校验。' },
        { tag: 'ffmpeg', title: '流媒体录制', desc: 'HLS / DASH / MMS 录制与直播时长上限，画质、音轨、字幕可选，转封装 MP4 / MKV。' },
        { tag: 'grabber', title: '整站镜像', desc: '按域名、深度、类型过滤抓取整站，支持 robots.txt 与预算上限，项目可断点续抓。' },
      ],
    },
    shots: {
      label: 'screenshots',
      title: '界面一览',
      themeDark: '深色',
      themeLight: '浅色',
      langZh: '中文',
      langEn: 'EN',
      alt: 'Swoop 主界面截图',
    },
    download: {
      label: 'download',
      title: '立即开始',
      latest: '最新版本',
      unreleased: '即将发布',
      winX64: 'x64 · 安装包',
      winArm: 'ARM64 · 安装包',
      macArm: 'Apple Silicon · DMG',
      storeArch: 'x64 / ARM64 · 自动更新',
      action: '下载',
      extName: '浏览器扩展',
      extEdge: 'Microsoft Edge · 加载项商店',
      extChromium: 'Chromium（360 / QQ 等）· 拖拽安装',
      extUnpacked: 'Chromium · 解压加载（兜底）',
      extFirefox: 'Mozilla Firefox · 静默安装',
      extAction: '前往安装',
      history: '需要历史版本或更新包？',
      allReleases: '查看全部 GitHub Releases →',
    },
    footer: { license: 'MIT license' },
  },
  en: {
    nav: { features: 'Features', shots: 'Screenshots', download: 'Download', github: 'GitHub' },
    hero: {
      comingSoon: 'releases · coming soon',
      latestRelease: 'latest release',
      titleA: 'A lightweight, fast ',
      titleB: 'download manager',
      titleEnd: '.',
      tagline: 'Built natively in Rust. Faster downloads, a lighter everyday experience.',
      highlightsLabel: 'Product highlights',
      highlights: ['Multi-connection', 'Resume downloads', 'Browser integration'],
      memoryBadge: 'Windows background idle ≈ 3 MiB · private working set',
      memoryNote: 'Release build tested on October 21, 2026: window hidden, browser bridge enabled; clipboard monitoring, notifications and floating window disabled. Idle private working set: 2.32–3.29 MiB; total working set: 6.91–9.53 MiB; Private Bytes: about 79–82 MiB. Working set measures physical residency, not all committed memory. Usage varies by device, settings and workload; these results do not cover macOS or every usage scenario.',
      memorySource: 'View test record ↗',
      ctaWindows: 'Download for Windows',
      ctaMac: 'macOS version',
      ctaStore: 'Get it from Microsoft Store',
      ctaDirect: 'Direct download',
      ctaWin: 'Windows',
      releases: 'github releases →',
    },
    features: {
      label: 'features',
      title: 'Why Swoop',
      items: [
        { tag: 'core', title: 'Native Rust', desc: 'Memory-safe with zero runtime dependencies. One small binary — fast to launch, light on resources.' },
        { tag: '1–32 conn', title: 'Multi-connection acceleration', desc: '8 connections by default, up to 32 per task. Dynamic segment rebalancing so one slow connection never stalls the file.' },
        { tag: 'MV3', title: 'Browser takeover & sniffing', desc: 'Chromium / Firefox extension intercepts downloads before the browser dialog; passive media sniffing on any page.' },
        { tag: 'byte-exact', title: 'Resumable downloads', desc: 'Per-segment progress is persisted continuously. Pause, quit, reboot — resume from the exact byte, with ETag revalidation.' },
        { tag: 'ffmpeg', title: 'Stream recording', desc: 'HLS / DASH / MMS recording with live duration caps; quality, audio track and subtitle selection; remux to MP4 / MKV.' },
        { tag: 'grabber', title: 'Site mirroring', desc: 'Mirror whole sites with domain, depth and type filters, robots.txt support, budget limits and resumable projects.' },
      ],
    },
    shots: {
      label: 'screenshots',
      title: 'Interface',
      themeDark: 'Dark',
      themeLight: 'Light',
      langZh: '中文',
      langEn: 'EN',
      alt: 'Swoop main window screenshot',
    },
    download: {
      label: 'download',
      title: 'Get started',
      latest: 'Latest version',
      unreleased: 'coming soon',
      winX64: 'x64 · Installer',
      winArm: 'ARM64 · Installer',
      macArm: 'Apple Silicon · DMG',
      storeArch: 'x64 / ARM64 · auto-updates',
      action: 'Download',
      extName: 'Browser extension',
      extEdge: 'Microsoft Edge · Add-ons',
      extChromium: 'Chromium (360 / QQ, etc.) · drag-and-drop install',
      extUnpacked: 'Chromium · load unpacked (fallback)',
      extFirefox: 'Mozilla Firefox · silent install',
      extAction: 'Get',
      history: 'Need older versions or update packages?',
      allReleases: 'Browse all GitHub Releases →',
    },
    footer: { license: 'MIT license' },
  },
} as const

export type Messages = (typeof messages)[Locale]

export function useI18n() {
  function t(): Messages {
    return messages[locale.value]
  }
  function toggleLocale() {
    locale.value = locale.value === 'zh' ? 'en' : 'zh'
  }
  return { locale, t, toggleLocale }
}
