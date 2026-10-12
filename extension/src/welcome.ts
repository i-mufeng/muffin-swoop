import { browser } from "./browser";
import { $ } from "./dom";
import { localizeDocument, normalizeLanguage, translate } from "./i18n";
import { DEFAULT_SETTINGS, loadSettings, saveSettings, type Settings } from "./settings";

// 欢迎页：安装或大版本更新时由后台打开，也可从弹窗「使用指南」进入。
// 连接检测走后台的 refresh 消息，顺带刷新工具栏徽标。

const POLL_MS = 3000;
type Connection = "checking" | "connected" | "offline" | "disabled";

let settings: Settings = DEFAULT_SETTINGS;
let connection: Connection = "checking";
let checking = false;
/** 检测进行中又有设置变化：结束后再测一轮，不丢掉这次变化。 */
let recheck = false;

function render(): void {
  const t = (key: string, values?: Record<string, string | number>) => translate(key, settings.language, values);
  localizeDocument(document, settings.language);
  $<HTMLSelectElement>("language").value = settings.language;
  $("version").textContent = `v${browser.runtime.getManifest().version}`;

  $("connect").classList.toggle("ok", connection === "connected");
  $("dot").className = `dot ${{ checking: "wait", connected: "on", offline: "err", disabled: "" }[connection]}`;
  $("status").textContent = {
    checking: t("检测中…"),
    connected: t("✓ 已连接 Swoop，可以开始下载了"),
    offline: t("还没连上 Swoop"),
    disabled: t("接管已关闭，请先在扩展弹窗中开启"),
  }[connection];
  $("help").hidden = connection !== "offline";
  $("port-hint").textContent = t("扩展端口为 {port}，需与 Swoop 中设置的端口一致；不一致请到扩展设置修改。", { port: settings.port });
  $<HTMLButtonElement>("retry").hidden = connection === "connected";
  $<HTMLButtonElement>("retry").disabled = connection === "checking";
}

async function check(showProgress: boolean): Promise<void> {
  if (checking) { recheck = true; return; }
  checking = true;
  recheck = false;
  try {
    settings = await loadSettings();
    if (!settings.enabled) { connection = "disabled"; return; }
    if (showProgress) { connection = "checking"; render(); }
    const connected = await browser.runtime.sendMessage({ type: "refresh" }).catch(() => false) as boolean;
    connection = connected ? "connected" : "offline";
  } finally {
    checking = false;
    render();
  }
  if (recheck) await check(false);
}

$<HTMLSelectElement>("language").addEventListener("change", (event) => {
  settings = { ...settings, language: normalizeLanguage((event.target as HTMLSelectElement).value) };
  render();
  void saveSettings({ language: settings.language });
});
$("open-options").addEventListener("click", () => { void browser.runtime.openOptionsPage(); });
$("retry").addEventListener("click", () => { void check(true); });
browser.storage.onChanged.addListener((_changes, area) => {
  if (area === "sync") void check(false);
});
// 用户切去启动 Swoop 再回来时立刻检测；后台标签页不轮询。
document.addEventListener("visibilitychange", () => { if (!document.hidden) void check(false); });
setInterval(() => { if (!document.hidden && connection !== "connected") void check(false); }, POLL_MS);

render();
void check(true);
