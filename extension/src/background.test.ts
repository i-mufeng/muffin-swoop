import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HandoffResult } from "./api";
import { DEFAULT_SETTINGS } from "./settings";

let message: (msg: unknown, sender: { url: string }, reply: (value: unknown) => void) => boolean;
let installed: (details?: { reason: string; previousVersion?: string }) => void;
let startup: () => void;
let storageChanged: (changes: Record<string, { newValue: unknown }>, area: string) => void;
let capture: (response: unknown) => void;
let closeTab: (tabId: number) => void;
let menuClicked: (info: Partial<chrome.contextMenus.OnClickData>, sourceTab?: typeof tab) => void;
let session: Record<string, unknown>;
let settings: Record<string, unknown>;
const pageUrl = "https://example.com/page?token=secret#top";
const taskUrl = "https://cdn.example.com/file.zip";
const extUrl = "chrome-extension://swoop/";
const tab = { id: 1, url: pageUrl, windowId: 2, incognito: false };
const chromeMock = {
  runtime: {
    getURL: (path: string) => extUrl + path,
    getManifest: () => ({ version: "0.4.0" }),
    onMessage: { addListener: vi.fn((handler) => { message = handler; }) },
    onInstalled: { addListener: vi.fn((handler) => { installed = handler; }) },
    onStartup: { addListener: vi.fn((handler) => { startup = handler; }) },
  },
  action: { setBadgeText: vi.fn(), setBadgeBackgroundColor: vi.fn(), setTitle: vi.fn() },
  storage: {
    sync: { get: vi.fn(async () => settings) },
    session: {
      get: vi.fn(async (keys: string[] | string | null) => keys === null ? { ...session }
        : Object.fromEntries((typeof keys === "string" ? [keys] : keys).map((key) => [key, session[key]]))),
      set: vi.fn(async (values: Record<string, unknown>) => { Object.assign(session, structuredClone(values)); }),
      remove: vi.fn(async (keys: string[] | string) => { (typeof keys === "string" ? [keys] : keys).forEach((key) => delete session[key]); }),
    },
    onChanged: { addListener: vi.fn((handler) => { storageChanged = handler; }) },
  },
  alarms: { create: vi.fn(), onAlarm: { addListener: vi.fn() } },
  tabs: { get: vi.fn(async () => tab), create: vi.fn(async () => ({})),
    onRemoved: { addListener: vi.fn((handler) => { closeTab = handler; }) }, onUpdated: { addListener: vi.fn() },
  },
  webRequest: { onHeadersReceived: { addListener: vi.fn((handler) => { capture = handler; }) } },
  scripting: { executeScript: vi.fn(async () => [{ result: {
    pageUrl, title: "文件列表", links: [{ url: taskUrl, title: "下载" }], truncated: false,
  } }]) },
  cookies: {
    getAllCookieStores: vi.fn(async () => [{ id: "container-2", tabIds: [1] }]),
    getAll: vi.fn(async () => [{ name: "session", value: "test", path: "/" }]),
  },
  contextMenus: { removeAll: vi.fn(async () => {}), create: vi.fn(), onClicked: { addListener: vi.fn((handler) => { menuClicked = handler; }) } },
  downloads: { onCreated: { addListener: vi.fn() } },
};

function request<T>(body: unknown, url = extUrl + "links.html"): Promise<T> {
  return new Promise((resolve) => { if (!message(body, { url }, (result) => resolve(result as T))) resolve(undefined as T); });
}

beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  session = {};
  settings = { ...DEFAULT_SETTINGS };
  vi.stubGlobal("chrome", chromeMock);
  vi.stubGlobal("browser", undefined);
  vi.stubGlobal("navigator", { userAgent: "Browser/1" });
  vi.stubGlobal("fetch", vi.fn(async (url: string) => url.endsWith("/ping")
    ? Response.json({ app: "swoop", media_handoff: true }) : Response.json({ id: 1, file_name: "file.zip", size: 42 })));
  await import("./background");
});

