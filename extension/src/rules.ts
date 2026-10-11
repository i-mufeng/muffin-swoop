// 纯函数：一次浏览器下载要不要交给 Swoop。保持规则少而可预期。

import type { Settings } from "./settings";

/** chrome.downloads.DownloadItem 里用到的字段 */
export interface DownloadLike {
  url: string;
  /** 已知的文件大小；未知时浏览器给 -1 或 0 */
  fileSize: number;
  totalBytes: number;
  incognito: boolean;
}

export type Decision = { take: true } | { take: false; reason: string };

const skip = (reason: string): Decision => ({ take: false, reason });

export function shouldIntercept(item: DownloadLike, s: Settings): Decision {
  if (!s.enabled) return skip("已关闭接管");
  // 隐身窗口的 Cookie 和历史不应写进 Swoop 的数据库
  if (item.incognito) return skip("隐身窗口的下载不接管");
  let url: URL;
  try {
    url = new URL(item.url);
  } catch {
    return skip("无效链接");
  }
  // blob: / data: / file: 等只有浏览器自己能取到
  if (url.protocol !== "http:" && url.protocol !== "https:") return skip(`不接管 ${url.protocol} 链接`);
  if (s.excludedDomains.some((d) => matchesDomain(url.hostname, d))) return skip("域名在排除列表中");
  const size = knownSize(item);
  if (s.minSizeMB > 0 && size !== undefined && size < s.minSizeMB * 1024 * 1024) return skip("小于大小阈值");
  return { take: true };
}

export function knownSize(item: Pick<DownloadLike, "fileSize" | "totalBytes">): number | undefined {
  const n = item.fileSize > 0 ? item.fileSize : item.totalBytes;
  return n > 0 ? n : undefined;
}

/** `example.com` 同时匹配自身和所有子域名 */
export function matchesDomain(host: string, pattern: string): boolean {
  const h = host.toLowerCase();
  const p = pattern.toLowerCase();
  return h === p || h.endsWith(`.${p}`);
}

/** 设置页的排除域名文本 → 规整的域名列表。容忍粘贴整条链接、`*.` 前缀、端口和中英文分隔符 */
export function parseDomainList(text: string): string[] {
  const out: string[] = [];
  for (const token of text.split(/[\s,，;；]+/)) {
    const d = token
      .trim()
      .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
      .replace(/[/?#].*$/, "")
      .replace(/:\d+$/, "")
      .replace(/^(\*\.|\.)+/, "")
      .toLowerCase();
    if (d && !out.includes(d)) out.push(d);
  }
  return out;
}
