import type { PageLink } from "./rules";

export interface MediaResponse {
  url: string;
  tabId: number;
  method: string;
  statusCode: number;
  responseHeaders?: { name: string; value?: string }[];
}
export interface MediaCandidate extends PageLink {
  kind: "video" | "audio" | "hls" | "dash";
  detectedAt: number;
}

/** 只查看 URL 与响应元数据；不读取响应体、密钥或许可证请求。 */
export function detectMedia(response: MediaResponse, now: number): MediaCandidate | undefined {
  if (response.tabId < 0 || response.method !== "GET" || ![200, 206].includes(response.statusCode)) return undefined;
  let url: URL;
  try { url = new URL(response.url); } catch { return undefined; }
  if (!/^https?:$/.test(url.protocol) || url.username || url.password || url.href.length > 8192) return undefined;
  url.hash = "";
  const header = (name: string) => response.responseHeaders?.find((header) => header.name.toLowerCase() === name)?.value;
  const mime = header("content-type")?.split(";")[0]?.trim().toLowerCase();
  let name = url.pathname.split("/").pop() || url.hostname;
  try { name = decodeURIComponent(name); } catch { /* 不完整编码保留原值 */ }
  const extension = /\.([a-z0-9]{1,12})$/i.exec(name)?.[1]?.toLowerCase() ?? "";
  // 分片和许可证不作为独立文件候选，避免播放时淹没列表。
  if (["ts", "m4s", "cmfv", "cmfa", "key"].includes(extension) || mime === "video/mp2t") return undefined;
  if (mime === "text/html" || mime === "application/json" || (mime === "application/xml" && extension !== "mpd")) return undefined;
  let kind: MediaCandidate["kind"];
  if (extension === "m3u8" || ["application/vnd.apple.mpegurl", "application/x-mpegurl", "audio/mpegurl", "audio/x-mpegurl"].includes(mime ?? "")) kind = "hls";
  else if (extension === "mpd" || mime === "application/dash+xml") kind = "dash";
  else if (mime?.startsWith("video/") || ["mp4", "webm", "mov", "mkv", "m4v", "avi", "flv", "ogv"].includes(extension)) kind = "video";
  else if (mime?.startsWith("audio/") || ["mp3", "m4a", "ogg", "opus", "wav", "flac", "aac"].includes(extension)) kind = "audio";
  else return undefined;
  const rawSize = response.statusCode === 206 ? /\/([0-9]+)$/.exec(header("content-range") ?? "")?.[1] : header("content-length");
  const size = rawSize && /^\d+$/.test(rawSize) ? Number(rawSize) : undefined;
  const media = kind === "hls" || kind === "dash" ? kind : undefined;
  return {
    url: url.href, title: `${name.slice(0, 260)} · ${kind.toUpperCase()}`, extension: extension || (media ?? kind), kind, detectedAt: now,
    ...(mime ? { mime } : {}), ...(media ? { media } : {}),
    ...(!media && size !== undefined && Number.isSafeInteger(size) && size >= 0 ? { size } : {}),
  };
}

export function mergeCandidates(previous: readonly MediaCandidate[], incoming: MediaCandidate | undefined, max: number, ageMs: number, now: number): MediaCandidate[] {
  const byUrl = new Map(previous.filter((item) => now - item.detectedAt < ageMs).map((item) => [item.url, item]));
  if (incoming) byUrl.set(incoming.url, { ...byUrl.get(incoming.url), ...incoming });
  return [...byUrl.values()].sort((a, b) => b.detectedAt - a.detectedAt).slice(0, max);
}
