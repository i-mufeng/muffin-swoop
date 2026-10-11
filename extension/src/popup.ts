import { $ } from "./dom";
import { ago, bytes } from "./format";
import { loadSettings, loadState, saveSettings, saveState, type RuntimeState, type Settings } from "./settings";

function render(settings: Settings, state: RuntimeState): void {
  $<HTMLInputElement>("enabled").checked = settings.enabled;

  const dot = $("dot");
  dot.classList.toggle("on", settings.enabled && state.connected);
  if (!settings.enabled) {
    $("status").textContent = "已关闭接管";
    $("status-sub").textContent = "右键菜单「用 Swoop 下载」仍可用";
  } else if (state.connected) {
    $("status").textContent = "已连接 Swoop";
    $("status-sub").textContent = `127.0.0.1:${settings.port}`;
  } else {
    $("status").textContent = "未连接 Swoop";
    $("status-sub").textContent = "请先启动 Swoop；未连接时下载照常由浏览器完成";
  }

  const r = state.lastResult;
  const box = $("result");
  box.hidden = !r;
  if (!r) return;
  box.classList.toggle("ok", r.ok);
  box.classList.toggle("err", !r.ok);
  $("result-title").textContent = r.ok ? `已交给 Swoop · ${ago(r.at)}` : `交接失败 · ${ago(r.at)}`;
  $("result-name").textContent = r.fileName || r.url;
  $("result-name").title = r.url;
  $("result-detail").textContent = r.ok
    ? r.size != null
      ? bytes(r.size)
      : ""
    : `${r.error ?? "未知错误"}${r.fellBack ? "，已由浏览器继续下载" : ""}`;
}

async function refresh(): Promise<void> {
  render(await loadSettings(), await loadState());
}

$("enabled").addEventListener("change", async (e) => {
  await saveSettings({ enabled: (e.target as HTMLInputElement).checked });
  await refresh();
});
$("open-options").addEventListener("click", () => {
  void chrome.runtime.openOptionsPage();
});

void (async () => {
  await refresh();
  // 打开 popup 即视为看过了失败原因：清掉徽标上的 !
  await saveState({ unseenFailure: false });
  await chrome.runtime.sendMessage({ type: "refresh" }).catch(() => {});
  await refresh();
})();
