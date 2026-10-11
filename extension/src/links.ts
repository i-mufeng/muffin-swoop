import { browser } from "./browser";
import type { HandoffResult } from "./api";
import { $ } from "./dom";
import { localizeDocument, translateError } from "./i18n";
import { mountLinkPicker } from "./links-view";
import { loadPageBatch } from "./page-tasks";
import { DEFAULT_SETTINGS, loadSettings } from "./settings";

$("open-options").addEventListener("click", () => { void browser.runtime.openOptionsPage(); });
let settings = DEFAULT_SETTINGS;
let errorMessage = "";
let view: ReturnType<typeof mountLinkPicker> | undefined;
browser.storage.onChanged.addListener((_changes, area) => {
  if (area === "sync") void loadSettings().then((next) => {
    settings = next;
    if (view) view.updateSettings(next);
    else {
      localizeDocument(document, next.language);
      $("notice").textContent = translateError(errorMessage, next.language);
    }
  });
});
void (async () => {
  try {
    settings = await loadSettings();
    localizeDocument(document, settings.language);
    const id = new URLSearchParams(location.search).get("batch");
    const page = id ? await loadPageBatch(id) : undefined;
    if (!page) throw new Error("链接列表已过期，请返回原网页重新提取");
    view = mountLinkPicker(document, page, settings, async (link) => {
      try {
        const result = await browser.runtime.sendMessage({ type: "submit-page-link", batchId: id, url: link.url }) as HandoffResult;
        if (!result || typeof result.ok !== "boolean") throw new Error("扩展未返回提交结果");
        return result;
      } catch {
        return { ok: false, error: "提交中断，请先检查 Swoop 任务列表再重试" };
      }
    });
    // 关闭页面停止未发出的请求；正在提交的项目由后台继续处理。
    window.addEventListener("pagehide", () => view?.batch.stop());
  } catch (error) {
    errorMessage = error instanceof Error ? error.message : "无法打开链接列表";
    $("notice").textContent = translateError(errorMessage, settings.language);
  }
})();
