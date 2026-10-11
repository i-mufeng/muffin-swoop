import { browser } from "./browser";
import { detectMedia, mergeCandidates, type MediaCandidate, type MediaResponse } from "./media";
import { saveAndOpenBatch, type PageReply } from "./page-tasks";
import { manualDownloadBlock } from "./rules";
import { loadSettings } from "./settings";

const PREFIX = "media-tab:";
interface MediaTab { pageUrl: string; candidates: MediaCandidate[]; updatedAt: number }

/** 串行存储变更，避免分片并发响应覆盖其它候选或在关标签后重新写回。 */
export class MediaCatalog {
  private pending: Promise<unknown> = Promise.resolve();
  private error?: string;

  private serial<T>(work: () => Promise<T>): Promise<T> {
    const result = this.pending.catch(() => {}).then(work);
    this.pending = result;
    return result;
  }

  capture(response: MediaResponse): Promise<void> {
    const candidate = detectMedia(response, Date.now());
    if (!candidate) return Promise.resolve();
    return this.serial(async () => {
      const settings = await loadSettings();
      if (!settings.detectMedia || manualDownloadBlock(candidate.url, settings)) return;
      let tab: chrome.tabs.Tab;
      try { tab = await browser.tabs.get(response.tabId); } catch { return; }
      if (tab.incognito || !tab.url || !/^https?:/i.test(tab.url)) return;
      const key = PREFIX + response.tabId;
      const raw = await browser.storage.session.get(key);
      const stored = raw[key] as MediaTab | undefined;
      const candidates = mergeCandidates(stored?.pageUrl === tab.url ? stored.candidates : [], candidate,
        settings.maxMediaCandidates, settings.mediaRetentionMinutes * 60_000, Date.now());
      await browser.storage.session.set({ [key]: { pageUrl: tab.url, candidates, updatedAt: Date.now() } satisfies MediaTab });
      await this.pruneStored();
      this.error = undefined;
    }).catch(() => { this.error = "媒体检测暂不可用，请检查扩展权限后重试"; });
  }

  clearTab(tabId: number): Promise<void> {
    return this.serial(() => browser.storage.session.remove(PREFIX + tabId));
  }

  cleanup(): Promise<void> { return this.serial(() => this.pruneStored()); }

  private async pruneStored(): Promise<void> {
    const settings = await loadSettings();
    const all = await browser.storage.session.get(null);
    const records = Object.entries(all).filter(([key]) => key.startsWith(PREFIX)) as [string, MediaTab][];
    records.sort(([, a], [, b]) => b.updatedAt - a.updatedAt);
    const remove: string[] = [];
    let count = 0;
    let bytes = 0;
    for (const [index, [key, tab]] of records.entries()) {
      const live = mergeCandidates(tab.candidates, undefined, settings.maxMediaCandidates, settings.mediaRetentionMinutes * 60_000, Date.now())
        .filter((candidate) => !manualDownloadBlock(candidate.url, settings));
      count += live.length;
      bytes += new TextEncoder().encode(JSON.stringify(live)).length;
      if (!settings.enabled || !settings.detectMedia || !live.length || count > 2000 || bytes > 2 * 1024 * 1024 || index >= 24) {
        remove.push(key);
      } else if (live.length !== tab.candidates.length) {
        await browser.storage.session.set({ [key]: { ...tab, candidates: live } });
      }
    }
    if (remove.length) await browser.storage.session.remove(remove);
  }

  async read(tabId: number): Promise<{ tab: chrome.tabs.Tab; candidates: MediaCandidate[] }> {
    await this.cleanup();
    if (this.error) throw new Error(this.error);
    const tab = await browser.tabs.get(tabId);
    const raw = await browser.storage.session.get(PREFIX + tabId);
    const stored = raw[PREFIX + tabId] as MediaTab | undefined;
    return { tab, candidates: stored && stored.pageUrl === tab.url ? stored.candidates : [] };
  }

  async open(tabId: number): Promise<PageReply> {
    try {
      const settings = await loadSettings();
      if (!settings.enabled || !settings.detectMedia) return { ok: false, error: "媒体检测已关闭，请在扩展设置中开启" };
      const { tab, candidates } = await this.read(tabId);
      if (!candidates.length) return { ok: false, error: "尚未发现媒体，请先在网页播放音视频后重试" };
      return saveAndOpenBatch({
        id: crypto.randomUUID(), tabId, pageUrl: tab.url!, title: tab.title ?? "", incognito: false,
        links: candidates, truncated: candidates.length >= settings.maxMediaCandidates, createdAt: Date.now(), kind: "media",
      }, tab.windowId);
    } catch (error) { return { ok: false, error: error instanceof Error ? error.message : "无法读取媒体列表" }; }
  }
}
