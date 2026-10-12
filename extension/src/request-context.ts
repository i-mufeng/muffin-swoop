/** 手动提交遵守 strict-origin-when-cross-origin，移除凭据和片段。 */
export function safeReferrer(target: string, source?: string, policy?: string): string | undefined {
  if (!source || policy === "no-referrer") return undefined;
  try {
    const to = new URL(target);
    const from = new URL(source);
    if (!/^https?:$/.test(from.protocol) || !/^https?:$/.test(to.protocol)) return undefined;
    if (from.protocol === "https:" && to.protocol !== "https:") return undefined;
    from.username = from.password = from.hash = "";
    if (policy === "same-origin" && from.origin !== to.origin) return undefined;
    if (policy === "origin" || policy === "strict-origin") return `${from.origin}/`;
    return from.origin === to.origin ? from.href : `${from.origin}/`;
  } catch { return undefined; }
}

/** 只生成预期的上下文头；非法控制字符不能进入持久化请求。 */
export function contextHeaders(userAgent: string, cookie?: string): [string, string][] {
  const valid = (value: string) => !/[\u0000-\u001f\u007f]/.test(value);
  const headers: [string, string][] = [];
  if (userAgent && valid(userAgent)) headers.push(["User-Agent", userAgent]);
  if (cookie && valid(cookie)) headers.push(["Cookie", cookie]);
  return headers;
}

/** 浏览器下载跳到了别的源（如登录后从 download.* 跳到 edelivery.*）时返回跳转目标；同源或非 HTTP(S) 返回 undefined。 */
export function redirectTarget(url: string, finalUrl?: string): string | undefined {
  if (!finalUrl) return undefined;
  try {
    const from = new URL(url);
    const to = new URL(finalUrl);
    if (!/^https?:$/.test(to.protocol) || from.origin === to.origin) return undefined;
    return to.href;
  } catch { return undefined; }
}
