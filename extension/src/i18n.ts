export type Language = "zh-CN" | "en-US";

// UI 语言由扩展设置决定；协议字段、网页内容和持久化错误原文不随语言变化。
export const ENGLISH: Readonly<Record<string, string>> = {
  "Swoop 扩展设置": "Swoop extension settings",
  "语言": "Language",
  "设置": "Settings",
  "端口": "Port",
  "测试连接": "Test connection",
  "需与 Swoop「设置 → 浏览器集成」里的端口一致，默认 51230。": "Match the port in Swoop Settings → Browser integration. Default: 51230.",
  "不接管的域名": "Excluded domains",
  "每行一个，自动包含子域名。这些网站的下载始终交给浏览器。": "One domain per line, including subdomains. Downloads from these sites stay in the browser.",
  "大小阈值（MB）": "Minimum size (MB)",
  "小于此大小的下载交给浏览器；0 表示不限。大小未知的下载照常接管。": "Smaller downloads stay in the browser; 0 means no limit. Downloads with unknown sizes are still captured.",
  "本页链接 · 同时提交数": "Page links · Concurrent submissions",
  "每页链接上限": "Maximum links per page",
  "检测网页音视频": "Detect page audio and video",
  "每页媒体上限": "Maximum media items per page",
  "保留时间（分钟）": "Retention (minutes)",
  "保存": "Save",
  "已保存": "Saved",
  "保存失败，请重试": "Could not save settings. Try again.",
  "无法读取扩展设置": "Could not load extension settings.",
  "端口需在 1024–65535 之间": "Port must be between 1024 and 65535.",
  "同时提交数需为 1–6，每页链接上限需为 100–10000": "Use 1–6 concurrent submissions and 100–10000 links per page.",
  "媒体上限需为 10–500，保留时间需为 1–1440 分钟": "Use 10–500 media items per page and 1–1440 minutes of retention.",
  "连接中…": "Connecting…",
  "✓ 已连接": "✓ Connected",
  "✗ 连不上：Swoop 未运行或端口不一致": "✗ Cannot connect. Start Swoop and check the port.",
  "检测中…": "Checking…",
  "接管浏览器下载": "Capture browser downloads",
  "关闭后下载完全交给浏览器": "When off, downloads stay in the browser",
  "下载本页链接…": "Download page links…",
  "下载本页链接": "Download page links",
  "下载本页链接 · Swoop": "Page links · Swoop",
  "网页媒体…": "Page media…",
  "网页媒体（{count}）…": "Page media ({count})…",
  "网页媒体": "Page media",
  "网页媒体 · Swoop": "Page media · Swoop",
  "已关闭接管": "Download capture is off",
  "开启后可提交下载": "Enable capture to send downloads",
  "已连接 Swoop": "Connected to Swoop",
  "未连接 Swoop": "Not connected to Swoop",
  "请先启动 Swoop；未连接时下载照常由浏览器完成": "Start Swoop to connect. Downloads still work in the browser.",
  "已交给 Swoop · {time}": "Sent to Swoop · {time}",
  "交接失败 · {time}": "Transfer failed · {time}",
  "未知错误": "Unknown error",
  "{error}，已由浏览器继续下载": "{error} · Download resumed in the browser.",
  "刚刚": "Just now",
  "{count} 分钟前": "{count} min ago",
  "{count} 小时前": "{count} hr ago",
  "{count} 天前": "{count} d ago",
  "Swoop：{error}": "Swoop: {error}",
  "上次交接失败": "Last transfer failed",
  "Swoop：已关闭接管": "Swoop: Capture is off",
  "Swoop：未连接（请先启动 Swoop）": "Swoop: Not connected (start Swoop)",
  "Swoop：已连接": "Swoop: Connected",
  "用 Swoop 下载": "Download with Swoop",
  "用 Swoop 下载本页链接…": "Download page links with Swoop…",
  "正在读取链接…": "Loading links…",
  "筛选名称或网址": "Filter by name or URL",
  "文件类型": "File type",
  "所有类型": "All types",
  "全选当前结果": "Select visible",
  "取消当前选择": "Clear visible selection",
  "选择": "Select",
  "链接": "Link",
  "提交结果": "Submission result",
  "没有符合条件的链接": "No matching links",
  "停止后续提交": "Stop pending submissions",
  "提交所选": "Submit selected",
  "提交所选（{count}）": "Submit selected ({count})",
  "无扩展名": "No extension",
  "显示 {visible} / {total} · 已选 {selected} · 已提交 {successes} · 失败 {failures}": "Showing {visible} / {total} · Selected {selected} · Sent {successes} · Failed {failures}",
  "接管或媒体检测已关闭，请在扩展设置中开启。": "Capture or media detection is off. Enable it in extension settings.",
  "选择媒体后交给 Swoop；流媒体清单会打开媒体窗口。": "Select media to send to Swoop. Streaming playlists open in the media window.",
  "链接较多，列表已截断；可在设置中调整每页上限。": "The list is truncated. Increase the per-page limit in settings to show more.",
  "选择要下载的链接后提交。": "Select the links you want to download, then submit.",
  "隐身窗口不会传递登录信息。": "Private windows do not share login information.",
  "提交中…": "Submitting…",
  "✓ 已交给 Swoop": "✓ Sent to Swoop",
  "待提交": "Ready to submit",
  "待选择": "Not selected",
  "选择 {title}": "Select {title}",
  "正在等待当前提交…": "Waiting for current submissions…",
  "未找到当前网页": "The current tab was not found.",
  "未能读取网页链接": "Could not read page links.",
  "未能读取网页媒体": "Could not read page media.",
  "无法打开网页媒体": "Could not open page media.",
  "链接列表已过期，请返回原网页重新提取": "This link list expired. Collect links from the original page again.",
  "此链接列表已过期，请返回原网页重新提取": "This link list expired. Collect links from the original page again.",
  "扩展未返回提交结果": "The extension did not return a submission result.",
  "提交中断，请先检查 Swoop 任务列表再重试": "Submission was interrupted. Check Swoop's task list before retrying.",
  "无法打开链接列表": "Could not open the link list.",
  "媒体检测暂不可用，请检查扩展权限后重试": "Media detection is unavailable. Check extension permissions and try again.",
  "媒体检测已关闭，请在扩展设置中开启": "Media detection is off. Enable it in extension settings.",
  "尚未发现媒体，请先在网页播放音视频后重试": "No media found yet. Play audio or video on the page, then try again.",
  "无法读取媒体列表": "Could not read the media list.",
  "无法读取媒体": "Could not read media.",
  "提交失败，请先检查 Swoop 任务列表再重试": "Submission failed. Check Swoop's task list before retrying.",
  "请在普通网页上使用此功能": "Use this feature on an HTTP or HTTPS page.",
  "Swoop 未运行，请先启动 Swoop": "Swoop is not running. Start Swoop first.",
  "当前 Swoop 尚不支持媒体接收，请更新后重试": "This version of Swoop cannot receive media. Update Swoop and try again.",
  "链接不在本页列表中": "The link is not in this page's list.",
  "媒体检测已关闭": "Media detection is off.",
  "原网页已变化，请重新提取链接": "The original page changed. Collect its links again.",
  "原网页已关闭，请重新打开后提取链接": "The original page was closed. Reopen it and collect its links again.",
  "接管已关闭，请先在扩展弹窗中开启": "Capture is off. Enable it in the extension popup first.",
  "未能读取网页链接，请刷新网页后重试": "Could not read page links. Refresh the page and try again.",
  "此页面不允许读取链接，请检查扩展的网站访问权限": "Links cannot be read on this page. Check the extension's site access permissions.",
  "无法保存或打开链接列表，请关闭旧列表后重试": "Could not save or open the link list. Close older lists and try again.",
  "无效链接": "Invalid URL",
  "此域名已在扩展设置中排除": "This domain is excluded in extension settings.",
  "只支持 HTTP(S) / FTP(S) 链接": "Only HTTP(S) and FTP(S) URLs are supported.",
  "FTP 账号请在 Swoop 中填写，不要放在链接中": "Enter FTP credentials in Swoop, not in the URL.",
  "请输入 FTP 文件链接": "Enter an FTP file URL.",
  "无效 FTP 文件链接": "Invalid FTP file URL",
  "提交失败": "Submission failed",
  "Swoop 返回了无效结果，请先检查任务列表再重试": "Swoop returned an invalid result. Check its task list before retrying.",
  "Swoop 返回 HTTP {status}": "Swoop returned HTTP {status}",
  "Swoop 响应超时，请先检查任务列表再重试": "Swoop timed out. Check its task list before retrying.",
  "无法连接 Swoop": "Could not connect to Swoop.",
};

export function normalizeLanguage(value: unknown): Language {
  return value === "en-US" ? "en-US" : "zh-CN";
}

export function translate(key: string, language: Language, values: Record<string, string | number> = {}): string {
  const template = language === "en-US" ? ENGLISH[key] ?? key : key;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => String(values[name] ?? match));
}

/** Only our own known messages are localized. Server/website/browser error details remain intact. */
export function translateError(error: string, language: Language): string {
  const http = /^Swoop 返回 HTTP (\d{3})$/.exec(error);
  return http ? translate("Swoop 返回 HTTP {status}", language, { status: http[1]! }) : translate(error, language);
}

export function localizeDocument(doc: Document, language: Language): void {
  doc.documentElement.lang = language;
  for (const node of doc.querySelectorAll<HTMLElement>("[data-i18n]")) {
    node.textContent = translate(node.dataset.i18n!, language);
  }
  for (const attribute of ["placeholder", "aria-label", "title"] as const) {
    for (const node of doc.querySelectorAll<HTMLElement>(`[data-i18n-${attribute}]`)) {
      node.setAttribute(attribute, translate(node.getAttribute(`data-i18n-${attribute}`)!, language));
    }
  }
}
