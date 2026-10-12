import { browser, isFirefox } from "./browser";
// Service worker：接管浏览器下载、右键菜单、心跳与徽标。
//
// 交接协议（docs/browser-extension.md §3）：先拿住浏览器下载（Chromium 不确定文件名，Firefox 暂停），
// Swoop 确认接手（200）才取消；其它任何情况都放行浏览器下载，用户无感。

import { ping, sendTask, supportsMedia, type HandoffResult, type TaskBody } from "./api";
import { badgeFor } from "./badge";
import { cookieHeader } from "./cookies";
import { knownSize, shouldIntercept, shouldOpenWelcome, manualDownloadBlock } from "./rules";
import { loadPageBatch, openPageLinks, savePageResult } from "./page-tasks";
import { contextHeaders, safeReferrer, redirectTarget } from "./request-context";
import { loadSettings, loadState, saveState, type LastResult } from "./settings";
import { MediaCatalog } from "./media-catalog";
import { translate } from "./i18n";

const HEARTBEAT_ALARM = "swoop-heartbeat";
const MENU_ID = "swoop-download";
const PAGE_MENU_ID = "swoop-page-links";
const mediaCatalog = new MediaCatalog();

browser.webRequest.onHeadersReceived.addListener((response) => {
  void mediaCatalog.capture(response);
}, { urls: ["http://*/*", "https://*/*"] }, ["responseHeaders"]);
browser.tabs.onRemoved.addListener((tabId) => { void mediaCatalog.clearTab(tabId); });
browser.tabs.onUpdated.addListener((tabId, change) => { if (change.url) void mediaCatalog.clearTab(tabId); });

// ---- 心跳与徽标 ----

async function refreshBadge(): Promise<void> {
  const [settings, state] = await Promise.all([loadSettings(), loadState()]);
  const badge = badgeFor(state, settings.enabled, settings.language);
  await Promise.all([
    browser.action.setBadgeText({ text: badge.text }),
    browser.action.setBadgeBackgroundColor({ color: badge.color }),
    browser.action.setTitle({ title: badge.title }),
  ]);
}

async function heartbeat(): Promise<boolean> {
  const settings = await loadSettings();
  const connected = settings.enabled && (await ping(settings.port));
  await saveState({ connected });
  await refreshBadge();
  await mediaCatalog.cleanup().catch(() => {});
  return connected;
}

// 连续设置事件必须串行更新，避免 removeAll 与另一轮 create 交错。
let menuRefresh: Promise<void> = Promise.resolve();
function refreshMenus(): Promise<void> {
  menuRefresh = menuRefresh.catch(() => {}).then(async () => {
    const { language } = await loadSettings();
    await browser.contextMenus.removeAll();
    await browser.contextMenus.create({ id: MENU_ID, title: translate("用 Swoop 下载", language), contexts: ["link", "image", "video", "audio"] });
    await browser.contextMenus.create({ id: PAGE_MENU_ID, title: translate("用 Swoop 下载本页链接…", language), contexts: ["page", "link", "image", "video", "audio"] });
  });
  return menuRefresh;
}
function setup(): void {
  // MV3 的 service worker 空闲会被回收，setInterval 不可靠；alarms 最短 30 秒
  void browser.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: isFirefox ? 1 : 0.5 });
  void heartbeat();
  void refreshMenus().catch((error: unknown) => console.warn("Swoop context menu refresh failed", error));
}

browser.runtime.onInstalled.addListener((details) => {
  setup();
  if (details && shouldOpenWelcome(details.reason, details.previousVersion, browser.runtime.getManifest().version)) {
    void browser.tabs.create({ url: browser.runtime.getURL("welcome.html") });
  }
});
browser.runtime.onStartup.addListener(setup);
browser.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === HEARTBEAT_ALARM) void heartbeat();
});
// 设置页改了端口/开关：立刻重新探测
browser.storage.onChanged.addListener((changes, area) => {
  if (area === "sync") void heartbeat();
  if (area === "sync" && changes.language) {
    void refreshMenus().catch((error: unknown) => console.warn("Swoop context menu refresh failed", error));
  }
});
// 仅扩展自己的页面可请求读取 / 提交，内容页面不能伪造来源页。
browser.runtime.onMessage.addListener((msg: unknown, sender, sendResponse) => {
  if (!sender.url?.startsWith(browser.runtime.getURL(""))) return false;
  const request = msg as { type?: string; tabId?: number; batchId?: string; url?: string } | null;
  if (request?.type === "refresh") {
    void heartbeat().then(sendResponse);
    return true;
  }
  if (request?.type === "collect-page" && Number.isInteger(request.tabId)) {
    void openPageLinks(request.tabId!).then(sendResponse);
    return true;
  }
  if (request?.type === "open-media" && Number.isInteger(request.tabId)) {
    void mediaCatalog.open(request.tabId!).then(sendResponse);
    return true;
  }
  if (request?.type === "media-count" && Number.isInteger(request.tabId)) {
    void mediaCatalog.read(request.tabId!).then(({ candidates }) => sendResponse({ ok: true, count: candidates.length }))
      .catch((error) => sendResponse({ ok: false, error: error instanceof Error ? error.message : "无法读取媒体" }));
    return true;
  }
  if (request?.type === "submit-page-link" && typeof request.batchId === "string" && typeof request.url === "string") {
    void submitPageLink(request.batchId, request.url)
      .then(sendResponse).catch(() => sendResponse({ ok: false, error: "提交失败，请先检查 Swoop 任务列表再重试" }));
    return true;
  }
  return false;
});

