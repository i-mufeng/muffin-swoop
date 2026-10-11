import { browser } from "./browser";
import type { PageReply } from "./page-tasks";
import { $ } from "./dom";
import { ago, bytes } from "./format";
import { localizeDocument, translate, translateError, type Language } from "./i18n";
import { loadSettings, loadState, saveSettings, saveState, type RuntimeState, type Settings } from "./settings";

let language: Language = "zh-CN";
let collectError = "";
let refreshRevision = 0;
function render(settings: Settings, state: RuntimeState): void {
  language = settings.language;
  const t = (key: string, values?: Record<string, string | number>) => translate(key, language, values);
  localizeDocument(document, language);
  $("collect-error").textContent = translateError(collectError, language);
  $<HTMLInputElement>("enabled").checked = settings.enabled;

  const dot = $("dot");
  dot.classList.toggle("on", settings.enabled && state.connected);
  if (!settings.enabled) {
    $("status").textContent = t("已关闭接管");
    $("status-sub").textContent = t("开启后可提交下载");
  } else if (state.connected) {
    $("status").textContent = t("已连接 Swoop");
    $("status-sub").textContent = `127.0.0.1:${settings.port}`;
  } else {
    $("status").textContent = t("未连接 Swoop");
    $("status-sub").textContent = t("请先启动 Swoop；未连接时下载照常由浏览器完成");
  }

  $<HTMLButtonElement>("collect-page").disabled = !settings.enabled;
  $<HTMLButtonElement>("open-media").disabled = !settings.enabled || !settings.detectMedia;

  const r = state.lastResult;
  const box = $("result");
  box.hidden = !r;
  if (!r) return;
  box.classList.toggle("ok", r.ok);
  box.classList.toggle("err", !r.ok);
  $("result-title").textContent = t(r.ok ? "已交给 Swoop · {time}" : "交接失败 · {time}", { time: ago(r.at, Date.now(), language) });
  $("result-name").textContent = r.fileName || r.url;
  $("result-name").title = r.url;
  const error = translateError(r.error ?? "未知错误", language);
  $("result-detail").textContent = r.ok ? r.size != null ? bytes(r.size) : ""
    : r.fellBack ? t("{error}，已由浏览器继续下载", { error }) : error;
}

async function refresh(): Promise<void> {
  const revision = ++refreshRevision;
  const [settings, state] = await Promise.all([loadSettings(), loadState()]);
  if (revision !== refreshRevision) return;
  render(settings, state);
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  if (tab?.id !== undefined) {
    const result = await browser.runtime.sendMessage({ type: "media-count", tabId: tab.id }).catch(() => undefined) as { ok?: boolean; count?: number } | undefined;
    if (revision !== refreshRevision) return;
    $("open-media").textContent = result?.ok && result.count
      ? translate("网页媒体（{count}）…", settings.language, { count: result.count }) : translate("网页媒体…", settings.language);
  }
}

$("enabled").addEventListener("change", async (e) => {
  await saveSettings({ enabled: (e.target as HTMLInputElement).checked });
  await refresh();
});
$("open-options").addEventListener("click", () => { void browser.runtime.openOptionsPage(); });
browser.storage.onChanged.addListener((_changes, area) => {
  if (area === "sync" || area === "session") void refresh();
});

void (async () => {
  await refresh();
  // 打开 popup 即视为看过了失败原因：清掉徽标上的 !
  await saveState({ unseenFailure: false });
  await browser.runtime.sendMessage({ type: "refresh" }).catch(() => {});
  await refresh();
})().catch(() => { $("collect-error").textContent = translate("无法读取扩展设置", language); });

async function openPage(type: "collect-page" | "open-media", buttonId: string, fallback: string): Promise<void> {
  $<HTMLButtonElement>(buttonId).disabled = true;
  try {
    const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
    if (tab?.id === undefined) throw new Error("未找到当前网页");
    const reply = await browser.runtime.sendMessage({ type, tabId: tab.id }) as PageReply;
    if (!reply?.ok) throw new Error(reply?.error ?? fallback);
    window.close();
  } catch (error) {
    collectError = error instanceof Error ? error.message : fallback;
    $("collect-error").textContent = translateError(collectError, language);
  } finally { await refresh(); }
}
$("collect-page").addEventListener("click", () => { void openPage("collect-page", "collect-page", "未能读取网页链接"); });
$("open-media").addEventListener("click", () => { void openPage("open-media", "open-media", "未能读取网页媒体"); });
