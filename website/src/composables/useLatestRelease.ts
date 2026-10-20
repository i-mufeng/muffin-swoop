import { onMounted, ref } from 'vue'
import { CDN_FEED_URL, GITHUB_LATEST_RELEASE_API, GITHUB_RELEASES_URL } from '../config'

export interface ArtifactInfo {
  url: string
  size: number
}

export interface ReleaseInfo {
  /** 展示用版本号，统一带 v 前缀 */
  version: string
  url: string
  date: string
  /** 平台键 → 产物（url / size），来自 CDN feed */
  platforms: Record<string, ArtifactInfo>
}

const release = ref<ReleaseInfo | null>(null)
const loaded = ref(false)

function withV(version: string): string {
  return version.startsWith('v') ? version : `v${version}`
}

/** 主数据源：cdn.muffin.xin 的更新 feed（签名清单，payload 为 JSON 字符串） */
async function fromFeed(): Promise<ReleaseInfo | null> {
  const resp = await fetch(CDN_FEED_URL)
  if (!resp.ok) return null
  const feed = await resp.json()
  const payload = JSON.parse(feed?.payload ?? 'null')
  if (!payload?.version) return null
  const platforms: Record<string, ArtifactInfo> = {}
  for (const [key, a] of Object.entries(payload.platforms ?? {}) as [string, any][]) {
    if (a?.url) platforms[key] = { url: a.url, size: a.size ?? 0 }
  }
  return { version: withV(payload.version), url: GITHUB_RELEASES_URL, date: '', platforms }
}

/** 备用数据源：GitHub API（feed 不可用时） */
async function fromGithub(): Promise<ReleaseInfo | null> {
  const resp = await fetch(GITHUB_LATEST_RELEASE_API, {
    headers: { Accept: 'application/vnd.github+json' },
  })
  if (!resp.ok) return null
  const data = await resp.json()
  if (!data?.tag_name) return null
  return {
    version: withV(data.tag_name),
    url: data.html_url ?? GITHUB_RELEASES_URL,
    date: (data.published_at ?? '').slice(0, 10),
    platforms: {},
  }
}

/**
 * 运行时获取最新版本：优先 CDN feed，失败回退 GitHub API，
 * 两者都不可用时保持 null，界面优雅降级。
 */
export function useLatestRelease() {
  onMounted(async () => {
    try {
      release.value = (await fromFeed()) ?? (await fromGithub())
    } catch {
      /* 静默降级 */
    } finally {
      loaded.value = true
    }
  })

  return { release, loaded }
}
