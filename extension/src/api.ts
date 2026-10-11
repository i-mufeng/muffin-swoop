// Swoop 本地服务的两个接口：GET /ping 与 POST /tasks。协议见 docs/browser-extension.md。

export interface TaskBody {
  /** 自动接管仅 HTTP(S)；手动入口也支持 FTP(S)，此时 headers 为空且不传 referrer。 */
  url: string;
  referrer?: string;
  headers: [string, string][];
  /** 浏览器看到的 MIME 与大小，Swoop 用来核对重新探测的是不是同一个文件 */
  mime?: string;
  size?: number;
  /** 清单候选交给桌面媒体窗口，不作为普通文件下载。 */
  media?: "hls" | "dash";
}

export type HandoffResult = { ok: true; fileName: string; size: number | null } | { ok: false; error: string };

const base = (port: number) => `http://127.0.0.1:${port}`;

async function fetchWithTimeout(url: string, init: RequestInit, ms: number): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), ms);
  try {
    return await fetch(url, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** Swoop 是否在线。本机连接被拒是瞬时的，所以每次接管前都可以现问 */
export async function ping(port: number, ms = 1500): Promise<boolean> {
  try {
    const resp = await fetchWithTimeout(`${base(port)}/ping`, {}, ms);
    if (!resp.ok) return false;
    const body = (await resp.json()) as { app?: string };
    return body.app === "swoop";
  } catch {
    return false;
  }
}

/** 旧桌面会忽略未知 JSON 字段，先确认能力以免误下载清单文本。 */
export async function supportsMedia(port: number): Promise<boolean> {
  try {
    const response = await fetchWithTimeout(`${base(port)}/ping`, {}, 1500);
    if (!response.ok) return false;
    const body = await response.json() as { app?: string; media_handoff?: boolean };
    return body.app === "swoop" && body.media_handoff === true;
  } catch { return false; }
}

/** Swoop 那边先探测（上限 10 秒）再回答，所以超时给足 15 秒 */
export async function sendTask(port: number, body: TaskBody, ms = 15_000): Promise<HandoffResult> {
  try {
    const resp = await fetchWithTimeout(
      `${base(port)}/tasks`,
      { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) },
      ms,
    );
    const json = (await resp.json().catch(() => ({}))) as { file_name?: string; size?: number | null; error?: string };
    if (resp.ok) {
      if (typeof json.file_name !== "string" || !json.file_name || !(json.size == null || (typeof json.size === "number" && Number.isFinite(json.size) && json.size >= 0))) {
        return { ok: false, error: "Swoop 返回了无效结果，请先检查任务列表再重试" };
      }
      return { ok: true, fileName: json.file_name, size: json.size ?? null };
    }
    return { ok: false, error: json.error ?? `Swoop 返回 HTTP ${resp.status}` };
  } catch (e) {
    return { ok: false, error: e instanceof DOMException && e.name === "AbortError" ? "Swoop 响应超时，请先检查任务列表再重试" : "无法连接 Swoop" };
  }
}
