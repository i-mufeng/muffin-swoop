// Service worker：接管浏览器下载、右键菜单、心跳与徽标。
//
// 交接协议（docs/browser-extension.md §3）：先暂停浏览器下载，Swoop 确认接手（200）才取消；
// 其它任何情况都恢复浏览器下载，用户无感。

import { ping, sendTask, type HandoffResult, type TaskBody } from "./api";
import { badgeFor } from "./badge";
import { cookieHeader } from "./cookies";
import { knownSize, shouldIntercept } from "./rules";
import { loadSettings, loadState, saveState, type LastResult } from "./settings";

const HEARTBEAT_ALARM = "swoop-heartbeat";
const MENU_ID = "swoop-download";

// ---- 心跳与徽标 ----

async function refreshBadge(): Promise<void> {
  const [settings, state] = await Promise.all([loadSettings(), loadState()]);
  const badge = badgeFor(state, settings.enabled);
  await Promise.all([
    chrome.action.setBadgeText({ text: badge.text }),
    chrome.action.setBadgeBackgroundColor({ color: badge.color }),
    chrome.action.setTitle({ title: badge.title }),
  ]);
}

async function heartbeat(): Promise<boolean> {
  const settings = await loadSettings();
  const connected = settings.enabled && (await ping(settings.port));
  await saveState({ connected });
  await refreshBadge();
  return connected;
}

function setup(): void {
  // MV3 的 service worker 空闲会被回收，setInterval 不可靠；alarms 最短 30 秒
  void chrome.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: 0.5 });
  void heartbeat();
}

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: MENU_ID,
      title: "用 Swoop 下载",
      contexts: ["link", "image", "video", "audio"],
    });
  });
  setup();
});
chrome.runtime.onStartup.addListener(setup);
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === HEARTBEAT_ALARM) void heartbeat();
});
// 设置页改了端口/开关：立刻重新探测
chrome.storage.onChanged.addListener((_changes, area) => {
  if (area === "sync") void heartbeat();
});
// popup 打开时请求刷新
chrome.runtime.onMessage.addListener((msg: unknown, _sender, sendResponse) => {
  if ((msg as { type?: string } | null)?.type === "refresh") {
    void heartbeat().then(sendResponse);
    return true;
  }
  return false;
});

// ---- 交接 ----

async function buildTask(url: string, referrer: string | undefined, withCookies: boolean): Promise<TaskBody> {
  const headers: [string, string][] = [["User-Agent", navigator.userAgent]];
  const cookie = withCookies ? await cookieHeader(url) : undefined;
  if (cookie) headers.push(["Cookie", cookie]);
  return { url, referrer: referrer || undefined, headers };
}

async function record(url: string, result: HandoffResult, fellBack: boolean): Promise<void> {
  const lastResult: LastResult = result.ok
    ? { ok: true, url, fileName: result.fileName, size: result.size, fellBack: false, at: Date.now() }
    : { ok: false, url, error: result.error, fellBack, at: Date.now() };
  await saveState({ lastResult, unseenFailure: !result.ok, connected: result.ok || (await loadState()).connected });
  await refreshBadge();
}

async function onDownloadCreated(item: chrome.downloads.DownloadItem): Promise<void> {
  const settings = await loadSettings();
  const decision = shouldIntercept(item, settings);
  if (!decision.take) return;

  // Swoop 没开就完全不碰这次下载
  if (!(await ping(settings.port, 800))) {
    await saveState({ connected: false });
    await refreshBadge();
    return;
  }

  try {
    await chrome.downloads.pause(item.id);
  } catch {
    // 小文件可能已经下完，交给浏览器即可
    return;
  }

  // 用原始 URL 而不是 finalUrl：重定向后的签名链接往往很快过期，续传时还得从头走一遍
  const body = await buildTask(item.url, item.referrer, true);
  body.mime = item.mime || undefined;
  body.size = knownSize(item);
  const result = await sendTask(settings.port, body);

  if (result.ok) {
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
  } else {
    await chrome.downloads.resume(item.id).catch(() => {});
  }
  await record(item.url, result, true);
}

chrome.downloads.onCreated.addListener((item) => {
  void onDownloadCreated(item);
});

async function onMenuClicked(info: chrome.contextMenus.OnClickData, tab?: chrome.tabs.Tab): Promise<void> {
  if (info.menuItemId !== MENU_ID) return;
  const url = info.linkUrl ?? info.srcUrl;
  if (!url || !/^https?:/i.test(url)) {
    await record(url ?? "", { ok: false, error: "只支持 http/https 链接" }, false);
    return;
  }
  const settings = await loadSettings();
  if (!(await ping(settings.port))) {
    await record(url, { ok: false, error: "Swoop 未运行，请先启动 Swoop" }, false);
    return;
  }
  // 用户主动选的，不受开关和过滤规则限制；隐身窗口不带 Cookie，避免写进 Swoop 的数据库
  const body = await buildTask(url, info.frameUrl ?? info.pageUrl, !tab?.incognito);
  await record(url, await sendTask(settings.port, body), false);
}

chrome.contextMenus.onClicked.addListener((info, tab) => {
  void onMenuClicked(info, tab);
});
