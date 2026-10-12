// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ENGLISH, localizeDocument, translate, translateError } from "./i18n";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings";
import { badgeFor } from "./badge";
import { ago } from "./format";

type StorageListener = (changes: Record<string, { oldValue?: unknown; newValue?: unknown }>, area: string) => void;
let listeners: StorageListener[] = [];
let stored: Record<string, unknown>;
let session: Record<string, unknown>;
const chromeMock = {
  storage: {
    sync: { get: vi.fn(async () => ({ ...stored })), set: vi.fn(async (patch: Record<string, unknown>) => {
      const changes = Object.fromEntries(Object.entries(patch).map(([key, newValue]) => [key, { oldValue: stored[key], newValue }]));
      Object.assign(stored, patch);
      listeners.forEach((listener) => listener(changes, "sync"));
    }) },
    session: { get: vi.fn(async () => ({ ...session })), set: vi.fn(async (patch: Record<string, unknown>) => { Object.assign(session, patch); }) },
    onChanged: { addListener: vi.fn((listener: StorageListener) => { listeners.push(listener); }) },
  },
  tabs: { query: vi.fn(async () => [{ id: 1 }]) },
  runtime: {
    openOptionsPage: vi.fn(),
    getManifest: () => ({ version: "0.3.0" }),
    sendMessage: vi.fn(async (request: { type: string }) => request.type === "media-count"
      ? { ok: true, count: 3 } : request.type === "refresh" ? true : { ok: false, error: "请在普通网页上使用此功能" }),
  },
};
const get = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
function loadDocument(name: string): void {
  const html = readFileSync(`public/${name}.html`, "utf8").replace(/<link[^>]*>/g, "");
  const parsed = new DOMParser().parseFromString(html, "text/html");
  document.head.innerHTML = parsed.head.innerHTML;
  document.body.innerHTML = parsed.body.innerHTML.replace(/<script[\s\S]*?<\/script>/g, "");
}
beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  listeners = [];
  stored = { ...DEFAULT_SETTINGS };
  session = { connected: false, unseenFailure: false };
  vi.stubGlobal("chrome", chromeMock);
  vi.stubGlobal("browser", undefined);
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ app: "swoop" })));
});

