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
      tagline: 'Rust 编写的多线程下载管理器：分段加速、断点续传、流媒体录制、整站镜像，一个小巧二进制，没有多余的一切。',
      ctaWindows: '下载 Windows 版',
      ctaMac: 'macOS 版',
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
      tagline: 'A multi-connection download manager written in Rust: segmented acceleration, resumable downloads, stream recording and site mirroring — one small binary, nothing more.',
      ctaWindows: 'Download for Windows',
      ctaMac: 'macOS version',
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

export type Messages = (typeof messages)['zh']

export function useI18n() {
  function t(): Messages {
    return messages[locale.value]
  }
  function toggleLocale() {
    locale.value = locale.value === 'zh' ? 'en' : 'zh'
  }
  return { locale, t, toggleLocale }
}
