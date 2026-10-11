import type { HandoffResult } from "./api";
import type { PageLink } from "./rules";

export type LinkStatus = "ready" | "sending" | "success" | "error";
export interface BatchItem {
  link: PageLink;
  selected: boolean;
  status: LinkStatus;
  result?: HandoffResult;
}

/** 页面交互状态与有界任务池，不依赖浏览器 / DOM。 */
export class LinkBatch {
  readonly items: BatchItem[];
  running = false;
  private stopped = false;

  constructor(links: readonly PageLink[], results: Record<string, HandoffResult> = {}) {
    this.items = links.map((link) => {
      const result = results[link.url];
      return { link, selected: false, status: result ? result.ok ? "success" : "error" : "ready", result };
    });
  }

  select(urls: readonly string[], selected: boolean): void {
    if (this.running) return;
    const visible = new Set(urls);
    for (const item of this.items) {
      if (visible.has(item.link.url) && item.status !== "success") item.selected = selected;
    }
  }

  get pending(): BatchItem[] {
    return this.items.filter((item) => item.selected && item.status !== "success" && item.status !== "sending");
  }

  stop(): void { this.stopped = true; }

  async run(concurrency: number, send: (link: PageLink) => Promise<HandoffResult>, changed: () => void = () => {}): Promise<void> {
    if (this.running) return;
    const queue = this.pending;
    if (!queue.length) return;
    this.running = true;
    this.stopped = false;
    let next = 0;
    const count = Number.isFinite(concurrency) ? Math.min(6, Math.max(1, Math.floor(concurrency))) : 1;
    const worker = async () => {
      while (!this.stopped) {
        const item = queue[next++];
        if (!item) return;
        item.status = "sending";
        item.result = undefined;
        changed();
        try {
          item.result = await send(item.link);
        } catch (error) {
          item.result = { ok: false, error: error instanceof Error ? error.message : "提交失败" };
        }
        item.status = item.result.ok ? "success" : "error";
        if (item.result.ok) item.selected = false;
        changed();
      }
    };
    changed();
    try { await Promise.all(Array.from({ length: Math.min(count, queue.length) }, worker)); }
    finally { this.running = false; changed(); }
  }
}
