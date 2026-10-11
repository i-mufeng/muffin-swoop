import type { HandoffResult } from "./api";
import { LinkBatch } from "./batch";
import type { PageBatch } from "./page-tasks";
import { filterPageLinks, type PageLink } from "./rules";
import type { Settings } from "./settings";
import { localizeDocument, translate, translateError } from "./i18n";

export function mountLinkPicker(doc: Document, page: PageBatch, initialSettings: Settings, send: (link: PageLink) => Promise<HandoffResult>) {
  const get = <T extends HTMLElement>(id: string): T => {
    const element = doc.getElementById(id);
    if (!element) throw new Error(`缺少 ${id}`);
    return element as T;
  };
  const batch = new LinkBatch(page.links, page.results);
  let settings = initialSettings;
  let stopping = false;
  const t = (key: string, values?: Record<string, string | number>) => translate(key, settings.language, values);
  const query = get<HTMLInputElement>("query");
  const extension = get<HTMLSelectElement>("extension");
  const submit = get<HTMLButtonElement>("submit");
  const stop = get<HTMLButtonElement>("stop");
  const select = get<HTMLButtonElement>("select-visible");
  const clear = get<HTMLButtonElement>("clear-visible");
  const list = get<HTMLTableSectionElement>("links");
  const source = get<HTMLElement>("source");
  source.textContent = page.title || page.pageUrl;
  source.title = page.pageUrl;
  const byUrl = new Map(batch.items.map((item) => [item.link.url, item]));
  const types = [...new Set(page.links.map((link) => link.extension))].sort();
  for (const value of types) {
    const option = doc.createElement("option");
    option.value = value || "-";
    option.textContent = value ? `.${value}` : t("无扩展名");
    extension.append(option);
  }
  const visibleLinks = () => filterPageLinks(page.links, query.value, extension.value);

  function update(): void {
    localizeDocument(doc, settings.language);
    if (page.kind === "media") {
      doc.title = t("网页媒体 · Swoop");
      doc.querySelector("h1")!.textContent = t("网页媒体");
    }
    const noExtension = extension.querySelector<HTMLOptionElement>('option[value="-"]');
    if (noExtension) noExtension.textContent = t("无扩展名");
    const selected = batch.pending.length;
    const successes = batch.items.filter((item) => item.status === "success").length;
    const failures = batch.items.filter((item) => item.status === "error").length;
    get<HTMLElement>("summary").textContent = t("显示 {visible} / {total} · 已选 {selected} · 已提交 {successes} · 失败 {failures}",
      { visible: visibleLinks().length, total: page.links.length, selected, successes, failures });
    const enabled = settings.enabled && (page.kind !== "media" || settings.detectMedia);
    get<HTMLElement>("notice").textContent = !enabled ? t("接管或媒体检测已关闭，请在扩展设置中开启。")
      : page.kind === "media" ? t("选择媒体后交给 Swoop；流媒体清单会打开媒体窗口。")
      : [t(page.truncated ? "链接较多，列表已截断；可在设置中调整每页上限。" : "选择要下载的链接后提交。"),
         page.incognito ? t("隐身窗口不会传递登录信息。") : ""].filter(Boolean).join(" ");
    submit.disabled = batch.running || selected === 0 || !enabled;
    submit.textContent = batch.running ? t("提交中…") : selected ? t("提交所选（{count}）", { count: selected }) : t("提交所选");
    stop.hidden = !batch.running;
    if (!batch.running) stopping = false;
    stop.disabled = stopping;
    stop.textContent = t(stopping ? "正在等待当前提交…" : "停止后续提交");
    select.disabled = clear.disabled = batch.running;
    for (const row of list.querySelectorAll<HTMLTableRowElement>("tr")) {
      const item = byUrl.get(row.dataset.url ?? "");
      if (!item) continue;
      row.dataset.status = item.status;
      const checkbox = row.querySelector<HTMLInputElement>("input")!;
      checkbox.checked = item.selected;
      checkbox.disabled = batch.running || item.status === "success";
      checkbox.setAttribute("aria-label", t("选择 {title}", { title: item.link.title }));
      const label = row.querySelector<HTMLElement>(".result")!;
      label.textContent = item.status === "sending" ? t("提交中…") : item.result?.ok
        ? `${t("✓ 已交给 Swoop")}${item.result.fileName ? ` · ${item.result.fileName}` : ""}`
        : item.result ? `✗ ${translateError(item.result.error, settings.language)}` : t(item.selected ? "待提交" : "待选择");
    }
  }

  function render(): void {
    list.replaceChildren();
    for (const link of visibleLinks()) {
      const row = doc.createElement("tr");
      row.dataset.url = link.url;
      const checkCell = doc.createElement("td");
      const checkbox = doc.createElement("input");
      checkbox.type = "checkbox";
      checkbox.id = `link-${page.links.indexOf(link)}`;
      checkbox.setAttribute("aria-label", t("选择 {title}", { title: link.title }));
      checkbox.addEventListener("change", () => { batch.select([link.url], checkbox.checked); update(); });
      checkCell.append(checkbox);
      const textCell = doc.createElement("td");
      const label = doc.createElement("label");
      label.htmlFor = checkbox.id;
      const title = doc.createElement("span");
      title.className = "link-name";
      title.textContent = link.title;
      const url = doc.createElement("span");
      url.className = "link-url";
      url.textContent = link.url;
      label.append(title, url);
      textCell.append(label);
      const result = doc.createElement("td");
      result.className = "result";
      row.append(checkCell, textCell, result);
      list.append(row);
    }
    get<HTMLElement>("empty").hidden = list.childElementCount !== 0;
    update();
  }

  query.addEventListener("input", render);
  extension.addEventListener("change", render);
  select.addEventListener("click", () => { batch.select(visibleLinks().map((link) => link.url), true); update(); });
  clear.addEventListener("click", () => { batch.select(visibleLinks().map((link) => link.url), false); update(); });
  submit.addEventListener("click", () => {
    if (settings.enabled && (page.kind !== "media" || settings.detectMedia)) void batch.run(settings.batchConcurrency, send, update);
  });
  stop.addEventListener("click", () => { batch.stop(); stopping = true; update(); });
  render();
  return {
    batch,
    updateSettings(next: Settings) {
      settings = next;
      if (!next.enabled || (page.kind === "media" && !next.detectMedia)) { batch.stop(); stopping = batch.running; }
      update();
    },
  };
}
