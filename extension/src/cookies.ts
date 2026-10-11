import { browser } from "./browser";
// 取下载链接的 Cookie（含 HttpOnly），拼成 Cookie 请求头。登录后的下载靠它才能在 Swoop 里成功。

export interface CookieLike {
  name: string;
  value: string;
  path: string;
}

/** 按 RFC 6265：path 更长的排在前面；没有 Cookie 时返回 undefined */
export function formatCookies(list: readonly CookieLike[]): string | undefined {
  if (list.length === 0) return undefined;
  return [...list]
    .sort((a, b) => b.path.length - a.path.length)
    .map((c) => (c.name ? `${c.name}=${c.value}` : c.value))
    .join("; ");
}

export async function cookieHeader(url: string, tabId?: number): Promise<string | undefined> {
  try {
    const stores = tabId === undefined ? [] : await browser.cookies.getAllCookieStores();
    const storeId = stores.find((store) => store.tabIds.includes(tabId!))?.id;
    // 未找到原标签的容器时不混用默认会话。
    if (tabId !== undefined && !storeId) return undefined;
    return formatCookies(await browser.cookies.getAll({ url, ...(storeId ? { storeId } : {}) }));
  } catch {
    return undefined;
  }
}
