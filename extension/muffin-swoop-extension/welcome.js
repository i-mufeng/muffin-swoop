"use strict";
(() => {
  // src/browser.ts
  var browser = globalThis.browser ?? globalThis.chrome;

  // src/dom.ts
  function $(id) {
    const el = document.getElementById(id);
    if (!el) throw new Error(`#${id} \u4E0D\u5B58\u5728`);
    return el;
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
  function localizeDocument(doc, language) {
    doc.documentElement.lang = language;
    for (const node of doc.querySelectorAll("[data-i18n]")) {
      node.textContent = translate(node.dataset.i18n, language);
    }
    for (const attribute of ["placeholder", "aria-label", "title"]) {
      for (const node of doc.querySelectorAll(`[data-i18n-${attribute}]`)) {
        node.setAttribute(attribute, translate(node.getAttribute(`data-i18n-${attribute}`), language));
      }
    }
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
  async function saveSettings(patch) {
    await browser.storage.sync.set(patch);
  }

  // src/welcome.ts
  var POLL_MS = 3e3;
  var settings = DEFAULT_SETTINGS;
  var connection = "checking";
  var checking = false;
  var recheck = false;
  function render() {
    const t = (key, values) => translate(key, settings.language, values);
    localizeDocument(document, settings.language);
    $("language").value = settings.language;
    $("version").textContent = `v${browser.runtime.getManifest().version}`;
    $("connect").classList.toggle("ok", connection === "connected");
    $("dot").className = `dot ${{ checking: "wait", connected: "on", offline: "err", disabled: "" }[connection]}`;
    $("status").textContent = {
      checking: t("\u68C0\u6D4B\u4E2D\u2026"),
      connected: t("\u2713 \u5DF2\u8FDE\u63A5 Swoop\uFF0C\u53EF\u4EE5\u5F00\u59CB\u4E0B\u8F7D\u4E86"),
      offline: t("\u8FD8\u6CA1\u8FDE\u4E0A Swoop"),
      disabled: t("\u63A5\u7BA1\u5DF2\u5173\u95ED\uFF0C\u8BF7\u5148\u5728\u6269\u5C55\u5F39\u7A97\u4E2D\u5F00\u542F")
    }[connection];
    $("help").hidden = connection !== "offline";
    $("port-hint").textContent = t("\u6269\u5C55\u7AEF\u53E3\u4E3A {port}\uFF0C\u9700\u4E0E Swoop \u4E2D\u8BBE\u7F6E\u7684\u7AEF\u53E3\u4E00\u81F4\uFF1B\u4E0D\u4E00\u81F4\u8BF7\u5230\u6269\u5C55\u8BBE\u7F6E\u4FEE\u6539\u3002", { port: settings.port });
    $("retry").hidden = connection === "connected";
    $("retry").disabled = connection === "checking";
  }
  async function check(showProgress) {
    if (checking) {
      recheck = true;
      return;
    }
    checking = true;
    recheck = false;
    try {
      settings = await loadSettings();
      if (!settings.enabled) {
        connection = "disabled";
        return;
      }
      if (showProgress) {
        connection = "checking";
        render();
      }
      const connected = await browser.runtime.sendMessage({ type: "refresh" }).catch(() => false);
      connection = connected ? "connected" : "offline";
    } finally {
      checking = false;
      render();
    }
    if (recheck) await check(false);
  }
  $("language").addEventListener("change", (event) => {
    settings = { ...settings, language: normalizeLanguage(event.target.value) };
    render();
    void saveSettings({ language: settings.language });
  });
  $("open-options").addEventListener("click", () => {
    void browser.runtime.openOptionsPage();
  });
  $("retry").addEventListener("click", () => {
    void check(true);
  });
  browser.storage.onChanged.addListener((_changes, area) => {
    if (area === "sync") void check(false);
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void check(false);
  });
  setInterval(() => {
    if (!document.hidden && connection !== "connected") void check(false);
  }, POLL_MS);
  render();
  void check(true);
})();
