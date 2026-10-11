// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { collectDocumentLinks } from "./collect";
import { mountLinkPicker } from "./links-view";
import type { PageBatch } from "./page-tasks";
import { DEFAULT_SETTINGS } from "./settings";

const page: PageBatch = {
  id: "batch", tabId: 1, pageUrl: "https://example.com/page", title: "示例", incognito: false, truncated: false, createdAt: 1,
  links: [
    { url: "https://example.com/a.zip", title: "Archive", extension: "zip" },
    { url: "https://example.com/b.pdf", title: "<script>bad()</script>", extension: "pdf" },
  ],
};
const get = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

beforeEach(() => {
  document.head.innerHTML = "";
  document.body.innerHTML = readFileSync("public/links.html", "utf8").split("<body>")[1]!.split("<script")[0]!;
});

describe("链接选择页面", () => {
  it("运行中切换语言保留选择与结果，停止状态、无扩展名和可访问标签同步翻译", async () => {
    let complete!: (result: { ok: false; error: string }) => void;
    const send = vi.fn(() => new Promise<{ ok: false; error: string }>((resolve) => { complete = resolve; }));
    const mediaPage: PageBatch = { ...page, kind: "media", links: [{ ...page.links[0]!, extension: "" }, page.links[1]!] };
    const view = mountLinkPicker(document, mediaPage, { ...DEFAULT_SETTINGS, language: "en-US", batchConcurrency: 1 }, send);
    expect(document.querySelector("h1")!.textContent).toBe("Page media");
    expect(get<HTMLSelectElement>("extension").options[1]!.textContent).toBe("No extension");
    get("select-visible").click(); get("submit").click(); get("stop").click();
    expect(get("stop").textContent).toBe("Waiting for current submissions…");
    expect(get("links").querySelector("input")!.getAttribute("aria-label")).toBe("Select Archive");
    view.updateSettings({ ...DEFAULT_SETTINGS, batchConcurrency: 1 });
    expect(get("stop").textContent).toBe("正在等待当前提交…");
    expect(get("links").textContent).toContain("提交中…");
    complete({ ok: false, error: "无法连接 Swoop" });
    await vi.waitFor(() => expect(view.batch.running).toBe(false));
    expect(send).toHaveBeenCalledTimes(1);
    view.updateSettings({ ...DEFAULT_SETTINGS, language: "en-US" });
    expect(get("links").textContent).toContain("Could not connect to Swoop.");
    expect(get("summary").textContent).toContain("Selected 2");
    expect(get("links").querySelector("script")).toBeNull();
  });
  it("筛选后全选只影响可见行，并展示实际响应与可重试失败", async () => {
    const send = vi.fn(async () => ({ ok: false as const, error: "需要登录" }));
    mountLinkPicker(document, page, DEFAULT_SETTINGS, send);
    expect(get<HTMLButtonElement>("submit").disabled).toBe(true);
    const query = get<HTMLInputElement>("query");
    query.value = "Archive";
    query.dispatchEvent(new Event("input"));
    get("select-visible").click();
    expect(get("summary").textContent).toContain("已选 1");
    get("submit").click();
    await vi.waitFor(() => expect(get("links").textContent).toContain("需要登录"));
    expect(send).toHaveBeenCalledExactlyOnceWith(page.links[0]);
    expect(get<HTMLButtonElement>("submit").disabled).toBe(false);
    expect(get("summary").textContent).toContain("失败 1");
  });
  it("切换扩展名不丢选中状态，关闭接管立刻禁用提交", () => {
    const send = vi.fn();
    const view = mountLinkPicker(document, page, DEFAULT_SETTINGS, send);
    get("select-visible").click();
    const extension = get<HTMLSelectElement>("extension");
    extension.value = "pdf";
    extension.dispatchEvent(new Event("change"));
    get("clear-visible").click();
    expect(view.batch.pending.map((item) => item.link.url)).toEqual([page.links[0]!.url]);
    view.updateSettings({ ...DEFAULT_SETTINGS, enabled: false });
    expect(get<HTMLButtonElement>("submit").disabled).toBe(true);
    expect(get("notice").textContent).toContain("已关闭");
    get("submit").click();
    expect(send).not.toHaveBeenCalled();
  });
  it("网页文字按纯文本渲染，成功后不允许重复勾选", async () => {
    const send = vi.fn(async () => ({ ok: true as const, fileName: "archive.zip", size: 3 }));
    mountLinkPicker(document, page, DEFAULT_SETTINGS, send);
    expect(get("links").querySelector("script")).toBeNull();
    expect(get("links").textContent).toContain("<script>bad()</script>");
    get("select-visible").click();
    get("submit").click();
    await vi.waitFor(() => expect(get("summary").textContent).toContain("已提交 2"));
    expect([...get("links").querySelectorAll("input")].every((input) => input.disabled)).toBe(true);
    get("select-visible").click();
    expect(get<HTMLButtonElement>("submit").disabled).toBe(true);
    expect(send).toHaveBeenCalledTimes(2);
  });
  it("提取仅包含超链接，不扫描图片、脚本与音视频资源，并保留 base URL", () => {
    document.head.innerHTML = '<base href="https://downloads.example.com/release/">';
    document.body.innerHTML = '<a href="a.zip">Archive</a><area href="b.pdf" alt="Map"><img src="image.png"><script type="text/plain" src="site.js"></script><video src="movie.mp4"></video>';
    const capture = collectDocumentLinks(100);
    expect(capture.links).toEqual([
      { url: "https://downloads.example.com/release/a.zip", title: "Archive" },
      { url: "https://downloads.example.com/release/b.pdf", title: "Map" },
    ]);
    expect(collectDocumentLinks(1).truncated).toBe(true);
    // 浏览器注入的是函数源码，不能依赖扩展模块里的语言或翻译闭包。
    const injected = new Function(`return (${collectDocumentLinks.toString()})`)() as typeof collectDocumentLinks;
    expect(injected(100)).toEqual(capture);
  });
  it("保留 noreferrer，非法链接不误报为上限截断", () => {
    document.body.innerHTML = '<a href="https://example.com/a" rel="noreferrer">下载</a><a href="https://[">无效</a>';
    const capture = collectDocumentLinks(10);
    expect(capture.links[0]?.referrerPolicy).toBe("no-referrer");
    expect(capture.truncated).toBe(false);
  });
});
