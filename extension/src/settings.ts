import { browser } from "./browser";
import { normalizeLanguage, type Language } from "./i18n";
// 用户设置（storage.sync，跟随浏览器账号同步）与运行状态（storage.session，浏览器关闭即清空）。

export interface Settings {
  language: Language;
  /** 总开关：关闭后浏览器下载完全不受影响 */
  enabled: boolean;
  /** Swoop 本地服务端口，需与 Swoop 设置里一致 */
  port: number;
  /** 这些域名（含子域名）的下载留给浏览器 */
  excludedDomains: string[];
  /** 小于这个大小（MB）的下载留给浏览器；0 表示不限 */
  minSizeMB: number;
  /** 本页链接逐项提交的并发数（1–6） */
  batchConcurrency: number;
  /** 每页最多保留的去重链接数（100–10000） */
  maxPageLinks: number;
  detectMedia: boolean;
  maxMediaCandidates: number;
  mediaRetentionMinutes: number;
}

export const DEFAULT_SETTINGS: Settings = {
  language: "zh-CN",
  enabled: true,
  port: 51230,
  excludedDomains: [],
  minSizeMB: 0,
  batchConcurrency: 2,
  maxPageLinks: 2000,
  detectMedia: true,
  maxMediaCandidates: 100,
  mediaRetentionMinutes: 60,
};

/** 把存储里可能残缺或被手改过的值规整成合法设置 */
export function normalizeSettings(raw: Partial<Record<keyof Settings, unknown>>): Settings {
  const port = Number(raw.port);
  const minSizeMB = Number(raw.minSizeMB);
  const batchConcurrency = Number(raw.batchConcurrency);
  const maxPageLinks = Number(raw.maxPageLinks);
  const maxMediaCandidates = Number(raw.maxMediaCandidates);
  const mediaRetentionMinutes = Number(raw.mediaRetentionMinutes);
  return {
    language: normalizeLanguage(raw.language),
    enabled: typeof raw.enabled === "boolean" ? raw.enabled : DEFAULT_SETTINGS.enabled,
    port: Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : DEFAULT_SETTINGS.port,
    excludedDomains: Array.isArray(raw.excludedDomains)
      ? raw.excludedDomains.filter((d): d is string => typeof d === "string" && d.length > 0)
      : [],
    minSizeMB: Number.isFinite(minSizeMB) && minSizeMB > 0 ? minSizeMB : 0,
    batchConcurrency: Number.isInteger(batchConcurrency) && batchConcurrency >= 1 && batchConcurrency <= 6
      ? batchConcurrency : DEFAULT_SETTINGS.batchConcurrency,
    maxPageLinks: Number.isInteger(maxPageLinks) && maxPageLinks >= 100 && maxPageLinks <= 10000
      ? maxPageLinks : DEFAULT_SETTINGS.maxPageLinks,
    detectMedia: typeof raw.detectMedia === "boolean" ? raw.detectMedia : DEFAULT_SETTINGS.detectMedia,
    maxMediaCandidates: Number.isInteger(maxMediaCandidates) && maxMediaCandidates >= 10 && maxMediaCandidates <= 500
      ? maxMediaCandidates : DEFAULT_SETTINGS.maxMediaCandidates,
    mediaRetentionMinutes: Number.isInteger(mediaRetentionMinutes) && mediaRetentionMinutes >= 1 && mediaRetentionMinutes <= 1440
      ? mediaRetentionMinutes : DEFAULT_SETTINGS.mediaRetentionMinutes,
  };
}

export async function loadSettings(): Promise<Settings> {
  const raw = await browser.storage.sync.get(Object.keys(DEFAULT_SETTINGS));
  return normalizeSettings(raw);
}

export async function saveSettings(patch: Partial<Settings>): Promise<void> {
  await browser.storage.sync.set(patch);
}

/** 最近一次交接的结果，popup 里展示 */
export interface LastResult {
  ok: boolean;
  url: string;
  fileName?: string;
  size?: number | null;
  error?: string;
  /** 交接回退给了浏览器（浏览器继续下载）还是彻底没下（右键菜单） */
  fellBack: boolean;
  at: number;
}

export interface RuntimeState {
  connected: boolean;
  lastResult?: LastResult;
  /** 有失败用户还没在 popup 里看过：徽标显示 ! */
  unseenFailure: boolean;
}

const DEFAULT_STATE: RuntimeState = { connected: false, unseenFailure: false };

export async function loadState(): Promise<RuntimeState> {
  const raw = await browser.storage.session.get(["connected", "lastResult", "unseenFailure"]);
  return { ...DEFAULT_STATE, ...(raw as Partial<RuntimeState>) };
}

export async function saveState(patch: Partial<RuntimeState>): Promise<void> {
  await browser.storage.session.set(patch);
}
