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
