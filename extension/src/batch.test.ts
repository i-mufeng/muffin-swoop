import { describe, expect, it } from "vitest";
import { LinkBatch } from "./batch";
import type { HandoffResult } from "./api";
import { filterPageLinks, manualDownloadBlock, normalizePageLinks } from "./rules";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings";

const page = "https://example.com/page";
const links = normalizePageLinks([
  { url: "/a.zip", title: "Archive" }, { url: "/b.pdf", title: "Report" }, { url: "/c.zip", title: "Other" },
], page, 100).links;
const success: HandoffResult = { ok: true, fileName: "a.zip", size: 5 };

describe("页面链接规则", () => {
  it("忽略不支持的协议及当前页片段，按最终请求 URL 去重，查询参数区分", () => {
    const result = normalizePageLinks([
      { url: "#top", title: "Top" }, { url: "/a.zip#one", title: "One" },
      { url: "https://example.com/a.zip#two", title: "Two" }, { url: "/a.zip?v=2", title: "" },
      ...["javascript:alert(1)", "blob:https://x/y", "mailto:a@b.com", "data:,x", "file:///tmp/a", "https://["].map((url) => ({ url, title: "" })),
    ], page, 20);
    expect(result.links.map((link) => link.url)).toEqual(["https://example.com/a.zip", "https://example.com/a.zip?v=2"]);
    expect(result.links[1]).toMatchObject({ title: "a.zip", extension: "zip" });
    expect(result.truncated).toBe(false);
  });
  it("手动链接接受 FTP(S) 文件，片段去重且不保存凭据与非法 FTP 目标", () => {
    const allowed = ["ftp://files.example.com/archive.zip", "ftps://files.example.com/%E6%96%87%E4%BB%B6.pdf"];
    const rejected = ["ftp://user:password@files.example.com/file", "ftp://@files.example.com/file",
      "ftps:///file", "ftp://files.example.com/dir/", "ftp://files.example.com/file?query=1",
      "ftp://files.example.com/a%0d%0aDELE%20x", "ftp://files.example.com/a%00", "ftp://files.example.com/a;type=a"];
    expect(normalizePageLinks([...allowed, allowed[0] + "#part", ...rejected].map((url) => ({ url, title: "" })), page, 20)
      .links.map((link) => link.url)).toEqual(allowed);
    for (const url of allowed) expect(manualDownloadBlock(url, DEFAULT_SETTINGS)).toBeUndefined();
    for (const url of rejected) expect(manualDownloadBlock(url, DEFAULT_SETTINGS), url).toBeDefined();
    expect(manualDownloadBlock(allowed[0]!, { ...DEFAULT_SETTINGS, excludedDomains: ["example.com"] })).toContain("排除");
  });
  it("提取上限按去重后链接计数", () => {
    const raw = [links[0]!, links[0]!, links[1]!, links[2]!];
    expect(normalizePageLinks(raw, page, 2)).toEqual({ links: links.slice(0, 2), truncated: true });
    expect(normalizePageLinks(raw.slice(0, 3), page, 2).truncated).toBe(false);
  });
  it("搜索名称、网址，并与文件类型共同过滤", () => {
    expect(filterPageLinks(links, "REPORT", "*")).toEqual([links[1]]);
    expect(filterPageLinks(links, "example.com", "zip")).toEqual([links[0], links[2]]);
    expect(filterPageLinks(links, "", "-")).toEqual([]);
  });
  it("手动提交也遵守停用与排除域名", () => {
    expect(manualDownloadBlock(links[0]!.url, DEFAULT_SETTINGS)).toBeUndefined();
    expect(manualDownloadBlock(links[0]!.url, { ...DEFAULT_SETTINGS, enabled: false })).toContain("已关闭");
    expect(manualDownloadBlock(links[0]!.url, { ...DEFAULT_SETTINGS, excludedDomains: ["example.com"] })).toContain("排除");
    expect(manualDownloadBlock("blob:https://a/b", DEFAULT_SETTINGS)).toContain("只支持");
  });
  it("并发与提取上限有合法范围，旧设置自动补默认值", () => {
    expect(normalizeSettings({ batchConcurrency: 0, maxPageLinks: 99999 })).toEqual(DEFAULT_SETTINGS);
    expect(normalizeSettings({ batchConcurrency: 6, maxPageLinks: 10000 })).toMatchObject({ batchConcurrency: 6, maxPageLinks: 10000 });
    expect(normalizeSettings({ batchConcurrency: 2.5, maxPageLinks: 1 })).toEqual(DEFAULT_SETTINGS);
  });
});

describe("批量交互状态", () => {
  it("默认无选择，只发送所选；每项记录实际成功或失败，成功项不会再发", async () => {
    const batch = new LinkBatch(links);
    expect(batch.pending).toEqual([]);
    batch.select([links[0]!.url, links[1]!.url], true);
    const sent: string[] = [];
    await batch.run(2, async (link) => {
      sent.push(link.url);
      return link.extension === "pdf" ? { ok: false, error: "登录过期" } : success;
    });
    expect(sent).toEqual([links[0]!.url, links[1]!.url]);
    expect(batch.items.map((item) => item.status)).toEqual(["success", "error", "ready"]);
    expect(batch.items[1]?.result).toEqual({ ok: false, error: "登录过期" });
    batch.select([links[0]!.url], true);
    await batch.run(2, async (link) => { sent.push(link.url); return success; });
    expect(sent).toEqual([links[0]!.url, links[1]!.url, links[1]!.url]);
  });
  it("并发不超过设置；停止只阻止后续，已发送项等待真实结果", async () => {
    const batch = new LinkBatch(links);
    batch.select(links.map((link) => link.url), true);
    const complete: ((result: HandoffResult) => void)[] = [];
    let active = 0;
    let peak = 0;
    const running = batch.run(2, async () => {
      active++;
      peak = Math.max(peak, active);
      const result = await new Promise<HandoffResult>((resolve) => complete.push(resolve));
      active--;
      return result;
    });
    expect(complete).toHaveLength(2);
    batch.stop();
    complete.forEach((resolve) => resolve(success));
    await running;
    expect(peak).toBe(2);
    expect(batch.items.map((item) => item.status)).toEqual(["success", "success", "ready"]);
    expect(batch.pending).toHaveLength(1);
  });
  it("发送异常逐项失败，运行中重复点击不会创建第二个批次", async () => {
    const batch = new LinkBatch(links);
    batch.select([links[0]!.url], true);
    let sends = 0;
    const send = async () => { sends++; throw new Error("连接中断"); };
    await Promise.all([batch.run(1, send), batch.run(1, send)]);
    expect(sends).toBe(1);
    expect(batch.items[0]?.result).toEqual({ ok: false, error: "连接中断" });
    expect(batch.running).toBe(false);
  });
});