describe("扩展双语", () => {
  it("旧设置与非法语言默认中文，英文可持久化并在模块重载后恢复", async () => {
    expect(normalizeSettings({}).language).toBe("zh-CN");
    expect(normalizeSettings({ language: "auto" }).language).toBe("zh-CN");
    const first = await import("./settings");
    await first.saveSettings({ language: "en-US" });
    vi.resetModules();
    expect((await (await import("./settings")).loadSettings()).language).toBe("en-US");
    expect(chromeMock.storage.sync.get).toHaveBeenCalledWith(expect.arrayContaining(["language"]));
  });
  it("所有静态页面文案有翻译，属性与中文可逆切换，不改配置值或 DOM id", () => {
    for (const name of ["options", "popup", "links", "welcome"]) {
      loadDocument(name);
      const ids = [...document.querySelectorAll("[id]")].map((node) => node.id);
      for (const node of document.querySelectorAll<HTMLElement>("[data-i18n]")) expect(ENGLISH[node.dataset.i18n!]).toBeTruthy();
      localizeDocument(document, "en-US");
      expect(document.documentElement.lang).toBe("en-US");
      expect(document.body.textContent?.replace("简体中文", "")).not.toMatch(/[\p{Script=Han}]/u);
      if (name === "links") expect(get<HTMLInputElement>("query").placeholder).toBe("Filter by name or URL");
      if (name === "options") expect([...get<HTMLSelectElement>("language").options].map((option) => option.value)).toEqual(["zh-CN", "en-US"]);
      localizeDocument(document, "zh-CN");
      expect(document.documentElement.lang).toBe("zh-CN");
      expect([...document.querySelectorAll("[id]")].map((node) => node.id)).toEqual(ids);
      for (const node of document.querySelectorAll<HTMLElement>("[data-i18n]")) expect(node.textContent).toBe(node.dataset.i18n);
    }
    for (const [key, value] of Object.entries(ENGLISH)) {
      expect(value.match(/\{\w+\}/g)?.sort() ?? []).toEqual(key.match(/\{\w+\}/g)?.sort() ?? []);
    }
  });
  it("设置页面真实 change/click 保存语言，校验与连接结果双语，保存失败不改变存储", async () => {
    loadDocument("options");
    await import("./options");
    await vi.waitFor(() => expect(get<HTMLInputElement>("port").value).toBe("51230"));
    const language = get<HTMLSelectElement>("language");
    language.value = "en-US";
    language.dispatchEvent(new Event("change"));
    expect(document.title).toBe("Swoop extension settings");
    get<HTMLInputElement>("port").value = "10";
    get("save").click();
    expect(get("msg").textContent).toBe("Port must be between 1024 and 65535.");
    expect(chromeMock.storage.sync.set).not.toHaveBeenCalled();
    get<HTMLInputElement>("port").value = "51230";
    get("save").click();
    await vi.waitFor(() => expect(get("msg").textContent).toBe("Saved"));
    expect(stored.language).toBe("en-US");
    expect(stored.enabled).toBe(true);
    get("test").click();
    await vi.waitFor(() => expect(get("test-result").textContent).toBe("✓ Connected"));
    vi.resetModules(); listeners = []; loadDocument("options");
    await import("./options");
    await vi.waitFor(() => expect(document.title).toBe("Swoop extension settings"));
    get<HTMLSelectElement>("language").value = "zh-CN";
    get<HTMLSelectElement>("language").dispatchEvent(new Event("change"));
    chromeMock.storage.sync.set.mockRejectedValueOnce(new Error("quota"));
    get("save").click();
    await vi.waitFor(() => expect(get("msg").textContent).toBe("保存失败，请重试"));
    expect(stored.language).toBe("en-US");
  });
  it("弹窗动态状态、媒体数量、失败和回退提示随保存事件切换，保留站点文件名", async () => {
    stored.language = "en-US";
    session.lastResult = { ok: false, url: "https://example.com/file", fileName: "原始文件.zip", error: "无法连接 Swoop", fellBack: true, at: Date.now() - 120_000 };
    loadDocument("popup");
    await import("./popup");
    await vi.waitFor(() => expect(get("open-media").textContent).toBe("Page media (3)…"));
    expect(get("status").textContent).toBe("Not connected to Swoop");
    expect(get("result-title").textContent).toBe("Transfer failed · 2 min ago");
    expect(get("result-detail").textContent).toBe("Could not connect to Swoop. · Download resumed in the browser.");
    expect(get("result-name").textContent).toBe("原始文件.zip");
    get("collect-page").click();
    await vi.waitFor(() => expect(get("collect-error").textContent).toBe("Use this feature on an HTTP or HTTPS page."));
    await chromeMock.storage.sync.set({ language: "zh-CN" });
    await vi.waitFor(() => expect(get("open-media").textContent).toBe("网页媒体（3）…"));
    expect(get("collect-error").textContent).toBe("请在普通网页上使用此功能");
    expect(get("result-detail").textContent).toBe("无法连接 Swoop，已由浏览器继续下载");
  });
  it("欢迎页检测连接、切换语言写入设置，未连接时给出端口提示", async () => {
    stored.language = "en-US";
    stored.port = 51999;
    const original = chromeMock.runtime.sendMessage.getMockImplementation()!;
    const interval = vi.spyOn(globalThis, "setInterval").mockReturnValue(0 as never);
    chromeMock.runtime.sendMessage.mockImplementation(async () => false as never);
    try {
    loadDocument("welcome");
    await import("./welcome");
    await vi.waitFor(() => expect(get("status").textContent).toBe("Not connected to Swoop yet"));
    expect(document.title).toBe("Welcome to Swoop");
    expect(get("version").textContent).toBe("v0.3.0");
    expect(get("help").hidden).toBe(false);
    expect(get("port-hint").textContent).toContain("51999");
    chromeMock.runtime.sendMessage.mockImplementation(async () => true as never);
    get("retry").click();
    await vi.waitFor(() => expect(get("status").textContent).toBe("✓ Connected to Swoop. You're ready to download."));
    expect(get("help").hidden).toBe(true);
    expect(get("retry").hidden).toBe(true);
    const language = get<HTMLSelectElement>("language");
    language.value = "zh-CN";
    language.dispatchEvent(new Event("change"));
    expect(document.title).toBe("欢迎使用 Swoop");
    expect(stored.language).toBe("zh-CN");
    await vi.waitFor(() => expect(get("status").textContent).toBe("✓ 已连接 Swoop，可以开始下载了"));
    stored.enabled = false;
    await chromeMock.storage.sync.set({ enabled: false });
    await vi.waitFor(() => expect(get("status").textContent).toBe("接管已关闭，请先在扩展弹窗中开启"));
    expect(interval).toHaveBeenCalledWith(expect.any(Function), 3000);
    } finally {
      chromeMock.runtime.sendMessage.mockImplementation(original);
      interval.mockRestore();
    }
  });
  it("失效链接页也使用保存语言并响应切换", async () => {
    stored.language = "en-US";
    loadDocument("links");
    await import("./links");
    await vi.waitFor(() => expect(get("notice").textContent).toContain("This link list expired"));
    expect(document.title).toBe("Page links · Swoop");
    await chromeMock.storage.sync.set({ language: "zh-CN" });
    await vi.waitFor(() => expect(get("notice").textContent).toBe("链接列表已过期，请返回原网页重新提取"));
  });
  it("徽标与时间双语，已知扩展错误翻译而外部原文不变", () => {
    expect(badgeFor({ connected: false, unseenFailure: false }, true, "en-US").title).toBe("Swoop: Not connected (start Swoop)");
    expect(badgeFor({ connected: false, unseenFailure: true }, true, "en-US").title).toBe("Swoop: Last transfer failed");
    expect(ago(0, 86_400_000, "en-US")).toBe("1 d ago");
    expect(translateError("Swoop 返回 HTTP 403", "en-US")).toBe("Swoop returned HTTP 403");
    expect(translateError("服务器自定义错误 {token}", "en-US")).toBe("服务器自定义错误 {token}");
    expect(translate("选择 {title}", "en-US", { title: "<img src=x>" })).toBe("Select <img src=x>");
  });
  it("后台可展示的固定错误都有英文映射", () => {
    for (const file of ["api", "background", "batch", "links", "media-catalog", "page-tasks", "rules"]) {
      const source = readFileSync(`src/${file}.ts`, "utf8");
      for (const [, text] of source.matchAll(/(?:error:\s*|return\s*)"([^"\n]*[\p{Script=Han}][^"\n]*)"/gu)) {
        expect(ENGLISH[text!], `${file}: ${text}`).toBeTruthy();
      }
    }
  });
});
