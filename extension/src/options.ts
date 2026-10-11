import { ping } from "./api";
import { $ } from "./dom";
import { parseDomainList } from "./rules";
import { DEFAULT_SETTINGS, loadSettings, normalizeSettings, saveSettings } from "./settings";

const port = $<HTMLInputElement>("port");
const excluded = $<HTMLTextAreaElement>("excluded");
const minSize = $<HTMLInputElement>("min-size");
const msg = $("msg");

function say(text: string, kind: "ok" | "err" | "" = ""): void {
  msg.textContent = text;
  msg.className = `hint ${kind}`;
}

async function load(): Promise<void> {
  const s = await loadSettings();
  port.value = String(s.port);
  excluded.value = s.excludedDomains.join("\n");
  minSize.value = String(s.minSizeMB);
}

$("save").addEventListener("click", async () => {
  const p = Number(port.value);
  if (!Number.isInteger(p) || p < 1024 || p > 65535) {
    say("端口需在 1024–65535 之间", "err");
    return;
  }
  const s = normalizeSettings({
    port: p,
    excludedDomains: parseDomainList(excluded.value),
    minSizeMB: Number(minSize.value),
  });
  await saveSettings({ port: s.port, excludedDomains: s.excludedDomains, minSizeMB: s.minSizeMB });
  await load();
  say("已保存", "ok");
});

$("test").addEventListener("click", async () => {
  const out = $("test-result");
  const p = Number(port.value) || DEFAULT_SETTINGS.port;
  out.textContent = "连接中…";
  out.textContent = (await ping(p)) ? "✓ 已连接" : "✗ 连不上：Swoop 未运行或端口不一致";
});

void load();
