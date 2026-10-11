import { describe, expect, it } from "vitest";
import { contextHeaders, safeReferrer } from "./request-context";

describe("提交请求的上下文", () => {
  it("同源保留路径但删除凭据和片段，跨源仅保留来源站点", () => {
    expect(safeReferrer("https://example.com/file", "https://user:pass@example.com/page?key=1#secret"))
      .toBe("https://example.com/page?key=1");
    expect(safeReferrer("https://cdn.example.com/file", "https://example.com/page?key=1#secret"))
      .toBe("https://example.com/");
  });
  it("HTTPS 降级、内部页面与无效来源均不传 Referer", () => {
    expect(safeReferrer("http://example.com/file", "https://example.com/page")).toBeUndefined();
    expect(safeReferrer("https://example.com/file", "chrome://downloads")).toBeUndefined();
    expect(safeReferrer("https://example.com/file", "invalid")).toBeUndefined();
  });
  it("只传 UA 与 Cookie，拒绝含控制字符的头值", () => {
    expect(contextHeaders("Browser/1.0", "a=1; b=2")).toEqual([["User-Agent", "Browser/1.0"], ["Cookie", "a=1; b=2"]]);
    expect(contextHeaders("Browser\r\nInjected: bad", "bad=\u0000")).toEqual([]);
  });
  it("尊重页面更严格的来源策略", () => {
    expect(safeReferrer("https://example.com/file", "https://example.com/page", "no-referrer")).toBeUndefined();
    expect(safeReferrer("https://cdn.example.com/file", "https://example.com/page", "same-origin")).toBeUndefined();
    expect(safeReferrer("https://example.com/file", "https://example.com/page", "origin")).toBe("https://example.com/");
  });
});
