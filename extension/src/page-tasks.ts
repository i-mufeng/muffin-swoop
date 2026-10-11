import { browser } from "./browser";
import { collectDocumentLinks } from "./collect";
import { normalizePageLinks, type PageLink } from "./rules";
import { loadSettings } from "./settings";
import type { HandoffResult } from "./api";

export interface PageBatch {
  id: string;
  tabId: number;
  pageUrl: string;
  title: string;
  incognito: boolean;
  links: PageLink[];
  truncated: boolean;
  createdAt: number;
  results?: Record<string, HandoffResult>;
  kind?: "media";
}
export type PageReply = { ok: true; batch: PageBatch } | { ok: false; error: string };
const PREFIX = "page-batch:";
const RESULT_PREFIX = "page-result:";
const MAX_AGE = 24 * 60 * 60 * 1000;

export async function loadPageBatch(id: string): Promise<PageBatch | undefined> {
  const raw = await browser.storage.session.get(PREFIX + id);
  const batch = raw[PREFIX + id] as PageBatch | undefined;
  if (!batch || Date.now() - batch.createdAt >= MAX_AGE) return undefined;
  const results = await browser.storage.session.get(batch.links.map((_, index) => `${RESULT_PREFIX}${id}:${index}`));
  return { ...batch, results: Object.fromEntries(batch.links.flatMap((link, index) => {
    const result = results[`${RESULT_PREFIX}${id}:${index}`] as HandoffResult | undefined;
    return result ? [[link.url, result]] : [];
  })) };
}

export async function savePageResult(batch: PageBatch, url: string, result: HandoffResult): Promise<void> {
  const index = batch.links.findIndex((link) => link.url === url);
  if (index >= 0) await browser.storage.session.set({ [`${RESULT_PREFIX}${batch.id}:${index}`]: result });
}

export async function openPageLinks(tabId: number): Promise<PageReply> {
  try {
    const [tab, settings] = await Promise.all([browser.tabs.get(tabId), loadSettings()]);
    if (!settings.enabled) return { ok: false, error: "接管已关闭，请先在扩展弹窗中开启" };
    if (!tab.url || !/^https?:/i.test(tab.url)) return { ok: false, error: "请在普通网页上使用此功能" };
    const captures = await browser.scripting.executeScript({
      target: { tabId }, func: collectDocumentLinks, args: [settings.maxPageLinks * 10],
    });
    const capture = captures[0]?.result;
    if (!capture) return { ok: false, error: "未能读取网页链接，请刷新网页后重试" };
    const normalized = normalizePageLinks(capture.links, capture.pageUrl, settings.maxPageLinks);
    const batch: PageBatch = {
      id: crypto.randomUUID(), tabId, pageUrl: capture.pageUrl, title: capture.title,
      incognito: tab.incognito ?? false, links: normalized.links,
      truncated: capture.truncated || normalized.truncated, createdAt: Date.now(),
    };
    return await saveAndOpenBatch(batch, tab.windowId);
  } catch {
    return { ok: false, error: "此页面不允许读取链接，请检查扩展的网站访问权限" };
  }
}

export async function saveAndOpenBatch(batch: PageBatch, windowId: number): Promise<PageReply> {
  try {
    // 只保存链接上下文，不保存 Cookie。删除失效记录避免 session 配额无界增长。
    const previous = await browser.storage.session.get(null);
    const oldKeys = Object.keys(previous).filter((key) => key.startsWith(PREFIX))
      .sort((a, b) => (previous[b] as PageBatch).createdAt - (previous[a] as PageBatch).createdAt);
    const remove = oldKeys.filter((key, index) => index >= 3 || Date.now() - (previous[key] as PageBatch).createdAt >= MAX_AGE);
    const expiredIds = remove.map((key) => key.slice(PREFIX.length));
    const resultKeys = Object.keys(previous).filter((key) => expiredIds.some((id) => key.startsWith(`${RESULT_PREFIX}${id}:`)));
    if (remove.length || resultKeys.length) await browser.storage.session.remove([...remove, ...resultKeys]);
    await browser.storage.session.set({ [PREFIX + batch.id]: batch });
    await browser.tabs.create({ url: browser.runtime.getURL(`links.html?batch=${encodeURIComponent(batch.id)}`), windowId });
    return { ok: true, batch };
  } catch {
    return { ok: false, error: "无法保存或打开链接列表，请关闭旧列表后重试" };
  }
}
