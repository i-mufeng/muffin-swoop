/**
 * 站点全局配置 —— 下载地址、仓库地址集中在这里，后续修改只动这一个文件。
 */

/** CDN 下载基地址（最新版分发） */
export const CDN_BASE = 'https://cdn.muffin.xin/swoop'

/** GitHub 仓库 */
export const GITHUB_REPO = 'i-mufeng/muffin-swoop'
export const GITHUB_URL = `https://github.com/${GITHUB_REPO}`
export const GITHUB_RELEASES_URL = `${GITHUB_URL}/releases`
export const GITHUB_LATEST_RELEASE_API = `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`

/** CDN 更新 feed（版本号与下载地址的唯一权威来源） */
export const CDN_FEED_URL = `${CDN_BASE}-release/feed.json`

/** 各平台下载地址（CDN 优先；feed 不可用时的兜底）
 *  与 scripts/publish-upyun.sh --latest 的改名规则保持一致：
 *  去版本段、去 -release、转小写，架构段保留。 */
export const DOWNLOADS = {
  windows: `${CDN_BASE}/latest/swoop-windows-x86_64-setup.exe`,
  windowsArm: `${CDN_BASE}/latest/swoop-windows-arm64-setup.exe`,
  macos: `${CDN_BASE}/latest/swoop-macos-arm64.dmg`,
}

/** Microsoft Store 应用页（Swoop 桌面版） */
export const MICROSOFT_STORE_URL =
  'https://apps.microsoft.com/detail/9P9FNXJXK17N?hl=zh-cn&gl=CN&ocid=pdpshare'

/** Edge 加载项商店（Swoop 扩展） */
export const EDGE_ADDON_URL =
  'https://microsoftedge.microsoft.com/addons/detail/swoop/ecoapkmgaijhekboiplkojnpefddffkh'

/** 浏览器扩展下载（CDN latest 直链；命名与 publish-upyun.sh --latest 的规则一致） */
export const EXTENSION_DOWNLOADS = {
  chromium: `${CDN_BASE}/latest/swoop-extension-chromium.crx`,
  chromiumUnpacked: `${CDN_BASE}/latest/swoop-extension-chromium-unpacked.zip`,
  firefox: `${CDN_BASE}/latest/swoop-extension-firefox.xpi`,
}

/** feed platforms 键名 → 站内下载卡片 */
export const PLATFORM_KEYS = {
  windows: 'windows-x86_64',
  windowsArm: 'windows-aarch64',
  macos: 'macos-aarch64',
} as const

/**
 * 截图展示素材：明暗 × 中英文共 4 张，放入 public/shots/ 即可生效。
 * 当前仓库只有 home-dark（英文），其余组合暂时回退到它；
 * 补齐 home-light-en.png / home-dark-zh.png / home-light-zh.png 后自动启用。
 */
export const SCREENSHOTS: Record<string, string> = {
  'dark-en': '/shots/home-dark-en.png',
  'light-en': '/shots/home-light-en.png',
  'dark-zh': '/shots/home-dark-zh.png',
  'light-zh': '/shots/home-light-zh.png',
}
export const SCREENSHOT_FALLBACK = '/shots/home-dark-en.png'
