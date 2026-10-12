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

export interface PageLink {
  url: string;
  title: string;
  extension: string;
  referrerPolicy?: string;
  mime?: string;
  size?: number;
  media?: "hls" | "dash";
}

/** 页面链接去重；片段不发送到服务器，保留查询参数与顺序。 */
export function normalizePageLinks(raw: readonly { url: string; title: string; referrerPolicy?: string }[], pageUrl: string, limit: number): { links: PageLink[]; truncated: boolean } {
  const seen = new Set<string>();
  const links: PageLink[] = [];
  let current: string;
  try { const page = new URL(pageUrl); page.hash = ""; current = page.href; } catch { current = pageUrl; }
  for (const candidate of raw) {
    let parsed: URL;
    try { parsed = new URL(candidate.url, pageUrl); } catch { continue; }
    parsed.hash = "";
    if (manualUrlBlock(parsed, candidate.url)) continue;
    if (parsed.href === current || seen.has(parsed.href)) continue;
    seen.add(parsed.href);
    if (links.length >= limit) return { links, truncated: true };
    let name = parsed.pathname.split("/").pop() ?? "";
    try { name = decodeURIComponent(name); } catch { /* 保留编码不完整的文件名 */ }
    const extension = /\.([a-z0-9]{1,12})$/i.exec(name)?.[1]?.toLowerCase() ?? "";
    links.push({ url: parsed.href, title: candidate.title.trim().slice(0, 300) || name || parsed.hostname, extension,
      ...(candidate.referrerPolicy ? { referrerPolicy: candidate.referrerPolicy } : {}),
    });
  }
  return { links, truncated: false };
}

export function filterPageLinks(links: readonly PageLink[], query: string, extension: string): PageLink[] {
  const needle = query.trim().toLocaleLowerCase();
  return links.filter((link) =>
    (extension === "*" || (extension === "-" ? !link.extension : link.extension === extension)) &&
    (!needle || `${link.title}\n${link.url}`.toLocaleLowerCase().includes(needle)),
  );
}

/** 手动下载遵守总开关和域名排除；页面未提供文件大小，不猜测大小阈值。 */
export function manualDownloadBlock(url: string, settings: Settings): string | undefined {
  if (!settings.enabled) return "接管已关闭，请先在扩展弹窗中开启";
  let parsed: URL;
  try { parsed = new URL(url); } catch { return "无效链接"; }
  const blocked = manualUrlBlock(parsed, url);
  if (blocked) return blocked;
  if (settings.excludedDomains.some((domain) => matchesDomain(parsed.hostname, domain))) return "此域名已在扩展设置中排除";
  return undefined;
}

function manualUrlBlock(parsed: URL, original: string): string | undefined {
  if (/^https?:$/.test(parsed.protocol)) return undefined;
  if (!/^ftps?:$/.test(parsed.protocol)) return "只支持 HTTP(S) / FTP(S) 链接";
  if (parsed.username || parsed.password || /^ftps?:\/\/[^/?#]*@/i.test(original.trimStart())) {
    return "FTP 账号请在 Swoop 中填写，不要放在链接中";
  }
  if (!parsed.hostname || !parsed.pathname || parsed.pathname.endsWith("/") || parsed.search || parsed.hash || /;type=/i.test(parsed.pathname)) {
    return "请输入 FTP 文件链接";
  }
  try {
    if (/[\u0000-\u001f\u007f-\u009f]/u.test(decodeURIComponent(parsed.pathname))) return "无效 FTP 文件链接";
  } catch { return "无效 FTP 文件链接"; }
  return undefined;
}

/** 首次安装、或主/次版本号变化（0.x 阶段次版本即"大版本"）时打开欢迎页；补丁更新与浏览器更新不打扰。 */
export function shouldOpenWelcome(reason: string, previousVersion: string | undefined, currentVersion: string): boolean {
  if (reason === "install") return true;
  if (reason !== "update" || !previousVersion) return false;
  const featureLine = (version: string) => version.split(".").slice(0, 2).map((part) => Number.parseInt(part, 10) || 0).join(".");
  return featureLine(previousVersion) !== featureLine(currentVersion);
}
