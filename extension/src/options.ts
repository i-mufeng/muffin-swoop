import { browser } from "./browser";
import { ping } from "./api";
import { $ } from "./dom";
import { localizeDocument, normalizeLanguage, translate, type Language } from "./i18n";
import { parseDomainList } from "./rules";
import { DEFAULT_SETTINGS, loadSettings, normalizeSettings, saveSettings } from "./settings";

const language = $<HTMLSelectElement>("language");
const port = $<HTMLInputElement>("port");
const excluded = $<HTMLTextAreaElement>("excluded");
const minSize = $<HTMLInputElement>("min-size");
const msg = $("msg");
const batchConcurrency = $<HTMLInputElement>("batch-concurrency");
const maxPageLinks = $<HTMLInputElement>("max-page-links");
const detectMedia = $<HTMLInputElement>("detect-media");
const maxMediaCandidates = $<HTMLInputElement>("max-media-candidates");
const mediaRetention = $<HTMLInputElement>("media-retention");
let currentLanguage: Language = "zh-CN";
let status = "";
let connection = "";

function renderLanguage(): void {
  currentLanguage = normalizeLanguage(language.value);
  localizeDocument(document, currentLanguage);
  msg.textContent = translate(status, currentLanguage);
  $("test-result").textContent = translate(connection, currentLanguage);
}
function say(text: string, kind: "ok" | "err" | "" = ""): void {
  status = text;
  msg.className = `hint ${kind}`;
  renderLanguage();
}

async function load(): Promise<void> {
  const s = await loadSettings();
  language.value = s.language;
  port.value = String(s.port);
  excluded.value = s.excludedDomains.join("\n");
  minSize.value = String(s.minSizeMB);
  batchConcurrency.value = String(s.batchConcurrency);
  maxPageLinks.value = String(s.maxPageLinks);
  detectMedia.checked = s.detectMedia;
  maxMediaCandidates.value = String(s.maxMediaCandidates);
  mediaRetention.value = String(s.mediaRetentionMinutes);
  renderLanguage();
}

language.addEventListener("change", renderLanguage);
browser.storage.onChanged.addListener((changes, area) => {
  if (area === "sync" && changes.language) {
    language.value = normalizeLanguage(changes.language.newValue);
    renderLanguage();
  }
});
$("save").addEventListener("click", async () => {
  const p = Number(port.value);
  if (!Number.isInteger(p) || p < 1024 || p > 65535) {
    say("端口需在 1024–65535 之间", "err");
    return;
  }
  if (!batchConcurrency.checkValidity() || !maxPageLinks.checkValidity()) {
    say("同时提交数需为 1–6，每页链接上限需为 100–10000", "err");
    return;
  }
  if (!maxMediaCandidates.checkValidity() || !mediaRetention.checkValidity()) {
    say("媒体上限需为 10–500，保留时间需为 1–1440 分钟", "err");
    return;
  }
  const s = normalizeSettings({
    language: language.value,
    port: p,
    excludedDomains: parseDomainList(excluded.value),
    minSizeMB: Number(minSize.value),
    batchConcurrency: Number(batchConcurrency.value),
    maxPageLinks: Number(maxPageLinks.value),
    detectMedia: detectMedia.checked,
    maxMediaCandidates: Number(maxMediaCandidates.value),
    mediaRetentionMinutes: Number(mediaRetention.value),
  });
  const button = $<HTMLButtonElement>("save");
  button.disabled = true;
  try {
    await saveSettings({ language: s.language, port: s.port, excludedDomains: s.excludedDomains, minSizeMB: s.minSizeMB,
      batchConcurrency: s.batchConcurrency, maxPageLinks: s.maxPageLinks, detectMedia: s.detectMedia,
      maxMediaCandidates: s.maxMediaCandidates, mediaRetentionMinutes: s.mediaRetentionMinutes });
    await load();
    say("已保存", "ok");
  } catch { say("保存失败，请重试", "err"); }
  finally { button.disabled = false; }
});

$("test").addEventListener("click", async () => {
  const p = Number(port.value) || DEFAULT_SETTINGS.port;
  connection = "连接中…";
  renderLanguage();
  connection = (await ping(p)) ? "✓ 已连接" : "✗ 连不上：Swoop 未运行或端口不一致";
  renderLanguage();
});

void load().catch(() => say("无法读取扩展设置", "err"));
