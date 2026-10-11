/** 通过 scripting.executeScript 注入：函数内不能引用模块变量或导入。 */
export function collectDocumentLinks(scanLimit: number): {
  pageUrl: string;
  title: string;
  links: { url: string; title: string; referrerPolicy?: string }[];
  truncated: boolean;
} {
  const anchors = document.querySelectorAll<HTMLAnchorElement | HTMLAreaElement>("a[href], area[href]");
  const links: { url: string; title: string; referrerPolicy?: string }[] = [];
  let scanned = 0;
  for (const anchor of anchors) {
    if (scanned >= scanLimit) break;
    scanned++;
    let url: string;
    try { url = new URL(anchor.getAttribute("href") ?? "", document.baseURI).href; } catch { continue; }
    const pagePolicy = document.querySelector<HTMLMetaElement>('meta[name="referrer" i]')?.content;
    const referrerPolicy = anchor.relList.contains("noreferrer") ? "no-referrer" : anchor.referrerPolicy || pagePolicy;
    links.push({ url, title: (anchor.textContent || anchor.getAttribute("title") || anchor.getAttribute("alt") || "").trim().slice(0, 300),
      ...(referrerPolicy ? { referrerPolicy } : {}),
    });
  }
  return { pageUrl: location.href, title: document.title, links, truncated: anchors.length > scanned };
}
