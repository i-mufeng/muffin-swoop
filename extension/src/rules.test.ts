import { describe, expect, it } from "vitest";

import { badgeFor } from "./badge";
import { formatCookies } from "./cookies";
import { ago, bytes } from "./format";
import { knownSize, matchesDomain, parseDomainList, shouldIntercept, shouldOpenWelcome, type DownloadLike } from "./rules";
import { DEFAULT_SETTINGS, normalizeSettings, type Settings } from "./settings";

const item = (patch: Partial<DownloadLike> = {}): DownloadLike => ({
  url: "https://example.com/file.zip",
  fileSize: -1,
  totalBytes: 0,
  incognito: false,
  ...patch,
});
const settings = (patch: Partial<Settings> = {}): Settings => ({ ...DEFAULT_SETTINGS, ...patch });

describe("shouldIntercept", () => {
  it("默认接管普通 http(s) 下载", () => {
    expect(shouldIntercept(item(), settings())).toEqual({ take: true });
    expect(shouldIntercept(item({ url: "http://a.com/x" }), settings()).take).toBe(true);
  });

  it("总开关关闭时放行", () => {
    expect(shouldIntercept(item(), settings({ enabled: false })).take).toBe(false);
  });

  it("blob/data/file、FTP(S) 与无效链接不自动接管", () => {
    for (const url of ["blob:https://a.com/uuid", "data:text/plain,hi", "file:///tmp/a", "ftp://a.com/x", "ftps://a.com/x", "not a url"]) {
      expect(shouldIntercept(item({ url }), settings()).take, url).toBe(false);
    }
  });

  it("隐身窗口放行", () => {
    expect(shouldIntercept(item({ incognito: true }), settings()).take).toBe(false);
  });

  it("排除域名包含子域名", () => {
    const s = settings({ excludedDomains: ["example.com"] });
    expect(shouldIntercept(item(), s).take).toBe(false);
    expect(shouldIntercept(item({ url: "https://dl.example.com/a" }), s).take).toBe(false);
    expect(shouldIntercept(item({ url: "https://notexample.com/a" }), s).take).toBe(true);
  });

  it("大小阈值：已知且偏小才放行，未知照常接管", () => {
    const s = settings({ minSizeMB: 1 });
    expect(shouldIntercept(item({ fileSize: 500_000 }), s).take).toBe(false);
    expect(shouldIntercept(item({ totalBytes: 2 * 1024 * 1024 }), s).take).toBe(true);
    expect(shouldIntercept(item(), s).take).toBe(true);
  });
});

describe("helpers", () => {
  it("knownSize 优先 fileSize，未知返回 undefined", () => {
    expect(knownSize({ fileSize: 10, totalBytes: 20 })).toBe(10);
    expect(knownSize({ fileSize: -1, totalBytes: 20 })).toBe(20);
    expect(knownSize({ fileSize: 0, totalBytes: 0 })).toBeUndefined();
  });

  it("matchesDomain 不区分大小写", () => {
    expect(matchesDomain("A.Example.COM", "example.com")).toBe(true);
    expect(matchesDomain("example.com", "a.example.com")).toBe(false);
  });

  it("parseDomainList 容忍各种粘贴格式", () => {
    expect(parseDomainList("https://Drive.Google.com/file/d/1?x=1\n*.example.com, foo.org:8080；.bar.net  foo.org")).toEqual([
      "drive.google.com",
      "example.com",
      "foo.org",
      "bar.net",
    ]);
    expect(parseDomainList("  \n ")).toEqual([]);
  });

  it("formatCookies 按 path 长度排序", () => {
    expect(formatCookies([])).toBeUndefined();
    expect(
      formatCookies([
        { name: "a", value: "1", path: "/" },
        { name: "b", value: "2", path: "/repo/releases" },
        { name: "", value: "bare", path: "/repo" },
      ]),
    ).toBe("b=2; bare; a=1");
  });

  it("normalizeSettings 修正非法值", () => {
    expect(normalizeSettings({})).toEqual(DEFAULT_SETTINGS);
    expect(normalizeSettings({ port: 80, minSizeMB: -3, excludedDomains: "x", enabled: "yes" })).toEqual(DEFAULT_SETTINGS);
    expect(normalizeSettings({ port: 6000, minSizeMB: 2, excludedDomains: ["a.com", 3, ""], enabled: false })).toEqual({
      ...DEFAULT_SETTINGS,
      enabled: false,
      port: 6000,
      minSizeMB: 2,
      excludedDomains: ["a.com"],
    });
  });

  it("badgeFor：失败优先，其次断开", () => {
    expect(badgeFor({ connected: true, unseenFailure: false }, true).text).toBe("");
    expect(badgeFor({ connected: false, unseenFailure: false }, true).text).toBe("•");
    expect(badgeFor({ connected: true, unseenFailure: true }, true).text).toBe("!");
    expect(badgeFor({ connected: false, unseenFailure: false }, false).text).toBe("");
  });

  it("format", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(1536)).toBe("1.5 KB");
    expect(bytes(250 * 1024 * 1024)).toBe("250 MB");
    expect(ago(1_000_000, 1_000_000 + 30_000)).toBe("刚刚");
    expect(ago(0, 3 * 3600_000)).toBe("3 小时前");
  });
});

describe("欢迎页触发", () => {
  it("首次安装与主/次版本更新打开，补丁更新、浏览器更新和缺失旧版本不打开", () => {
    expect(shouldOpenWelcome("install", undefined, "0.3.0")).toBe(true);
    expect(shouldOpenWelcome("update", "0.3.0", "0.4.0")).toBe(true);
    expect(shouldOpenWelcome("update", "0.9.2", "1.0.0")).toBe(true);
    expect(shouldOpenWelcome("update", "0.3.0", "0.3.1")).toBe(false);
    expect(shouldOpenWelcome("update", "0.3", "0.3.2")).toBe(false);
    expect(shouldOpenWelcome("update", undefined, "0.4.0")).toBe(false);
    expect(shouldOpenWelcome("browser_update", undefined, "0.4.0")).toBe(false);
    expect(shouldOpenWelcome("chrome_update", "0.3.0", "0.4.0")).toBe(false);
  });
});
