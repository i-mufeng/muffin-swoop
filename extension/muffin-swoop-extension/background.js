"use strict";
(() => {
  // src/browser.ts
  var browser = globalThis.browser ?? globalThis.chrome;
  var isFirefox = false;

  // src/api.ts
  var base = (port) => `http://127.0.0.1:${port}`;
  async function fetchWithTimeout(url, init, ms) {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), ms);
    try {
      return await fetch(url, { ...init, signal: ctrl.signal });
    } finally {
      clearTimeout(timer);
    }
  }
  async function ping(port, ms = 1500) {
    try {
      const resp = await fetchWithTimeout(`${base(port)}/ping`, {}, ms);
      if (!resp.ok) return false;
      const body = await resp.json();
      return body.app === "swoop";
    } catch {
      return false;
    }
  }
  async function supportsMedia(port) {
    try {
      const response = await fetchWithTimeout(`${base(port)}/ping`, {}, 1500);
      if (!response.ok) return false;
      const body = await response.json();
      return body.app === "swoop" && body.media_handoff === true;
    } catch {
      return false;
    }
  }
  async function sendTask(port, body, ms = 15e3) {
    try {
      const resp = await fetchWithTimeout(
        `${base(port)}/tasks`,
        { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) },
        ms
      );
      const json = await resp.json().catch(() => ({}));
      if (resp.ok) {
        if (typeof json.file_name !== "string" || !json.file_name || !(json.size == null || typeof json.size === "number" && Number.isFinite(json.size) && json.size >= 0)) {
          return { ok: false, error: "Swoop \u8FD4\u56DE\u4E86\u65E0\u6548\u7ED3\u679C\uFF0C\u8BF7\u5148\u68C0\u67E5\u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5" };
        }
        return { ok: true, fileName: json.file_name, size: json.size ?? null };
      }
      return { ok: false, error: json.error ?? `Swoop \u8FD4\u56DE HTTP ${resp.status}` };
    } catch (e) {
      return { ok: false, error: e instanceof DOMException && e.name === "AbortError" ? "Swoop \u54CD\u5E94\u8D85\u65F6\uFF0C\u8BF7\u5148\u68C0\u67E5\u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5" : "\u65E0\u6CD5\u8FDE\u63A5 Swoop" };
    }
  }

  // src/i18n.ts
  var ENGLISH = {
    "Swoop \u6269\u5C55\u8BBE\u7F6E": "Swoop extension settings",
    "\u4F7F\u7528\u6307\u5357": "Guide",
    "\u6B22\u8FCE\u4F7F\u7528 Swoop": "Welcome to Swoop",
    "\u628A\u6D4F\u89C8\u5668\u91CC\u7684\u4E0B\u8F7D\u4EA4\u7ED9 Swoop \u591A\u8FDE\u63A5\u4E0B\u8F7D\u5668\u3002Swoop \u672A\u8FD0\u884C\u65F6\uFF0C\u6D4F\u89C8\u5668\u7167\u5E38\u4E0B\u8F7D\u3002": "Send browser downloads to the Swoop multi-connection downloader. When Swoop is not running, downloads stay in the browser.",
    "\u4E24\u6B65\u5F00\u59CB": "Get started in two steps",
    "\u5B89\u88C5\u5E76\u542F\u52A8 Swoop \u684C\u9762\u7AEF": "Install and start the Swoop desktop app",
    "\u6269\u5C55\u53EA\u8D1F\u8D23\u628A\u4E0B\u8F7D\u4EA4\u8FC7\u53BB\uFF0C\u771F\u6B63\u4E0B\u8F7D\u7531\u684C\u9762\u7AEF\u5B8C\u6210\u3002\u652F\u6301 macOS \u4E0E Windows\u3002": "The extension only hands downloads over. The desktop app does the downloading. Available for macOS and Windows.",
    "\u4E0B\u8F7D\u684C\u9762\u7AEF": "Get the desktop app",
    "\u8FDE\u63A5 Swoop": "Connect to Swoop",
    "\u91CD\u65B0\u68C0\u6D4B": "Check again",
    "\u2713 \u5DF2\u8FDE\u63A5 Swoop\uFF0C\u53EF\u4EE5\u5F00\u59CB\u4E0B\u8F7D\u4E86": "\u2713 Connected to Swoop. You're ready to download.",
    "\u8FD8\u6CA1\u8FDE\u4E0A Swoop": "Not connected to Swoop yet",
    "\u786E\u8BA4 Swoop \u6B63\u5728\u8FD0\u884C\uFF08\u5173\u95ED\u7A97\u53E3\u540E\u5B83\u4F1A\u7559\u5728\u83DC\u5355\u680F / \u6258\u76D8\uFF09\u3002": "Make sure Swoop is running (it stays in the menu bar or system tray after you close its window).",
    "\u5728 Swoop\u300C\u8BBE\u7F6E \u2192 \u6D4F\u89C8\u5668\u96C6\u6210\u300D\u4E2D\u5F00\u542F\u96C6\u6210\u3002": "Turn on integration in Swoop Settings \u2192 Browser integration.",
    "\u6269\u5C55\u7AEF\u53E3\u4E3A {port}\uFF0C\u9700\u4E0E Swoop \u4E2D\u8BBE\u7F6E\u7684\u7AEF\u53E3\u4E00\u81F4\uFF1B\u4E0D\u4E00\u81F4\u8BF7\u5230\u6269\u5C55\u8BBE\u7F6E\u4FEE\u6539\u3002": "The extension uses port {port}. It must match the port in Swoop; change it in extension settings if needed.",
    "\u53EF\u4EE5\u8FD9\u6837\u7528": "What you can do",
    "\u81EA\u52A8\u63A5\u7BA1\u4E0B\u8F7D": "Automatic download capture",
    "\u6D4F\u89C8\u5668\u5224\u5B9A\u4E3A\u4E0B\u8F7D\u7684\u6587\u4EF6\u4F1A\u4EA4\u7ED9 Swoop\uFF1B\u63A5\u624B\u5931\u8D25\u65F6\u7531\u6D4F\u89C8\u5668\u7EE7\u7EED\uFF0C\u4E0D\u4F1A\u4E22\u4E0B\u8F7D\u3002\u7F51\u9875\u91CC\u6253\u5F00\u7684 PDF\u3001\u56FE\u7247\u3001\u89C6\u9891\u4E0D\u53D7\u5F71\u54CD\u3002": "Files the browser decides to download go to Swoop. If Swoop can't take one, the browser continues, so nothing is lost. PDFs, images and videos opened in a page are not affected.",
    "\u53F3\u952E\u4E0B\u8F7D": "Right-click to download",
    "\u5728\u94FE\u63A5\u3001\u56FE\u7247\u6216\u97F3\u89C6\u9891\u4E0A\u53F3\u952E\uFF0C\u9009\u62E9\u300C\u7528 Swoop \u4E0B\u8F7D\u300D\u3002": "Right-click a link, image, audio or video and choose Download with Swoop.",
    "\u70B9\u5DE5\u5177\u680F\u7684 Swoop \u56FE\u6807 \u2192\u300C\u4E0B\u8F7D\u672C\u9875\u94FE\u63A5\u2026\u300D\uFF0C\u6309\u540D\u79F0\u6216\u7C7B\u578B\u7B5B\u9009\u540E\u6279\u91CF\u63D0\u4EA4\u3002": "Click the Swoop toolbar icon \u2192 Download page links\u2026, filter by name or type, then send them in bulk.",
    "\u5148\u5728\u7F51\u9875\u64AD\u653E\u97F3\u89C6\u9891\uFF0C\u518D\u6253\u5F00\u300C\u7F51\u9875\u5A92\u4F53\u2026\u300D\u9009\u62E9\u8981\u4E0B\u8F7D\u7684\u5185\u5BB9\uFF1BHLS / DASH \u4F1A\u5728 Swoop \u7684\u5A92\u4F53\u7A97\u53E3\u7EE7\u7EED\u3002": "Play audio or video on the page, then open Page media\u2026 to pick what to download. HLS / DASH continue in Swoop's media window.",
    "\u5C0F\u8D34\u58EB": "Tips",
    "\u628A Swoop \u56FA\u5B9A\u5230\u5DE5\u5177\u680F\uFF1A\u70B9\u6D4F\u89C8\u5668\u7684\u6269\u5C55\u56FE\u6807\uFF0C\u518D\u70B9 Swoop \u65C1\u7684\u56FE\u9489\u3002": "Pin Swoop to the toolbar: click the browser's extensions icon, then the pin next to Swoop.",
    "\u67D0\u4E9B\u7F51\u7AD9\u4E0D\u60F3\u4EA4\u7ED9 Swoop\uFF1F\u5728\u8BBE\u7F6E\u4E2D\u6DFB\u52A0\u6392\u9664\u57DF\u540D\uFF0C\u6216\u8BBE\u7F6E\u5927\u5C0F\u9608\u503C\u3002": "Want some sites to stay in the browser? Add excluded domains or a minimum size in settings.",
    "\u968F\u65F6\u53EF\u4EE5\u4ECE\u6269\u5C55\u5F39\u7A97\u7684\u300C\u4F7F\u7528\u6307\u5357\u300D\u56DE\u5230\u8FD9\u4E00\u9875\u3002": "Return to this page anytime from Guide in the extension popup.",
    "\u8BED\u8A00": "Language",
    "\u8BBE\u7F6E": "Settings",
    "\u7AEF\u53E3": "Port",
    "\u6D4B\u8BD5\u8FDE\u63A5": "Test connection",
    "\u9700\u4E0E Swoop\u300C\u8BBE\u7F6E \u2192 \u6D4F\u89C8\u5668\u96C6\u6210\u300D\u91CC\u7684\u7AEF\u53E3\u4E00\u81F4\uFF0C\u9ED8\u8BA4 51230\u3002": "Match the port in Swoop Settings \u2192 Browser integration. Default: 51230.",
    "\u4E0D\u63A5\u7BA1\u7684\u57DF\u540D": "Excluded domains",
    "\u6BCF\u884C\u4E00\u4E2A\uFF0C\u81EA\u52A8\u5305\u542B\u5B50\u57DF\u540D\u3002\u8FD9\u4E9B\u7F51\u7AD9\u7684\u4E0B\u8F7D\u59CB\u7EC8\u4EA4\u7ED9\u6D4F\u89C8\u5668\u3002": "One domain per line, including subdomains. Downloads from these sites stay in the browser.",
    "\u5927\u5C0F\u9608\u503C\uFF08MB\uFF09": "Minimum size (MB)",
    "\u5C0F\u4E8E\u6B64\u5927\u5C0F\u7684\u4E0B\u8F7D\u4EA4\u7ED9\u6D4F\u89C8\u5668\uFF1B0 \u8868\u793A\u4E0D\u9650\u3002\u5927\u5C0F\u672A\u77E5\u7684\u4E0B\u8F7D\u7167\u5E38\u63A5\u7BA1\u3002": "Smaller downloads stay in the browser; 0 means no limit. Downloads with unknown sizes are still captured.",
    "\u672C\u9875\u94FE\u63A5 \xB7 \u540C\u65F6\u63D0\u4EA4\u6570": "Page links \xB7 Concurrent submissions",
    "\u6BCF\u9875\u94FE\u63A5\u4E0A\u9650": "Maximum links per page",
    "\u68C0\u6D4B\u7F51\u9875\u97F3\u89C6\u9891": "Detect page audio and video",
    "\u6BCF\u9875\u5A92\u4F53\u4E0A\u9650": "Maximum media items per page",
    "\u4FDD\u7559\u65F6\u95F4\uFF08\u5206\u949F\uFF09": "Retention (minutes)",
    "\u4FDD\u5B58": "Save",
    "\u5DF2\u4FDD\u5B58": "Saved",
    "\u4FDD\u5B58\u5931\u8D25\uFF0C\u8BF7\u91CD\u8BD5": "Could not save settings. Try again.",
    "\u65E0\u6CD5\u8BFB\u53D6\u6269\u5C55\u8BBE\u7F6E": "Could not load extension settings.",
    "\u7AEF\u53E3\u9700\u5728 1024\u201365535 \u4E4B\u95F4": "Port must be between 1024 and 65535.",
    "\u540C\u65F6\u63D0\u4EA4\u6570\u9700\u4E3A 1\u20136\uFF0C\u6BCF\u9875\u94FE\u63A5\u4E0A\u9650\u9700\u4E3A 100\u201310000": "Use 1\u20136 concurrent submissions and 100\u201310000 links per page.",
    "\u5A92\u4F53\u4E0A\u9650\u9700\u4E3A 10\u2013500\uFF0C\u4FDD\u7559\u65F6\u95F4\u9700\u4E3A 1\u20131440 \u5206\u949F": "Use 10\u2013500 media items per page and 1\u20131440 minutes of retention.",
    "\u8FDE\u63A5\u4E2D\u2026": "Connecting\u2026",
    "\u2713 \u5DF2\u8FDE\u63A5": "\u2713 Connected",
    "\u2717 \u8FDE\u4E0D\u4E0A\uFF1ASwoop \u672A\u8FD0\u884C\u6216\u7AEF\u53E3\u4E0D\u4E00\u81F4": "\u2717 Cannot connect. Start Swoop and check the port.",
    "\u68C0\u6D4B\u4E2D\u2026": "Checking\u2026",
    "\u63A5\u7BA1\u6D4F\u89C8\u5668\u4E0B\u8F7D": "Capture browser downloads",
    "\u5173\u95ED\u540E\u4E0B\u8F7D\u5B8C\u5168\u4EA4\u7ED9\u6D4F\u89C8\u5668": "When off, downloads stay in the browser",
    "\u4E0B\u8F7D\u672C\u9875\u94FE\u63A5\u2026": "Download page links\u2026",
    "\u4E0B\u8F7D\u672C\u9875\u94FE\u63A5": "Download page links",
    "\u4E0B\u8F7D\u672C\u9875\u94FE\u63A5 \xB7 Swoop": "Page links \xB7 Swoop",
    "\u7F51\u9875\u5A92\u4F53\u2026": "Page media\u2026",
    "\u7F51\u9875\u5A92\u4F53\uFF08{count}\uFF09\u2026": "Page media ({count})\u2026",
    "\u7F51\u9875\u5A92\u4F53": "Page media",
    "\u7F51\u9875\u5A92\u4F53 \xB7 Swoop": "Page media \xB7 Swoop",
    "\u5DF2\u5173\u95ED\u63A5\u7BA1": "Download capture is off",
    "\u5F00\u542F\u540E\u53EF\u63D0\u4EA4\u4E0B\u8F7D": "Enable capture to send downloads",
    "\u5DF2\u8FDE\u63A5 Swoop": "Connected to Swoop",
    "\u672A\u8FDE\u63A5 Swoop": "Not connected to Swoop",
    "\u8BF7\u5148\u542F\u52A8 Swoop\uFF1B\u672A\u8FDE\u63A5\u65F6\u4E0B\u8F7D\u7167\u5E38\u7531\u6D4F\u89C8\u5668\u5B8C\u6210": "Start Swoop to connect. Downloads still work in the browser.",
    "\u5DF2\u4EA4\u7ED9 Swoop \xB7 {time}": "Sent to Swoop \xB7 {time}",
    "\u4EA4\u63A5\u5931\u8D25 \xB7 {time}": "Transfer failed \xB7 {time}",
    "\u672A\u77E5\u9519\u8BEF": "Unknown error",
    "{error}\uFF0C\u5DF2\u7531\u6D4F\u89C8\u5668\u7EE7\u7EED\u4E0B\u8F7D": "{error} \xB7 Download resumed in the browser.",
    "\u521A\u521A": "Just now",
    "{count} \u5206\u949F\u524D": "{count} min ago",
    "{count} \u5C0F\u65F6\u524D": "{count} hr ago",
    "{count} \u5929\u524D": "{count} d ago",
    "Swoop\uFF1A{error}": "Swoop: {error}",
    "\u4E0A\u6B21\u4EA4\u63A5\u5931\u8D25": "Last transfer failed",
    "Swoop\uFF1A\u5DF2\u5173\u95ED\u63A5\u7BA1": "Swoop: Capture is off",
    "Swoop\uFF1A\u672A\u8FDE\u63A5\uFF08\u8BF7\u5148\u542F\u52A8 Swoop\uFF09": "Swoop: Not connected (start Swoop)",
    "Swoop\uFF1A\u5DF2\u8FDE\u63A5": "Swoop: Connected",
    "\u7528 Swoop \u4E0B\u8F7D": "Download with Swoop",
    "\u7528 Swoop \u4E0B\u8F7D\u672C\u9875\u94FE\u63A5\u2026": "Download page links with Swoop\u2026",
    "\u6B63\u5728\u8BFB\u53D6\u94FE\u63A5\u2026": "Loading links\u2026",
    "\u7B5B\u9009\u540D\u79F0\u6216\u7F51\u5740": "Filter by name or URL",
    "\u6587\u4EF6\u7C7B\u578B": "File type",
    "\u6240\u6709\u7C7B\u578B": "All types",
    "\u5168\u9009\u5F53\u524D\u7ED3\u679C": "Select visible",
    "\u53D6\u6D88\u5F53\u524D\u9009\u62E9": "Clear visible selection",
    "\u9009\u62E9": "Select",
    "\u94FE\u63A5": "Link",
    "\u63D0\u4EA4\u7ED3\u679C": "Submission result",
    "\u6CA1\u6709\u7B26\u5408\u6761\u4EF6\u7684\u94FE\u63A5": "No matching links",
    "\u505C\u6B62\u540E\u7EED\u63D0\u4EA4": "Stop pending submissions",
    "\u63D0\u4EA4\u6240\u9009": "Submit selected",
    "\u63D0\u4EA4\u6240\u9009\uFF08{count}\uFF09": "Submit selected ({count})",
    "\u65E0\u6269\u5C55\u540D": "No extension",
    "\u663E\u793A {visible} / {total} \xB7 \u5DF2\u9009 {selected} \xB7 \u5DF2\u63D0\u4EA4 {successes} \xB7 \u5931\u8D25 {failures}": "Showing {visible} / {total} \xB7 Selected {selected} \xB7 Sent {successes} \xB7 Failed {failures}",
    "\u63A5\u7BA1\u6216\u5A92\u4F53\u68C0\u6D4B\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5728\u6269\u5C55\u8BBE\u7F6E\u4E2D\u5F00\u542F\u3002": "Capture or media detection is off. Enable it in extension settings.",
    "\u9009\u62E9\u5A92\u4F53\u540E\u4EA4\u7ED9 Swoop\uFF1B\u6D41\u5A92\u4F53\u6E05\u5355\u4F1A\u6253\u5F00\u5A92\u4F53\u7A97\u53E3\u3002": "Select media to send to Swoop. Streaming playlists open in the media window.",
    "\u94FE\u63A5\u8F83\u591A\uFF0C\u5217\u8868\u5DF2\u622A\u65AD\uFF1B\u53EF\u5728\u8BBE\u7F6E\u4E2D\u8C03\u6574\u6BCF\u9875\u4E0A\u9650\u3002": "The list is truncated. Increase the per-page limit in settings to show more.",
    "\u9009\u62E9\u8981\u4E0B\u8F7D\u7684\u94FE\u63A5\u540E\u63D0\u4EA4\u3002": "Select the links you want to download, then submit.",
    "\u9690\u8EAB\u7A97\u53E3\u4E0D\u4F1A\u4F20\u9012\u767B\u5F55\u4FE1\u606F\u3002": "Private windows do not share login information.",
    "\u63D0\u4EA4\u4E2D\u2026": "Submitting\u2026",
    "\u2713 \u5DF2\u4EA4\u7ED9 Swoop": "\u2713 Sent to Swoop",
    "\u5F85\u63D0\u4EA4": "Ready to submit",
    "\u5F85\u9009\u62E9": "Not selected",
    "\u9009\u62E9 {title}": "Select {title}",
    "\u6B63\u5728\u7B49\u5F85\u5F53\u524D\u63D0\u4EA4\u2026": "Waiting for current submissions\u2026",
    "\u672A\u627E\u5230\u5F53\u524D\u7F51\u9875": "The current tab was not found.",
    "\u672A\u80FD\u8BFB\u53D6\u7F51\u9875\u94FE\u63A5": "Could not read page links.",
    "\u672A\u80FD\u8BFB\u53D6\u7F51\u9875\u5A92\u4F53": "Could not read page media.",
    "\u65E0\u6CD5\u6253\u5F00\u7F51\u9875\u5A92\u4F53": "Could not open page media.",
    "\u94FE\u63A5\u5217\u8868\u5DF2\u8FC7\u671F\uFF0C\u8BF7\u8FD4\u56DE\u539F\u7F51\u9875\u91CD\u65B0\u63D0\u53D6": "This link list expired. Collect links from the original page again.",
    "\u6B64\u94FE\u63A5\u5217\u8868\u5DF2\u8FC7\u671F\uFF0C\u8BF7\u8FD4\u56DE\u539F\u7F51\u9875\u91CD\u65B0\u63D0\u53D6": "This link list expired. Collect links from the original page again.",
    "\u6269\u5C55\u672A\u8FD4\u56DE\u63D0\u4EA4\u7ED3\u679C": "The extension did not return a submission result.",
    "\u63D0\u4EA4\u4E2D\u65AD\uFF0C\u8BF7\u5148\u68C0\u67E5 Swoop \u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5": "Submission was interrupted. Check Swoop's task list before retrying.",
    "\u65E0\u6CD5\u6253\u5F00\u94FE\u63A5\u5217\u8868": "Could not open the link list.",
    "\u5A92\u4F53\u68C0\u6D4B\u6682\u4E0D\u53EF\u7528\uFF0C\u8BF7\u68C0\u67E5\u6269\u5C55\u6743\u9650\u540E\u91CD\u8BD5": "Media detection is unavailable. Check extension permissions and try again.",
    "\u5A92\u4F53\u68C0\u6D4B\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5728\u6269\u5C55\u8BBE\u7F6E\u4E2D\u5F00\u542F": "Media detection is off. Enable it in extension settings.",
    "\u5C1A\u672A\u53D1\u73B0\u5A92\u4F53\uFF0C\u8BF7\u5148\u5728\u7F51\u9875\u64AD\u653E\u97F3\u89C6\u9891\u540E\u91CD\u8BD5": "No media found yet. Play audio or video on the page, then try again.",
    "\u65E0\u6CD5\u8BFB\u53D6\u5A92\u4F53\u5217\u8868": "Could not read the media list.",
    "\u65E0\u6CD5\u8BFB\u53D6\u5A92\u4F53": "Could not read media.",
    "\u63D0\u4EA4\u5931\u8D25\uFF0C\u8BF7\u5148\u68C0\u67E5 Swoop \u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5": "Submission failed. Check Swoop's task list before retrying.",
    "\u8BF7\u5728\u666E\u901A\u7F51\u9875\u4E0A\u4F7F\u7528\u6B64\u529F\u80FD": "Use this feature on an HTTP or HTTPS page.",
    "Swoop \u672A\u8FD0\u884C\uFF0C\u8BF7\u5148\u542F\u52A8 Swoop": "Swoop is not running. Start Swoop first.",
    "\u5F53\u524D Swoop \u5C1A\u4E0D\u652F\u6301\u5A92\u4F53\u63A5\u6536\uFF0C\u8BF7\u66F4\u65B0\u540E\u91CD\u8BD5": "This version of Swoop cannot receive media. Update Swoop and try again.",
    "\u94FE\u63A5\u4E0D\u5728\u672C\u9875\u5217\u8868\u4E2D": "The link is not in this page's list.",
    "\u5A92\u4F53\u68C0\u6D4B\u5DF2\u5173\u95ED": "Media detection is off.",
    "\u539F\u7F51\u9875\u5DF2\u53D8\u5316\uFF0C\u8BF7\u91CD\u65B0\u63D0\u53D6\u94FE\u63A5": "The original page changed. Collect its links again.",
    "\u539F\u7F51\u9875\u5DF2\u5173\u95ED\uFF0C\u8BF7\u91CD\u65B0\u6253\u5F00\u540E\u63D0\u53D6\u94FE\u63A5": "The original page was closed. Reopen it and collect its links again.",
    "\u63A5\u7BA1\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5148\u5728\u6269\u5C55\u5F39\u7A97\u4E2D\u5F00\u542F": "Capture is off. Enable it in the extension popup first.",
    "\u672A\u80FD\u8BFB\u53D6\u7F51\u9875\u94FE\u63A5\uFF0C\u8BF7\u5237\u65B0\u7F51\u9875\u540E\u91CD\u8BD5": "Could not read page links. Refresh the page and try again.",
    "\u6B64\u9875\u9762\u4E0D\u5141\u8BB8\u8BFB\u53D6\u94FE\u63A5\uFF0C\u8BF7\u68C0\u67E5\u6269\u5C55\u7684\u7F51\u7AD9\u8BBF\u95EE\u6743\u9650": "Links cannot be read on this page. Check the extension's site access permissions.",
    "\u65E0\u6CD5\u4FDD\u5B58\u6216\u6253\u5F00\u94FE\u63A5\u5217\u8868\uFF0C\u8BF7\u5173\u95ED\u65E7\u5217\u8868\u540E\u91CD\u8BD5": "Could not save or open the link list. Close older lists and try again.",
    "\u65E0\u6548\u94FE\u63A5": "Invalid URL",
    "\u6B64\u57DF\u540D\u5DF2\u5728\u6269\u5C55\u8BBE\u7F6E\u4E2D\u6392\u9664": "This domain is excluded in extension settings.",
    "\u53EA\u652F\u6301 HTTP(S) / FTP(S) \u94FE\u63A5": "Only HTTP(S) and FTP(S) URLs are supported.",
    "FTP \u8D26\u53F7\u8BF7\u5728 Swoop \u4E2D\u586B\u5199\uFF0C\u4E0D\u8981\u653E\u5728\u94FE\u63A5\u4E2D": "Enter FTP credentials in Swoop, not in the URL.",
    "\u8BF7\u8F93\u5165 FTP \u6587\u4EF6\u94FE\u63A5": "Enter an FTP file URL.",
    "\u65E0\u6548 FTP \u6587\u4EF6\u94FE\u63A5": "Invalid FTP file URL",
    "\u63D0\u4EA4\u5931\u8D25": "Submission failed",
    "Swoop \u8FD4\u56DE\u4E86\u65E0\u6548\u7ED3\u679C\uFF0C\u8BF7\u5148\u68C0\u67E5\u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5": "Swoop returned an invalid result. Check its task list before retrying.",
    "Swoop \u8FD4\u56DE HTTP {status}": "Swoop returned HTTP {status}",
    "Swoop \u54CD\u5E94\u8D85\u65F6\uFF0C\u8BF7\u5148\u68C0\u67E5\u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5": "Swoop timed out. Check its task list before retrying.",
    "\u65E0\u6CD5\u8FDE\u63A5 Swoop": "Could not connect to Swoop."
  };
  function normalizeLanguage(value) {
    return value === "en-US" ? "en-US" : "zh-CN";
  }
  function translate(key, language, values = {}) {
    const template = language === "en-US" ? ENGLISH[key] ?? key : key;
    return template.replace(/\{(\w+)\}/g, (match, name) => String(values[name] ?? match));
  }
  function translateError(error, language) {
    const http = /^Swoop 返回 HTTP (\d{3})$/.exec(error);
    return http ? translate("Swoop \u8FD4\u56DE HTTP {status}", language, { status: http[1] }) : translate(error, language);
  }

  // src/badge.ts
  function badgeFor(state, enabled, language = "zh-CN") {
    if (state.unseenFailure) {
      return { text: "!", color: "#e5484d", title: translate(
        "Swoop\uFF1A{error}",
        language,
        { error: translateError(state.lastResult?.error ?? "\u4E0A\u6B21\u4EA4\u63A5\u5931\u8D25", language) }
      ) };
    }
    if (!enabled) return { text: "", color: "#8b8d98", title: translate("Swoop\uFF1A\u5DF2\u5173\u95ED\u63A5\u7BA1", language) };
    if (!state.connected) return { text: "\u2022", color: "#8b8d98", title: translate("Swoop\uFF1A\u672A\u8FDE\u63A5\uFF08\u8BF7\u5148\u542F\u52A8 Swoop\uFF09", language) };
    return { text: "", color: "#4a6cf2", title: translate("Swoop\uFF1A\u5DF2\u8FDE\u63A5", language) };
  }

  // src/cookies.ts
  function formatCookies(list) {
    if (list.length === 0) return void 0;
    return [...list].sort((a, b) => b.path.length - a.path.length).map((c) => c.name ? `${c.name}=${c.value}` : c.value).join("; ");
  }
  async function cookieHeader(url, tabId) {
    try {
      const stores = tabId === void 0 ? [] : await browser.cookies.getAllCookieStores();
      const storeId = stores.find((store) => store.tabIds.includes(tabId))?.id;
      if (tabId !== void 0 && !storeId) return void 0;
      return formatCookies(await browser.cookies.getAll({ url, ...storeId ? { storeId } : {} }));
    } catch {
      return void 0;
    }
  }

  // src/rules.ts
  var skip = (reason) => ({ take: false, reason });
  function shouldIntercept(item, s) {
    if (!s.enabled) return skip("\u5DF2\u5173\u95ED\u63A5\u7BA1");
    if (item.incognito) return skip("\u9690\u8EAB\u7A97\u53E3\u7684\u4E0B\u8F7D\u4E0D\u63A5\u7BA1");
    let url;
    try {
      url = new URL(item.url);
    } catch {
      return skip("\u65E0\u6548\u94FE\u63A5");
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return skip(`\u4E0D\u63A5\u7BA1 ${url.protocol} \u94FE\u63A5`);
    if (s.excludedDomains.some((d) => matchesDomain(url.hostname, d))) return skip("\u57DF\u540D\u5728\u6392\u9664\u5217\u8868\u4E2D");
    const size = knownSize(item);
    if (s.minSizeMB > 0 && size !== void 0 && size < s.minSizeMB * 1024 * 1024) return skip("\u5C0F\u4E8E\u5927\u5C0F\u9608\u503C");
    return { take: true };
  }
  function knownSize(item) {
    const n = item.fileSize > 0 ? item.fileSize : item.totalBytes;
    return n > 0 ? n : void 0;
  }
  function matchesDomain(host, pattern) {
    const h = host.toLowerCase();
    const p = pattern.toLowerCase();
    return h === p || h.endsWith(`.${p}`);
  }
  function normalizePageLinks(raw, pageUrl, limit) {
    const seen = /* @__PURE__ */ new Set();
    const links = [];
    let current;
    try {
      const page = new URL(pageUrl);
      page.hash = "";
      current = page.href;
    } catch {
      current = pageUrl;
    }
    for (const candidate of raw) {
      let parsed;
      try {
        parsed = new URL(candidate.url, pageUrl);
      } catch {
        continue;
      }
      parsed.hash = "";
      if (manualUrlBlock(parsed, candidate.url)) continue;
      if (parsed.href === current || seen.has(parsed.href)) continue;
      seen.add(parsed.href);
      if (links.length >= limit) return { links, truncated: true };
      let name = parsed.pathname.split("/").pop() ?? "";
      try {
        name = decodeURIComponent(name);
      } catch {
      }
      const extension = /\.([a-z0-9]{1,12})$/i.exec(name)?.[1]?.toLowerCase() ?? "";
      links.push({
        url: parsed.href,
        title: candidate.title.trim().slice(0, 300) || name || parsed.hostname,
        extension,
        ...candidate.referrerPolicy ? { referrerPolicy: candidate.referrerPolicy } : {}
      });
    }
    return { links, truncated: false };
  }
  function manualDownloadBlock(url, settings) {
    if (!settings.enabled) return "\u63A5\u7BA1\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5148\u5728\u6269\u5C55\u5F39\u7A97\u4E2D\u5F00\u542F";
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return "\u65E0\u6548\u94FE\u63A5";
    }
    const blocked = manualUrlBlock(parsed, url);
    if (blocked) return blocked;
    if (settings.excludedDomains.some((domain) => matchesDomain(parsed.hostname, domain))) return "\u6B64\u57DF\u540D\u5DF2\u5728\u6269\u5C55\u8BBE\u7F6E\u4E2D\u6392\u9664";
    return void 0;
  }
  function manualUrlBlock(parsed, original) {
    if (/^https?:$/.test(parsed.protocol)) return void 0;
    if (!/^ftps?:$/.test(parsed.protocol)) return "\u53EA\u652F\u6301 HTTP(S) / FTP(S) \u94FE\u63A5";
    if (parsed.username || parsed.password || /^ftps?:\/\/[^/?#]*@/i.test(original.trimStart())) {
      return "FTP \u8D26\u53F7\u8BF7\u5728 Swoop \u4E2D\u586B\u5199\uFF0C\u4E0D\u8981\u653E\u5728\u94FE\u63A5\u4E2D";
    }
    if (!parsed.hostname || !parsed.pathname || parsed.pathname.endsWith("/") || parsed.search || parsed.hash || /;type=/i.test(parsed.pathname)) {
      return "\u8BF7\u8F93\u5165 FTP \u6587\u4EF6\u94FE\u63A5";
    }
    try {
      if (/[\u0000-\u001f\u007f-\u009f]/u.test(decodeURIComponent(parsed.pathname))) return "\u65E0\u6548 FTP \u6587\u4EF6\u94FE\u63A5";
    } catch {
      return "\u65E0\u6548 FTP \u6587\u4EF6\u94FE\u63A5";
    }
    return void 0;
  }
  function shouldOpenWelcome(reason, previousVersion, currentVersion) {
    if (reason === "install") return true;
    if (reason !== "update" || !previousVersion) return false;
    const featureLine = (version) => version.split(".").slice(0, 2).map((part) => Number.parseInt(part, 10) || 0).join(".");
    return featureLine(previousVersion) !== featureLine(currentVersion);
  }

  // src/collect.ts
  function collectDocumentLinks(scanLimit) {
    const anchors = document.querySelectorAll("a[href], area[href]");
    const links = [];
    let scanned = 0;
    for (const anchor of anchors) {
      if (scanned >= scanLimit) break;
      scanned++;
      let url;
      try {
        url = new URL(anchor.getAttribute("href") ?? "", document.baseURI).href;
      } catch {
        continue;
      }
      const pagePolicy = document.querySelector('meta[name="referrer" i]')?.content;
      const referrerPolicy = anchor.relList.contains("noreferrer") ? "no-referrer" : anchor.referrerPolicy || pagePolicy;
      links.push({
        url,
        title: (anchor.textContent || anchor.getAttribute("title") || anchor.getAttribute("alt") || "").trim().slice(0, 300),
        ...referrerPolicy ? { referrerPolicy } : {}
      });
    }
    return { pageUrl: location.href, title: document.title, links, truncated: anchors.length > scanned };
  }

  // src/settings.ts
  var DEFAULT_SETTINGS = {
    language: "zh-CN",
    enabled: true,
    port: 51230,
    excludedDomains: [],
    minSizeMB: 0,
    batchConcurrency: 2,
    maxPageLinks: 2e3,
    detectMedia: true,
    maxMediaCandidates: 100,
    mediaRetentionMinutes: 60
  };
  function normalizeSettings(raw) {
    const port = Number(raw.port);
    const minSizeMB = Number(raw.minSizeMB);
    const batchConcurrency = Number(raw.batchConcurrency);
    const maxPageLinks = Number(raw.maxPageLinks);
    const maxMediaCandidates = Number(raw.maxMediaCandidates);
    const mediaRetentionMinutes = Number(raw.mediaRetentionMinutes);
    return {
      language: normalizeLanguage(raw.language),
      enabled: typeof raw.enabled === "boolean" ? raw.enabled : DEFAULT_SETTINGS.enabled,
      port: Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : DEFAULT_SETTINGS.port,
      excludedDomains: Array.isArray(raw.excludedDomains) ? raw.excludedDomains.filter((d) => typeof d === "string" && d.length > 0) : [],
      minSizeMB: Number.isFinite(minSizeMB) && minSizeMB > 0 ? minSizeMB : 0,
      batchConcurrency: Number.isInteger(batchConcurrency) && batchConcurrency >= 1 && batchConcurrency <= 6 ? batchConcurrency : DEFAULT_SETTINGS.batchConcurrency,
      maxPageLinks: Number.isInteger(maxPageLinks) && maxPageLinks >= 100 && maxPageLinks <= 1e4 ? maxPageLinks : DEFAULT_SETTINGS.maxPageLinks,
      detectMedia: typeof raw.detectMedia === "boolean" ? raw.detectMedia : DEFAULT_SETTINGS.detectMedia,
      maxMediaCandidates: Number.isInteger(maxMediaCandidates) && maxMediaCandidates >= 10 && maxMediaCandidates <= 500 ? maxMediaCandidates : DEFAULT_SETTINGS.maxMediaCandidates,
      mediaRetentionMinutes: Number.isInteger(mediaRetentionMinutes) && mediaRetentionMinutes >= 1 && mediaRetentionMinutes <= 1440 ? mediaRetentionMinutes : DEFAULT_SETTINGS.mediaRetentionMinutes
    };
  }
  async function loadSettings() {
    const raw = await browser.storage.sync.get(Object.keys(DEFAULT_SETTINGS));
    return normalizeSettings(raw);
  }
  var DEFAULT_STATE = { connected: false, unseenFailure: false };
  async function loadState() {
    const raw = await browser.storage.session.get(["connected", "lastResult", "unseenFailure"]);
    return { ...DEFAULT_STATE, ...raw };
  }
  async function saveState(patch) {
    await browser.storage.session.set(patch);
  }

  // src/page-tasks.ts
  var PREFIX = "page-batch:";
  var RESULT_PREFIX = "page-result:";
  var MAX_AGE = 24 * 60 * 60 * 1e3;
  async function loadPageBatch(id) {
    const raw = await browser.storage.session.get(PREFIX + id);
    const batch = raw[PREFIX + id];
    if (!batch || Date.now() - batch.createdAt >= MAX_AGE) return void 0;
    const results = await browser.storage.session.get(batch.links.map((_, index) => `${RESULT_PREFIX}${id}:${index}`));
    return { ...batch, results: Object.fromEntries(batch.links.flatMap((link, index) => {
      const result = results[`${RESULT_PREFIX}${id}:${index}`];
      return result ? [[link.url, result]] : [];
    })) };
  }
  async function savePageResult(batch, url, result) {
    const index = batch.links.findIndex((link) => link.url === url);
    if (index >= 0) await browser.storage.session.set({ [`${RESULT_PREFIX}${batch.id}:${index}`]: result });
  }
  async function openPageLinks(tabId) {
    try {
      const [tab, settings] = await Promise.all([browser.tabs.get(tabId), loadSettings()]);
      if (!settings.enabled) return { ok: false, error: "\u63A5\u7BA1\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5148\u5728\u6269\u5C55\u5F39\u7A97\u4E2D\u5F00\u542F" };
      if (!tab.url || !/^https?:/i.test(tab.url)) return { ok: false, error: "\u8BF7\u5728\u666E\u901A\u7F51\u9875\u4E0A\u4F7F\u7528\u6B64\u529F\u80FD" };
      const captures = await browser.scripting.executeScript({
        target: { tabId },
        func: collectDocumentLinks,
        args: [settings.maxPageLinks * 10]
      });
      const capture = captures[0]?.result;
      if (!capture) return { ok: false, error: "\u672A\u80FD\u8BFB\u53D6\u7F51\u9875\u94FE\u63A5\uFF0C\u8BF7\u5237\u65B0\u7F51\u9875\u540E\u91CD\u8BD5" };
      const normalized = normalizePageLinks(capture.links, capture.pageUrl, settings.maxPageLinks);
      const batch = {
        id: crypto.randomUUID(),
        tabId,
        pageUrl: capture.pageUrl,
        title: capture.title,
        incognito: tab.incognito ?? false,
        links: normalized.links,
        truncated: capture.truncated || normalized.truncated,
        createdAt: Date.now()
      };
      return await saveAndOpenBatch(batch, tab.windowId);
    } catch {
      return { ok: false, error: "\u6B64\u9875\u9762\u4E0D\u5141\u8BB8\u8BFB\u53D6\u94FE\u63A5\uFF0C\u8BF7\u68C0\u67E5\u6269\u5C55\u7684\u7F51\u7AD9\u8BBF\u95EE\u6743\u9650" };
    }
  }
  async function saveAndOpenBatch(batch, windowId) {
    try {
      const previous = await browser.storage.session.get(null);
      const oldKeys = Object.keys(previous).filter((key) => key.startsWith(PREFIX)).sort((a, b) => previous[b].createdAt - previous[a].createdAt);
      const remove = oldKeys.filter((key, index) => index >= 3 || Date.now() - previous[key].createdAt >= MAX_AGE);
      const expiredIds = remove.map((key) => key.slice(PREFIX.length));
      const resultKeys = Object.keys(previous).filter((key) => expiredIds.some((id) => key.startsWith(`${RESULT_PREFIX}${id}:`)));
      if (remove.length || resultKeys.length) await browser.storage.session.remove([...remove, ...resultKeys]);
      await browser.storage.session.set({ [PREFIX + batch.id]: batch });
      await browser.tabs.create({ url: browser.runtime.getURL(`links.html?batch=${encodeURIComponent(batch.id)}`), windowId });
      return { ok: true, batch };
    } catch {
      return { ok: false, error: "\u65E0\u6CD5\u4FDD\u5B58\u6216\u6253\u5F00\u94FE\u63A5\u5217\u8868\uFF0C\u8BF7\u5173\u95ED\u65E7\u5217\u8868\u540E\u91CD\u8BD5" };
    }
  }

  // src/request-context.ts
  function safeReferrer(target, source, policy) {
    if (!source || policy === "no-referrer") return void 0;
    try {
      const to = new URL(target);
      const from = new URL(source);
      if (!/^https?:$/.test(from.protocol) || !/^https?:$/.test(to.protocol)) return void 0;
      if (from.protocol === "https:" && to.protocol !== "https:") return void 0;
      from.username = from.password = from.hash = "";
      if (policy === "same-origin" && from.origin !== to.origin) return void 0;
      if (policy === "origin" || policy === "strict-origin") return `${from.origin}/`;
      return from.origin === to.origin ? from.href : `${from.origin}/`;
    } catch {
      return void 0;
    }
  }
  function contextHeaders(userAgent, cookie) {
    const valid = (value) => !/[\u0000-\u001f\u007f]/.test(value);
    const headers = [];
    if (userAgent && valid(userAgent)) headers.push(["User-Agent", userAgent]);
    if (cookie && valid(cookie)) headers.push(["Cookie", cookie]);
    return headers;
  }
  function redirectTarget(url, finalUrl) {
    if (!finalUrl) return void 0;
    try {
      const from = new URL(url);
      const to = new URL(finalUrl);
      if (!/^https?:$/.test(to.protocol) || from.origin === to.origin) return void 0;
      return to.href;
    } catch {
      return void 0;
    }
  }

  // src/media.ts
  function detectMedia(response, now) {
    if (response.tabId < 0 || response.method !== "GET" || ![200, 206].includes(response.statusCode)) return void 0;
    let url;
    try {
      url = new URL(response.url);
    } catch {
      return void 0;
    }
    if (!/^https?:$/.test(url.protocol) || url.username || url.password || url.href.length > 8192) return void 0;
    url.hash = "";
    const header = (name2) => response.responseHeaders?.find((header2) => header2.name.toLowerCase() === name2)?.value;
    const mime = header("content-type")?.split(";")[0]?.trim().toLowerCase();
    let name = url.pathname.split("/").pop() || url.hostname;
    try {
      name = decodeURIComponent(name);
    } catch {
    }
    const extension = /\.([a-z0-9]{1,12})$/i.exec(name)?.[1]?.toLowerCase() ?? "";
    if (["ts", "m4s", "cmfv", "cmfa", "key"].includes(extension) || mime === "video/mp2t") return void 0;
    if (mime === "text/html" || mime === "application/json" || mime === "application/xml" && extension !== "mpd") return void 0;
    let kind;
    if (extension === "m3u8" || ["application/vnd.apple.mpegurl", "application/x-mpegurl", "audio/mpegurl", "audio/x-mpegurl"].includes(mime ?? "")) kind = "hls";
    else if (extension === "mpd" || mime === "application/dash+xml") kind = "dash";
    else if (mime?.startsWith("video/") || ["mp4", "webm", "mov", "mkv", "m4v", "avi", "flv", "ogv"].includes(extension)) kind = "video";
    else if (mime?.startsWith("audio/") || ["mp3", "m4a", "ogg", "opus", "wav", "flac", "aac"].includes(extension)) kind = "audio";
    else return void 0;
    const rawSize = response.statusCode === 206 ? /\/([0-9]+)$/.exec(header("content-range") ?? "")?.[1] : header("content-length");
    const size = rawSize && /^\d+$/.test(rawSize) ? Number(rawSize) : void 0;
    const media = kind === "hls" || kind === "dash" ? kind : void 0;
    return {
      url: url.href,
      title: `${name.slice(0, 260)} \xB7 ${kind.toUpperCase()}`,
      extension: extension || (media ?? kind),
      kind,
      detectedAt: now,
      ...mime ? { mime } : {},
      ...media ? { media } : {},
      ...!media && size !== void 0 && Number.isSafeInteger(size) && size >= 0 ? { size } : {}
    };
  }
  function mergeCandidates(previous, incoming, max, ageMs, now) {
    const byUrl = new Map(previous.filter((item) => now - item.detectedAt < ageMs).map((item) => [item.url, item]));
    if (incoming) byUrl.set(incoming.url, { ...byUrl.get(incoming.url), ...incoming });
    return [...byUrl.values()].sort((a, b) => b.detectedAt - a.detectedAt).slice(0, max);
  }

  // src/media-catalog.ts
  var PREFIX2 = "media-tab:";
  var MediaCatalog = class {
    pending = Promise.resolve();
    error;
    serial(work) {
      const result = this.pending.catch(() => {
      }).then(work);
      this.pending = result;
      return result;
    }
    capture(response) {
      const candidate = detectMedia(response, Date.now());
      if (!candidate) return Promise.resolve();
      return this.serial(async () => {
        const settings = await loadSettings();
        if (!settings.detectMedia || manualDownloadBlock(candidate.url, settings)) return;
        let tab;
        try {
          tab = await browser.tabs.get(response.tabId);
        } catch {
          return;
        }
        if (tab.incognito || !tab.url || !/^https?:/i.test(tab.url)) return;
        const key = PREFIX2 + response.tabId;
        const raw = await browser.storage.session.get(key);
        const stored = raw[key];
        const candidates = mergeCandidates(
          stored?.pageUrl === tab.url ? stored.candidates : [],
          candidate,
          settings.maxMediaCandidates,
          settings.mediaRetentionMinutes * 6e4,
          Date.now()
        );
        await browser.storage.session.set({ [key]: { pageUrl: tab.url, candidates, updatedAt: Date.now() } });
        await this.pruneStored();
        this.error = void 0;
      }).catch(() => {
        this.error = "\u5A92\u4F53\u68C0\u6D4B\u6682\u4E0D\u53EF\u7528\uFF0C\u8BF7\u68C0\u67E5\u6269\u5C55\u6743\u9650\u540E\u91CD\u8BD5";
      });
    }
    clearTab(tabId) {
      return this.serial(() => browser.storage.session.remove(PREFIX2 + tabId));
    }
    cleanup() {
      return this.serial(() => this.pruneStored());
    }
    async pruneStored() {
      const settings = await loadSettings();
      const all = await browser.storage.session.get(null);
      const records = Object.entries(all).filter(([key]) => key.startsWith(PREFIX2));
      records.sort(([, a], [, b]) => b.updatedAt - a.updatedAt);
      const remove = [];
      let count = 0;
      let bytes = 0;
      for (const [index, [key, tab]] of records.entries()) {
        const live = mergeCandidates(tab.candidates, void 0, settings.maxMediaCandidates, settings.mediaRetentionMinutes * 6e4, Date.now()).filter((candidate) => !manualDownloadBlock(candidate.url, settings));
        count += live.length;
        bytes += new TextEncoder().encode(JSON.stringify(live)).length;
        if (!settings.enabled || !settings.detectMedia || !live.length || count > 2e3 || bytes > 2 * 1024 * 1024 || index >= 24) {
          remove.push(key);
        } else if (live.length !== tab.candidates.length) {
          await browser.storage.session.set({ [key]: { ...tab, candidates: live } });
        }
      }
      if (remove.length) await browser.storage.session.remove(remove);
    }
    async read(tabId) {
      await this.cleanup();
      if (this.error) throw new Error(this.error);
      const tab = await browser.tabs.get(tabId);
      const raw = await browser.storage.session.get(PREFIX2 + tabId);
      const stored = raw[PREFIX2 + tabId];
      return { tab, candidates: stored && stored.pageUrl === tab.url ? stored.candidates : [] };
    }
    async open(tabId) {
      try {
        const settings = await loadSettings();
        if (!settings.enabled || !settings.detectMedia) return { ok: false, error: "\u5A92\u4F53\u68C0\u6D4B\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5728\u6269\u5C55\u8BBE\u7F6E\u4E2D\u5F00\u542F" };
        const { tab, candidates } = await this.read(tabId);
        if (!candidates.length) return { ok: false, error: "\u5C1A\u672A\u53D1\u73B0\u5A92\u4F53\uFF0C\u8BF7\u5148\u5728\u7F51\u9875\u64AD\u653E\u97F3\u89C6\u9891\u540E\u91CD\u8BD5" };
        return saveAndOpenBatch({
          id: crypto.randomUUID(),
          tabId,
          pageUrl: tab.url,
          title: tab.title ?? "",
          incognito: false,
          links: candidates,
          truncated: candidates.length >= settings.maxMediaCandidates,
          createdAt: Date.now(),
          kind: "media"
        }, tab.windowId);
      } catch (error) {
        return { ok: false, error: error instanceof Error ? error.message : "\u65E0\u6CD5\u8BFB\u53D6\u5A92\u4F53\u5217\u8868" };
      }
    }
  };

  // src/background.ts
  var HEARTBEAT_ALARM = "swoop-heartbeat";
  var MENU_ID = "swoop-download";
  var PAGE_MENU_ID = "swoop-page-links";
  var mediaCatalog = new MediaCatalog();
  browser.webRequest.onHeadersReceived.addListener((response) => {
    void mediaCatalog.capture(response);
  }, { urls: ["http://*/*", "https://*/*"] }, ["responseHeaders"]);
  browser.tabs.onRemoved.addListener((tabId) => {
    void mediaCatalog.clearTab(tabId);
  });
  browser.tabs.onUpdated.addListener((tabId, change) => {
    if (change.url) void mediaCatalog.clearTab(tabId);
  });
  async function refreshBadge() {
    const [settings, state] = await Promise.all([loadSettings(), loadState()]);
    const badge = badgeFor(state, settings.enabled, settings.language);
    await Promise.all([
      browser.action.setBadgeText({ text: badge.text }),
      browser.action.setBadgeBackgroundColor({ color: badge.color }),
      browser.action.setTitle({ title: badge.title })
    ]);
  }
  async function heartbeat() {
    const settings = await loadSettings();
    const connected = settings.enabled && await ping(settings.port);
    await saveState({ connected });
    await refreshBadge();
    await mediaCatalog.cleanup().catch(() => {
    });
    return connected;
  }
  var menuRefresh = Promise.resolve();
  function refreshMenus() {
    menuRefresh = menuRefresh.catch(() => {
    }).then(async () => {
      const { language } = await loadSettings();
      await browser.contextMenus.removeAll();
      await browser.contextMenus.create({ id: MENU_ID, title: translate("\u7528 Swoop \u4E0B\u8F7D", language), contexts: ["link", "image", "video", "audio"] });
      await browser.contextMenus.create({ id: PAGE_MENU_ID, title: translate("\u7528 Swoop \u4E0B\u8F7D\u672C\u9875\u94FE\u63A5\u2026", language), contexts: ["page", "link", "image", "video", "audio"] });
    });
    return menuRefresh;
  }
  function setup() {
    void browser.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: isFirefox ? 1 : 0.5 });
    void heartbeat();
    void refreshMenus().catch((error) => console.warn("Swoop context menu refresh failed", error));
  }
  browser.runtime.onInstalled.addListener((details) => {
    setup();
    if (details && shouldOpenWelcome(details.reason, details.previousVersion, browser.runtime.getManifest().version)) {
      void browser.tabs.create({ url: browser.runtime.getURL("welcome.html") });
    }
  });
  browser.runtime.onStartup.addListener(setup);
  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === HEARTBEAT_ALARM) void heartbeat();
  });
  browser.storage.onChanged.addListener((changes, area) => {
    if (area === "sync") void heartbeat();
    if (area === "sync" && changes.language) {
      void refreshMenus().catch((error) => console.warn("Swoop context menu refresh failed", error));
    }
  });
  browser.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (!sender.url?.startsWith(browser.runtime.getURL(""))) return false;
    const request = msg;
    if (request?.type === "refresh") {
      void heartbeat().then(sendResponse);
      return true;
    }
    if (request?.type === "collect-page" && Number.isInteger(request.tabId)) {
      void openPageLinks(request.tabId).then(sendResponse);
      return true;
    }
    if (request?.type === "open-media" && Number.isInteger(request.tabId)) {
      void mediaCatalog.open(request.tabId).then(sendResponse);
      return true;
    }
    if (request?.type === "media-count" && Number.isInteger(request.tabId)) {
      void mediaCatalog.read(request.tabId).then(({ candidates }) => sendResponse({ ok: true, count: candidates.length })).catch((error) => sendResponse({ ok: false, error: error instanceof Error ? error.message : "\u65E0\u6CD5\u8BFB\u53D6\u5A92\u4F53" }));
      return true;
    }
    if (request?.type === "submit-page-link" && typeof request.batchId === "string" && typeof request.url === "string") {
      void submitPageLink(request.batchId, request.url).then(sendResponse).catch(() => sendResponse({ ok: false, error: "\u63D0\u4EA4\u5931\u8D25\uFF0C\u8BF7\u5148\u68C0\u67E5 Swoop \u4EFB\u52A1\u5217\u8868\u518D\u91CD\u8BD5" }));
      return true;
    }
    return false;
  });
  async function buildTask(url, referrer, withCookies, tabId, referrerPolicy) {
    const parsed = new URL(url);
    if (/^ftps?:$/.test(parsed.protocol)) return { url: parsed.href, headers: [] };
    const cookie = withCookies ? await cookieHeader(url, tabId) : void 0;
    return { url, referrer: safeReferrer(url, referrer, referrerPolicy), headers: contextHeaders(navigator.userAgent, cookie) };
  }
  async function record(url, result, fellBack) {
    if (/^ftps?:/i.test(url.trimStart())) {
      try {
        const safe = new URL(url);
        safe.username = "";
        safe.password = "";
        url = safe.href;
      } catch {
        url = "";
      }
    }
    const lastResult = result.ok ? { ok: true, url, fileName: result.fileName, size: result.size, fellBack: false, at: Date.now() } : { ok: false, url, error: result.error, fellBack, at: Date.now() };
    await saveState({ lastResult, unseenFailure: !result.ok, connected: result.ok || (await loadState()).connected });
    await refreshBadge();
  }
  async function readyToTake(item) {
    const settings = await loadSettings();
    if (!shouldIntercept(item, settings).take) return void 0;
    if (!await ping(settings.port, 800)) {
      await saveState({ connected: false });
      await refreshBadge();
      return void 0;
    }
    return settings.port;
  }
  async function handOff(item, port) {
    const body = await buildTask(item.url, item.referrer, true);
    const target = redirectTarget(item.url, item.finalUrl);
    const targetCookie = target ? await cookieHeader(target) : void 0;
    if (target && targetCookie) body.redirect_cookies = [[new URL(target).origin, targetCookie]];
    body.mime = item.mime || void 0;
    body.size = knownSize(item);
    const result = await sendTask(port, body);
    if (result.ok) {
      await browser.downloads.cancel(item.id).catch(() => {
      });
      await browser.downloads.erase({ id: item.id }).catch(() => {
      });
    }
    await record(item.url, result, true).catch(() => {
    });
    return result.ok;
  }
  async function onDeterminingFilename(item) {
    const port = await readyToTake(item);
    return port !== void 0 && handOff(item, port);
  }
  async function onDownloadCreated(item) {
    const port = await readyToTake(item);
    if (port === void 0) return;
    try {
      await browser.downloads.pause(item.id);
    } catch {
      return;
    }
    if (!await handOff(item, port)) await browser.downloads.resume(item.id).catch(() => {
    });
  }
  if (browser.downloads.onDeterminingFilename) {
    browser.downloads.onDeterminingFilename.addListener((item, suggest) => {
      void onDeterminingFilename(item).catch(() => false).then((taken) => {
        if (!taken) suggest();
      });
      return true;
    });
  } else {
    browser.downloads.onCreated.addListener((item) => {
      void onDownloadCreated(item);
    });
  }
  async function submitManual(url, referrer, incognito, tabId, referrerPolicy, hint) {
    const settings = await loadSettings();
    const blocked = manualDownloadBlock(url, settings);
    if (blocked) return { ok: false, error: blocked };
    if (/^ftps?:$/.test(new URL(url).protocol) && !/^https?:\/\//i.test(referrer ?? "")) {
      return { ok: false, error: "\u8BF7\u5728\u666E\u901A\u7F51\u9875\u4E0A\u4F7F\u7528\u6B64\u529F\u80FD" };
    }
    if (!await ping(settings.port)) return { ok: false, error: "Swoop \u672A\u8FD0\u884C\uFF0C\u8BF7\u5148\u542F\u52A8 Swoop" };
    const body = await buildTask(url, referrer, !incognito, tabId, referrerPolicy);
    Object.assign(body, hint);
    const current = await loadSettings();
    const changed = manualDownloadBlock(url, current);
    if (changed) return { ok: false, error: changed };
    if (hint?.media && !await supportsMedia(current.port)) return { ok: false, error: "\u5F53\u524D Swoop \u5C1A\u4E0D\u652F\u6301\u5A92\u4F53\u63A5\u6536\uFF0C\u8BF7\u66F4\u65B0\u540E\u91CD\u8BD5" };
    return sendTask(current.port, body);
  }
  var pageSubmissions = /* @__PURE__ */ new Map();
  function submitPageLink(batchId, url) {
    const key = JSON.stringify([batchId, url]);
    const existing = pageSubmissions.get(key);
    if (existing) return existing;
    const pending = doSubmitPageLink(batchId, url).finally(() => pageSubmissions.delete(key));
    pageSubmissions.set(key, pending);
    return pending;
  }
  async function doSubmitPageLink(batchId, url) {
    const batch = await loadPageBatch(batchId);
    if (!batch) return { ok: false, error: "\u6B64\u94FE\u63A5\u5217\u8868\u5DF2\u8FC7\u671F\uFF0C\u8BF7\u8FD4\u56DE\u539F\u7F51\u9875\u91CD\u65B0\u63D0\u53D6" };
    const link = batch.links.find((link2) => link2.url === url);
    if (!link) return { ok: false, error: "\u94FE\u63A5\u4E0D\u5728\u672C\u9875\u5217\u8868\u4E2D" };
    const previous = batch.results?.[url];
    if (previous?.ok) return previous;
    if (batch.kind === "media" && !(await loadSettings()).detectMedia) return { ok: false, error: "\u5A92\u4F53\u68C0\u6D4B\u5DF2\u5173\u95ED" };
    try {
      const tab = await browser.tabs.get(batch.tabId);
      if (tab.url !== batch.pageUrl || tab.incognito !== batch.incognito) {
        return { ok: false, error: "\u539F\u7F51\u9875\u5DF2\u53D8\u5316\uFF0C\u8BF7\u91CD\u65B0\u63D0\u53D6\u94FE\u63A5" };
      }
    } catch {
      return { ok: false, error: "\u539F\u7F51\u9875\u5DF2\u5173\u95ED\uFF0C\u8BF7\u91CD\u65B0\u6253\u5F00\u540E\u63D0\u53D6\u94FE\u63A5" };
    }
    const result = await submitManual(
      url,
      batch.pageUrl,
      batch.incognito,
      batch.tabId,
      link.referrerPolicy,
      { mime: link.mime, size: link.size, media: link.media }
    );
    await savePageResult(batch, url, result);
    await record(url, result, false);
    return result;
  }
  async function onMenuClicked(info, tab) {
    if (info.menuItemId === PAGE_MENU_ID) {
      if (tab?.id !== void 0) {
        const opened = await openPageLinks(tab.id);
        if (!opened.ok) await record(info.pageUrl ?? "", opened, false);
      }
      return;
    }
    if (info.menuItemId !== MENU_ID) return;
    const url = info.linkUrl ?? info.srcUrl ?? "";
    const result = await submitManual(url, info.frameUrl ?? info.pageUrl, tab?.incognito ?? false, tab?.id);
    await record(url, result, false);
  }
  browser.contextMenus.onClicked.addListener((info, tab) => {
    void onMenuClicked(info, tab);
  });
})();