// ---- 交接 ----

async function buildTask(url: string, referrer: string | undefined, withCookies: boolean, tabId?: number, referrerPolicy?: string): Promise<TaskBody> {
  // FTP 使用独立协议认证；不能把网页会话、Referer 或 UA 交给 FTP 服务。
  const parsed = new URL(url);
  if (/^ftps?:$/.test(parsed.protocol)) return { url: parsed.href, headers: [] };
  const cookie = withCookies ? await cookieHeader(url, tabId) : undefined;
  return { url, referrer: safeReferrer(url, referrer, referrerPolicy), headers: contextHeaders(navigator.userAgent, cookie) };
}

async function record(url: string, result: HandoffResult, fellBack: boolean): Promise<void> {
  // 被拒绝的手动 FTP 凭据链接也不能留在扩展会话记录中。
  if (/^ftps?:/i.test(url.trimStart())) {
    try { const safe = new URL(url); safe.username = ""; safe.password = ""; url = safe.href; } catch { url = ""; }
  }
  const lastResult: LastResult = result.ok
    ? { ok: true, url, fileName: result.fileName, size: result.size, fellBack: false, at: Date.now() }
    : { ok: false, url, error: result.error, fellBack, at: Date.now() };
  await saveState({ lastResult, unseenFailure: !result.ok, connected: result.ok || (await loadState()).connected });
  await refreshBadge();
}

/** 过滤并确认 Swoop 在线；返回 undefined 表示这次下载不碰。 */
async function readyToTake(item: chrome.downloads.DownloadItem): Promise<number | undefined> {
  const settings = await loadSettings();
  if (!shouldIntercept(item, settings).take) return undefined;
  // Swoop 没开就完全不碰这次下载
  if (!(await ping(settings.port, 800))) {
    await saveState({ connected: false });
    await refreshBadge();
    return undefined;
  }
  return settings.port;
}

/** 提交给 Swoop；接手成功就取消并抹掉浏览器下载，返回 true。 */
async function handOff(item: chrome.downloads.DownloadItem, port: number): Promise<boolean> {
  // 用原始 URL 而不是 finalUrl：重定向后的签名链接往往很快过期，续传时还得从头走一遍
  const body = await buildTask(item.url, item.referrer, true);
  // 已登录站点常跳到另一个域名再发文件，那个域名的登录 Cookie 也要交给 Swoop
  const target = redirectTarget(item.url, item.finalUrl);
  const targetCookie = target ? await cookieHeader(target) : undefined;
  if (target && targetCookie) body.redirect_cookies = [[new URL(target).origin, targetCookie]];
  body.mime = item.mime || undefined;
  body.size = knownSize(item);
  const result = await sendTask(port, body);

  if (result.ok) {
    await browser.downloads.cancel(item.id).catch(() => {});
    await browser.downloads.erase({ id: item.id }).catch(() => {});
  }
  await record(item.url, result, true).catch(() => {});
  return result.ok;
}

// Chromium：浏览器要等文件名确定后才把下载交给下载面板 / 另存为对话框。
// 在这里拿住回调交给 Swoop，接手成功就直接取消、不 suggest，Edge / Chrome 的下载面板不会弹出；
// 失败再 suggest 放行，浏览器照常下载。此阶段下载不会完成，所以不需要 pause。
async function onDeterminingFilename(item: chrome.downloads.DownloadItem): Promise<boolean> {
  const port = await readyToTake(item);
  return port !== undefined && handOff(item, port);
}

