import { describe, expect, it } from "vitest";
import { detectMedia, mergeCandidates, type MediaResponse } from "./media";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings";

const response = (patch: Partial<MediaResponse> = {}): MediaResponse => ({
  tabId: 1, method: "GET", statusCode: 200, url: "https://example.com/movie.mp4", ...patch,
});

describe("媒体响应识别", () => {
  it("按 URL 或响应类型识别渐进音视频、HLS、DASH", () => {
    expect(detectMedia(response(), 1)).toMatchObject({ kind: "video", extension: "mp4" });
    expect(detectMedia(response({ url: "https://example.com/sound", responseHeaders: [{ name: "Content-Type", value: "audio/mpeg; charset=binary" }] }), 1))
      .toMatchObject({ kind: "audio", mime: "audio/mpeg" });
    expect(detectMedia(response({ url: "https://example.com/master.m3u8" }), 1)).toMatchObject({ kind: "hls", media: "hls" });
    expect(detectMedia(response({ url: "https://example.com/play.mpd", responseHeaders: [{ name: "content-type", value: "application/xml" }] }), 1))
      .toMatchObject({ kind: "dash", media: "dash" });
  });
  it("忽略后台/失败/POST/blob/FTP(S)/分片/密钥/网页响应", () => {
    for (const patch of [
      { tabId: -1 }, { statusCode: 403 }, { statusCode: 302 }, { method: "POST" },
      { url: "blob:https://example.com/id" }, { url: "https://example.com/part.m4s" }, { url: "https://example.com/key.key" },
      { url: "ftp://example.com/movie.mp4" }, { url: "ftps://example.com/master.m3u8" },
      { responseHeaders: [{ name: "content-type", value: "text/html" }] },
      { responseHeaders: [{ name: "content-type", value: "video/mp2t" }] },
    ]) expect(detectMedia(response(patch), 1)).toBeUndefined();
  });
  it("Range 响应使用总大小，清单不把文本大小传给桌面媒体任务", () => {
    const headers = [{ name: "Content-Range", value: "bytes 0-1023/50000" }, { name: "Content-Length", value: "1024" }];
    expect(detectMedia(response({ statusCode: 206, responseHeaders: headers }), 1)?.size).toBe(50000);
    expect(detectMedia(response({ statusCode: 206, responseHeaders: headers.slice(1) }), 1)?.size).toBeUndefined();
    expect(detectMedia(response({ url: "https://example.com/a.m3u8", responseHeaders: headers }), 1)?.size).toBeUndefined();
  });
  it("按完整 URL 去重、刷新时间、保留查询签名、清理过期并限制数量", () => {
    const a = detectMedia(response({ url: "https://example.com/a.mp4?signature=1#x" }), 10)!;
    const b = detectMedia(response({ url: "https://example.com/a.mp4?signature=2" }), 20)!;
    const freshA = { ...a, detectedAt: 30 };
    expect(a.url).toBe("https://example.com/a.mp4?signature=1");
    expect(mergeCandidates([a, b], freshA, 2, 100, 30)).toEqual([freshA, b]);
    expect(mergeCandidates([a, b], freshA, 1, 100, 30)).toEqual([freshA]);
    expect(mergeCandidates([a, b], undefined, 10, 15, 30)).toEqual([b]);
  });
  it("媒体配置有边界且旧设置补默认", () => {
    expect(normalizeSettings({ maxMediaCandidates: 501, mediaRetentionMinutes: 0, detectMedia: "false" })).toEqual(DEFAULT_SETTINGS);
    expect(normalizeSettings({ maxMediaCandidates: 10, mediaRetentionMinutes: 1440, detectMedia: false }))
      .toMatchObject({ maxMediaCandidates: 10, mediaRetentionMinutes: 1440, detectMedia: false });
  });
});