describe("后台入口与桥接", () => {
  it("首次安装与次版本更新打开欢迎页，补丁更新不打开", () => {
    installed({ reason: "install" });
    expect(chromeMock.tabs.create).toHaveBeenCalledWith({ url: extUrl + "welcome.html" });
    chromeMock.tabs.create.mockClear();
    installed({ reason: "update", previousVersion: "0.4.0" });
    expect(chromeMock.tabs.create).not.toHaveBeenCalled();
    installed({ reason: "update", previousVersion: "0.3.2" });
    expect(chromeMock.tabs.create).toHaveBeenCalledTimes(1);
  });
  it("安装与连续语言变更串行重建菜单，最后保留最新语言", async () => {
    let finish!: () => void;
    chromeMock.contextMenus.removeAll.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    installed();
    await vi.waitFor(() => expect(chromeMock.contextMenus.removeAll).toHaveBeenCalledTimes(1));
    settings.language = "zh-CN";
    storageChanged({ language: { newValue: "zh-CN" } }, "sync");
    settings.language = "en-US";
    storageChanged({ language: { newValue: "en-US" } }, "sync");
    expect(chromeMock.contextMenus.create).not.toHaveBeenCalled();
    expect(chromeMock.contextMenus.removeAll).toHaveBeenCalledTimes(1);
    finish();
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(6));
    expect(chromeMock.contextMenus.removeAll).toHaveBeenCalledTimes(3);
    expect(chromeMock.contextMenus.create.mock.calls.at(-1)![0].title).toBe("Download page links with Swoop…");
  });
  it("安装、语言变更与重启按保存语言刷新菜单和工具栏，不改菜单 ID", async () => {
    installed();
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(2));
    expect(chromeMock.contextMenus.create.mock.calls[0]![0]).toMatchObject({ id: "swoop-download", title: "用 Swoop 下载" });
    settings.language = "en-US";
    storageChanged({ language: { newValue: "en-US" } }, "sync");
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(4));
    expect(chromeMock.contextMenus.create.mock.calls.slice(-2).map(([menu]) => [menu.id, menu.title])).toEqual([
      ["swoop-download", "Download with Swoop"], ["swoop-page-links", "Download page links with Swoop…"],
    ]);
    await vi.waitFor(() => expect(chromeMock.action.setTitle).toHaveBeenLastCalledWith({ title: "Swoop: Connected" }));
    vi.resetModules();
    await import("./background");
    startup();
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(6));
    expect(chromeMock.contextMenus.create.mock.calls.at(-1)![0].title).toBe("Download page links with Swoop…");
    expect(chromeMock.contextMenus.removeAll).toHaveBeenCalledTimes(3);
    settings.language = "zh-CN";
    storageChanged({ language: { newValue: "zh-CN" } }, "sync");
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(8));
    expect(chromeMock.contextMenus.create.mock.calls.at(-1)![0].title).toBe("用 Swoop 下载本页链接…");
  });
  it("安装注册两个可达菜单，弹窗提取入口打开扩展选择页", async () => {
    installed();
    await vi.waitFor(() => expect(chromeMock.contextMenus.create).toHaveBeenCalledTimes(2));
    const reply = await request<{ ok: true; batch: { id: string } }>({ type: "collect-page", tabId: 1 });
    expect(reply.ok).toBe(true);
    expect(chromeMock.tabs.create).toHaveBeenCalledWith({ url: `${extUrl}links.html?batch=${reply.batch.id}`, windowId: 2 });
    expect(chromeMock.scripting.executeScript).toHaveBeenCalledWith(expect.objectContaining({ target: { tabId: 1 } }));
  });
  it("仅扩展页面可提取；停用时不执行网页脚本", async () => {
    expect(await request({ type: "collect-page", tabId: 1 }, pageUrl)).toBeUndefined();
    settings.enabled = false;
    expect(await request({ type: "collect-page", tabId: 1 })).toMatchObject({ ok: false });
    expect(chromeMock.scripting.executeScript).not.toHaveBeenCalled();
  });
  it("发送实际协议、正确 Cookie 容器与安全 Referer，重复请求及刷新后成功项不再提交", async () => {
    const reply = await request<{ batch: { id: string } }>({ type: "collect-page", tabId: 1 });
    const payload = { type: "submit-page-link", batchId: reply.batch.id, url: taskUrl };
    const results = await Promise.all([request<HandoffResult>(payload), request<HandoffResult>(payload)]);
    expect(results).toEqual([{ ok: true, fileName: "file.zip", size: 42 }, { ok: true, fileName: "file.zip", size: 42 }]);
    expect(chromeMock.cookies.getAll).toHaveBeenCalledWith({ url: taskUrl, storeId: "container-2" });
    const posts = () => vi.mocked(fetch).mock.calls.filter(([, init]) => init?.method === "POST");
    expect(posts()).toHaveLength(1);
    expect(JSON.parse(posts()[0]![1]!.body as string)).toEqual({
      url: taskUrl, referrer: "https://example.com/", headers: [["User-Agent", "Browser/1"], ["Cookie", "session=test"]],
    });
    await request(payload);
    const { loadPageBatch } = await import("./page-tasks");
    const { LinkBatch } = await import("./batch");
    const stored = (await loadPageBatch(reply.batch.id))!;
    const restored = new LinkBatch(stored.links, stored.results);
    restored.select([taskUrl], true);
    expect(restored.pending).toEqual([]);
    expect(posts()).toHaveLength(1);
  });
  it("逐项失败可重试，列表外链接不会提交", async () => {
    const reply = await request<{ batch: { id: string } }>({ type: "collect-page", tabId: 1 });
    const payload = { type: "submit-page-link", batchId: reply.batch.id, url: taskUrl };
    vi.mocked(fetch).mockImplementation(async (url) => String(url).endsWith("/ping")
      ? Response.json({ app: "swoop" }) : Response.json({ error: "登录过期" }, { status: 422 }));
    expect(await request(payload)).toEqual({ ok: false, error: "登录过期" });
    expect(await request({ ...payload, url: "https://other.example/file" })).toMatchObject({ ok: false });
    vi.mocked(fetch).mockImplementation(async (url) => String(url).endsWith("/ping")
      ? Response.json({ app: "swoop" }) : Response.json({ id: null, file_name: "file.zip", size: null }));
    expect(await request(payload)).toEqual({ ok: true, fileName: "file.zip", size: null });
    expect(vi.mocked(fetch).mock.calls.filter(([, init]) => init?.method === "POST")).toHaveLength(2);
  });
  it("HTTP(S) 网页中的 FTP(S) 链接按空头协议提交，不读取浏览器 Cookie", async () => {
    const urls = ["ftp://files.example.com/archive.zip", "ftps://files.example.com/archive.zip"];
    chromeMock.scripting.executeScript.mockResolvedValueOnce([{ result: {
      pageUrl, title: "FTP 下载", links: urls.map((url) => ({ url, title: "下载" })), truncated: false,
    } }]);
    const reply = await request<{ batch: { id: string; links: { url: string }[] } }>({ type: "collect-page", tabId: 1 });
    expect(reply.batch.links.map((link) => link.url)).toEqual(urls);
    for (const url of urls) {
      expect(await request({ type: "submit-page-link", batchId: reply.batch.id, url })).toMatchObject({ ok: true });
    }
    const posts = vi.mocked(fetch).mock.calls.filter(([, init]) => init?.method === "POST");
    expect(posts.map(([, init]) => JSON.parse(init!.body as string))).toEqual(urls.map((url) => ({ url, headers: [] })));
    expect(chromeMock.cookies.getAllCookieStores).not.toHaveBeenCalled();
    expect(chromeMock.cookies.getAll).not.toHaveBeenCalled();
  });
  it("右键 FTP(S) 手动提交不携带网页来源、UA 或 Cookie", async () => {
    for (const protocol of ["ftp", "ftps"]) {
      const url = `${protocol}://files.example.com/file.zip`;
      menuClicked({ menuItemId: "swoop-download", linkUrl: ` ${url} `, pageUrl }, tab);
      await vi.waitFor(() => expect(session.lastResult).toMatchObject({ ok: true, url }));
    }
    const posts = vi.mocked(fetch).mock.calls.filter(([, init]) => init?.method === "POST");
    expect(posts.map(([, init]) => JSON.parse(init!.body as string))).toEqual([
      { url: "ftp://files.example.com/file.zip", headers: [] }, { url: "ftps://files.example.com/file.zip", headers: [] },
    ]);
    expect(chromeMock.cookies.getAllCookieStores).not.toHaveBeenCalled();
    expect(chromeMock.cookies.getAll).not.toHaveBeenCalled();
  });
  it("FTP(S) 只能作为目标，FTP 来源页不执行提取或手动提交", async () => {
    chromeMock.tabs.get.mockResolvedValueOnce({ ...tab, url: "ftp://files.example.com/" });
    expect(await request({ type: "collect-page", tabId: 1 })).toEqual({ ok: false, error: "请在普通网页上使用此功能" });
    expect(chromeMock.scripting.executeScript).not.toHaveBeenCalled();
    menuClicked({ menuItemId: "swoop-download", linkUrl: "ftp://files.example.com/file.zip", pageUrl: "file:///tmp/index.html" }, tab);
    await vi.waitFor(() => expect(session.lastResult).toMatchObject({ ok: false, error: "请在普通网页上使用此功能" }));
    expect(fetch).not.toHaveBeenCalled();
    expect(chromeMock.cookies.getAll).not.toHaveBeenCalled();
  });
  it("右键拒绝 FTP URL 内嵌凭据，失败记录也不保存用户名或密码", async () => {
    menuClicked({ menuItemId: "swoop-download", linkUrl: "ftps://private-user:private-pass@files.example.com/file.zip", pageUrl }, tab);
    await vi.waitFor(() => expect(session.lastResult).toMatchObject({ ok: false, url: "ftps://files.example.com/file.zip" }));
    expect(JSON.stringify(session)).not.toContain("private-user");
    expect(JSON.stringify(session)).not.toContain("private-pass");
    expect(fetch).not.toHaveBeenCalled();
    expect(chromeMock.cookies.getAll).not.toHaveBeenCalled();
  });
  it("只读媒体监听可达选择页，HLS 使用 media 字段提交而非下载清单文件", async () => {
    expect(chromeMock.webRequest.onHeadersReceived.addListener).toHaveBeenCalledWith(expect.any(Function),
      { urls: ["http://*/*", "https://*/*"] }, ["responseHeaders"]);
    const url = "https://cdn.example.com/master.m3u8";
    capture({ tabId: 1, method: "GET", url, statusCode: 200, responseHeaders: [{ name: "Content-Type", value: "application/vnd.apple.mpegurl" }, { name: "Content-Length", value: "100" }] });
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 1 });
    const reply = await request<{ ok: true; batch: { id: string; kind: string } }>({ type: "open-media", tabId: 1 });
    expect(reply.batch.kind).toBe("media");
    await request({ type: "submit-page-link", batchId: reply.batch.id, url });
    const post = vi.mocked(fetch).mock.calls.find(([, init]) => init?.method === "POST");
    expect(JSON.parse(post![1]!.body as string)).toMatchObject({ url, media: "hls", mime: "application/vnd.apple.mpegurl" });
    expect(JSON.parse(post![1]!.body as string).size).toBeUndefined();
  });
  it("媒体关闭与标签关闭清空候选，不自动调用 /tasks", async () => {
    const response = { tabId: 1, method: "GET", url: "https://example.com/a.mp4", statusCode: 200 };
    capture(response);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 1 });
    closeTab(1);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 0 });
    settings.detectMedia = false;
    capture(response);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 0 });
    expect(await request({ type: "open-media", tabId: 1 })).toMatchObject({ ok: false });
    expect(vi.mocked(fetch).mock.calls.some(([, init]) => init?.method === "POST")).toBe(false);
  });
  it("旧桌面未声明媒体能力时不发送清单任务", async () => {
    vi.mocked(fetch).mockImplementation(async () => Response.json({ app: "swoop" }));
    const url = "https://example.com/master.m3u8";
    capture({ tabId: 1, method: "GET", url, statusCode: 200 });
    const reply = await request<{ batch: { id: string } }>({ type: "open-media", tabId: 1 });
    expect(await request({ type: "submit-page-link", batchId: reply.batch.id, url })).toEqual({ ok: false, error: "当前 Swoop 尚不支持媒体接收，请更新后重试" });
    expect(vi.mocked(fetch).mock.calls.some(([, init]) => init?.method === "POST")).toBe(false);
  });
  it("并发媒体响应不会丢项，关闭事件排在未完成捕获后清理", async () => {
    const response = { tabId: 1, method: "GET", url: "https://example.com/a.mp4", statusCode: 200 };
    capture(response);
    capture(response);
    capture({ ...response, url: "https://example.com/b.mp3" });
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 2 });
    capture({ ...response, url: "https://example.com/c.mp4" });
    closeTab(1);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 0 });
  });
  it("禁用会清理已有媒体，隐身标签不被动记录", async () => {
    const response = { tabId: 1, method: "GET", url: "https://example.com/a.mp4", statusCode: 200 };
    capture(response);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 1 });
    settings.detectMedia = false;
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 0 });
    settings.detectMedia = true;
    chromeMock.tabs.get.mockResolvedValueOnce({ ...tab, incognito: true });
    capture(response);
    expect(await request({ type: "media-count", tabId: 1 })).toEqual({ ok: true, count: 0 });
  });
});