// Firefox 没有 onDeterminingFilename：下载已开始，先暂停，确认接手再取消，否则恢复。
async function onDownloadCreated(item: chrome.downloads.DownloadItem): Promise<void> {
  const port = await readyToTake(item);
  if (port === undefined) return;
  try {
    await browser.downloads.pause(item.id);
  } catch {
    // 小文件可能已经下完，交给浏览器即可
    return;
  }
  if (!(await handOff(item, port))) await browser.downloads.resume(item.id).catch(() => {});
}

if (browser.downloads.onDeterminingFilename) {
  browser.downloads.onDeterminingFilename.addListener((item, suggest) => {
    void onDeterminingFilename(item).catch(() => false).then((taken) => { if (!taken) suggest(); });
    return true;
  });
} else {
  browser.downloads.onCreated.addListener((item) => {
    void onDownloadCreated(item);
  });
}

async function submitManual(url: string, referrer: string | undefined, incognito: boolean, tabId?: number, referrerPolicy?: string,
  hint?: Pick<TaskBody, "mime" | "size" | "media">): Promise<HandoffResult> {
  const settings = await loadSettings();
  const blocked = manualDownloadBlock(url, settings);
  if (blocked) return { ok: false, error: blocked };
  if (/^ftps?:$/.test(new URL(url).protocol) && !/^https?:\/\//i.test(referrer ?? "")) {
    return { ok: false, error: "请在普通网页上使用此功能" };
  }
  if (!(await ping(settings.port))) return { ok: false, error: "Swoop 未运行，请先启动 Swoop" };
  // 隐身下载沿用原有边界：主动提交 URL，但不向桌面应用传递隐身 Cookie。
  const body = await buildTask(url, referrer, !incognito, tabId, referrerPolicy);
  Object.assign(body, hint);
  // Cookie / 探测期间用户也可能关闭接管，发送前再检查一次。
  const current = await loadSettings();
  const changed = manualDownloadBlock(url, current);
  if (changed) return { ok: false, error: changed };
  if (hint?.media && !(await supportsMedia(current.port))) return { ok: false, error: "当前 Swoop 尚不支持媒体接收，请更新后重试" };
  return sendTask(current.port, body);
}

const pageSubmissions = new Map<string, Promise<HandoffResult>>();
function submitPageLink(batchId: string, url: string): Promise<HandoffResult> {
  const key = JSON.stringify([batchId, url]);
  const existing = pageSubmissions.get(key);
  if (existing) return existing;
  const pending = doSubmitPageLink(batchId, url).finally(() => pageSubmissions.delete(key));
  pageSubmissions.set(key, pending);
  return pending;
}

async function doSubmitPageLink(batchId: string, url: string): Promise<HandoffResult> {
  const batch = await loadPageBatch(batchId);
  if (!batch) return { ok: false, error: "此链接列表已过期，请返回原网页重新提取" };
  const link = batch.links.find((link) => link.url === url);
  if (!link) return { ok: false, error: "链接不在本页列表中" };
  const previous = batch.results?.[url];
  if (previous?.ok) return previous;
  if (batch.kind === "media" && !(await loadSettings()).detectMedia) return { ok: false, error: "媒体检测已关闭" };
  try {
    const tab = await browser.tabs.get(batch.tabId);
    // 不允许原标签跳到另一个页面后继续使用旧会话上下文。
    if (tab.url !== batch.pageUrl || tab.incognito !== batch.incognito) {
      return { ok: false, error: "原网页已变化，请重新提取链接" };
    }
  } catch { return { ok: false, error: "原网页已关闭，请重新打开后提取链接" }; }
  const result = await submitManual(url, batch.pageUrl, batch.incognito, batch.tabId, link.referrerPolicy,
    { mime: link.mime, size: link.size, media: link.media });
  await savePageResult(batch, url, result);
  await record(url, result, false);
  return result;
}

async function onMenuClicked(info: chrome.contextMenus.OnClickData, tab?: chrome.tabs.Tab): Promise<void> {
  if (info.menuItemId === PAGE_MENU_ID) {
    if (tab?.id !== undefined) {
      const opened = await openPageLinks(tab.id);
      if (!opened.ok) await record(info.pageUrl ?? "", opened, false);
    }
    return;
  }
  if (info.menuItemId !== MENU_ID) return;
  const url = info.linkUrl ?? info.srcUrl ?? "";
  const result = await submitManual(url, info.frameUrl ?? info.pageUrl, tab?.incognito ?? false, tab?.id);
  await record(url, result, false);
}

browser.contextMenus.onClicked.addListener((info, tab) => {
  void onMenuClicked(info, tab);
});
