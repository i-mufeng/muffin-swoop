# 浏览器扩展方案（v0.3）

状态：Chromium 接管、本页链接批量选择、手动 FTP(S) 交接、网页媒体候选检测与 Firefox MV3 独立构建已实现（2026-10-11）。新增功能与 Firefox 已通过自动化和构建检查，真实浏览器验收仍按第 8 节区分。决策记录（2026-10-11）：

| 决策点 | 结论 |
| --- | --- |
| 目标浏览器 | Chromium 系（Chrome / Edge / Arc / Brave 同一个 MV3 包）及 Firefox 140+ 独立 MV3 包；Safari 尚未实现 |
| 接收行为 | 可配置，默认静默加入 + 吐司；可切换为弹出新建对话框 |
| 通信方式 | Swoop 内置本地 HTTP 服务（`127.0.0.1`），不用 Native Messaging |
| 扩展技术 | TypeScript + esbuild，Chromium 产物 `extension/dist/`、Firefox 产物 `extension/dist-firefox/` |

## 1. 目标与非目标

**要解决的 IDM 痛点**

- 页内打开 PDF / 图片 / 视频被抢走变成下载。
- 每次下载都弹对话框。
- 抓流媒体分片、几十 KB 小文件，徒增噪音。
- 登录后的下载链接拿不到 Cookie 而失败。

**非目标（v0.3 不做）**

- DRM 内容解密、密钥/许可证获取、绕过站点登录或访问限制。
- 内容脚本劫持页面链接点击。
- Swoop 未运行时由扩展拉起 Swoop（本地 HTTP 做不到，留给以后的 Native Messaging 可选项）。
- 按住修饰键绕过拦截（`downloads` API 不暴露按键状态）。

## 2. 拦截点：只接管浏览器自己判定为下载的请求

IDM 工作在网络层（响应头 + 扩展名），所以 `Content-Type: application/pdf` 但没有 `Content-Disposition: attachment` 的页内 PDF 也会被抓。

Swoop 扩展在 Chromium 上监听 `chrome.downloads.onDeterminingFilename`，Firefox（没有这个事件）监听 `downloads.onCreated`。两者都只在浏览器**已经决定要下载**时触发：`attachment`、用户"另存为"、浏览器无法渲染的类型。页内渲染 PDF、播放视频、显示图片都不经过它，所以机制上不会误抓。用户在 PDF 阅读器里点"保存"，才会产生一次下载事件，这时才交给 Swoop。

Chrome MV3 已经移除普通扩展的阻塞式 `webRequest`，网络层拦截对我们本来也不可行。

## 3. 交接协议：先拿住下载，确认成功再取消

```
Chromium：onDeterminingFilename(item, suggest)  —— 返回 true，异步拿住回调
Firefox ：onCreated(item) → downloads.pause(item.id)
  ├─ 过滤：总开关 / 仅 http(s) / 排除域名 / 大小阈值 / Swoop 在线
  ├─ POST /tasks {url, headers, referrer, mime, size}
  │     Swoop 侧：探测（上限 10s）→ 成功 200 / 失败 4xx
  ├─ 200 → downloads.cancel + downloads.erase   （浏览器下载消失，Swoop 接管）
  └─ 4xx / 网络错误 → suggest()（Chromium）/ downloads.resume（Firefox）（浏览器继续，用户无感）
```

Chromium 要等文件名确定后才把下载交给下载面板和"另存为"对话框，所以在 `onDeterminingFilename` 里接手成功、不调用 `suggest` 就取消，Edge / Chrome 的下载面板不会先弹出来。代价是每个浏览器下载都要等扩展回应：不接管的立即 `suggest`，Swoop 不在线时最多等 `ping` 的 800ms，回退时浏览器面板要等 Swoop 探测结束才出现。这一阶段浏览器下载不会完成，不需要 `pause`。其他扩展也监听这个事件改文件名时可能互相影响。

要点：

- **失败一定可回退**。拿住（Chromium 未 `suggest` / Firefox `pause`）期间浏览器下载还在，只有 Swoop 探测成功才 `cancel`。POST 表单结果、`401`、服务器拒绝 Range 探测等情况都回到浏览器。
- `blob:` / `data:` 直接放行；Firefox `pause` 失败（小文件已完成）直接放行。
- **HTTP(S) 浏览器上下文**：扩展用 `cookies` 权限取该 URL 的 Cookie（含 HttpOnly），连同 `Referer`、浏览器 `User-Agent` 一起传给 Swoop。手动提交匹配原标签的 Cookie store，无法确认容器时不回退到默认会话。隐身手动提交不带 Cookie。自动下载事件没有标签 ID，分区 Cookie 与 Firefox 容器自动接管仍需实测。
- **手动 FTP(S)**：普通 HTTP(S) 网页的右键链接和本页链接列表可提交 FTP(S) 文件。扩展不查询 Cookie，且不发送 `Referer`、UA 或任何 HTTP 请求头；浏览器下载自动接管与媒体检测仍仅限 HTTP(S)。
- `Referer` 删除凭据与片段，跨源仅保留来源站点，HTTPS 降级不发送；本页链接还尊重 `noreferrer`、链接属性及页面 meta 中更严格的来源策略（页面响应头策略不在当前提取范围）。UA / Cookie 含控制字符时不发送对应头值。
- 两种接收模式对扩展透明：Swoop 都先探测，成功后"静默"模式直接 `engine.add`，"询问"模式把探测结果塞进 channel，UI 下一帧弹出预填好文件名/大小的新建对话框。扩展只看 200。

## 4. 本地 HTTP 接口

绑定 `127.0.0.1:51230`（可在设置里改），hyper 实现（reqwest 已依赖 hyper，编译成本近零）。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/ping` | `{ "app": "swoop", "version": "0.1.0", "media_handoff": true }`，Chromium 每 30s、Firefox 每 60s 更新徽标；旧版本可无媒体能力字段 |
| `POST` | `/tasks` | 请求体见下；200 返回 `{ "id": 12, "file_name": "...", "size": 1234 }`，4xx 返回 `{ "error": "..." }` |

```json
{
  "url": "https://example.com/file.zip",
  "referrer": "https://example.com/page",
  "headers": [["Cookie", "a=1; b=2"], ["User-Agent", "Mozilla/5.0 ..."]],
  "mime": "application/zip",
  "size": 1234,
  "redirect_cookies": [["https://cdn.example.com", "s=1"]]
}
```

- `redirect_cookies`（可选）：浏览器下载跳转到其他源时（`DownloadItem.finalUrl`），附带那个源的 Cookie。已登录站点常这样跳（download.oracle.com → edelivery.oracle.com）。Swoop 跨域跳转时照旧丢弃原 Cookie，只在请求对应源时带上它自己的 Cookie；任务里以内部头 `x-swoop-redirect-cookie: <origin> <cookie>` 持久化，从不原样发出。

- FTP(S) 仅通过手动入口发送，例如 `{ "url": "ftp://files.example.com/archive.zip", "headers": [] }`，不带 `referrer`。桌面按 FTP 默认配置探测并接收匿名下载；`ftps://` 使用隐式 TLS 且验证证书。需要账号的站点应在桌面新建任务中填写独立 FTP 凭据，不能把用户名或密码放进 URL。扩展不接受 FTP 目录、查询参数、控制字符或 `;type=` 指令；被拒绝的凭据 URL 不写入链接列表，最近结果也会清除用户名和密码。
- `mime` / `size` 可选，来自 `DownloadItem`。Swoop 用 GET 重探后核对：浏览器下的不是网页、Swoop 却拿到 `text/html`（POST 表单、未登录跳转页），或两边大小都已知但不相等，就返回 422，浏览器继续下。右键菜单不传这两个字段。
- `suggested_name` 已从协议中去掉：扩展拿到下载时浏览器还没确定文件名，文件名以 Swoop 探测结果（Content-Disposition / URL）为准。
- `media` 可选，且只能为 `"hls"` 或 `"dash"`：网页媒体候选中的清单必须携带此字段，交给桌面媒体窗口由用户继续操作，不能把清单文本作为普通下载完成。成功仍返回有效的 `file_name`、`size`（可空），表示桌面已接收，不代表下载完成。普通音视频不传此字段。媒体窗口如果拒绝凭据头、受保护内容或不支持的输入，扩展逐项显示实际错误；不擅自丢掉 Cookie 后重试。
- 发出带 `media` 的任务前，扩展必须确认 `/ping` 的 `media_handoff` 严格为 `true`；未声明的旧桌面仅提示更新，不能依靠旧服务忽略未知 JSON 字段的行为。

**安全边界**

- 只绑定回环地址。
- `Host` 必须是 `127.0.0.1` / `localhost`（防 DNS rebinding）。
- `Origin` 只允许缺省（本机脚本）和 `chrome-extension://` / `moz-extension://`；`http(s)://` 与 `null`（沙箱 iframe、file 页面）一律拒绝。网页无法伪造 Origin，Chrome 的 Private Network Access 也会拦网页访问 localhost。
- `POST /tasks` 要求 `Content-Type: application/json`，网页的"简单请求"表单带不了这个类型。
- 不校验具体扩展 ID：开发者模式下 ID 随路径变化，且本机其它扩展已属可信代码。
- 请求体上限 64 KB，单连接超时 15s。

**隐私说明**：Cookie 作为任务 headers 持久化到 `swoop.db`（明文），否则重启后续传登录下载会失败。IDM 同样如此。数据库在用户目录、仅本用户可读。

## 5. 代码改动

### 5.1 `crates/core`

- `AddOptions` 新增 `headers: Vec<(String, String)>`；`probe` 和 `engine.rs` 的分段请求（`client.get(...)` 处）都附加这些头。按请求设置的 `User-Agent` 会覆盖 `Config.user_agent`。
- `TaskRecord` / `TaskShared` 增加 `headers`；`tasks` 表加列 `headers TEXT NOT NULL DEFAULT '[]'`（JSON）。按现有约定用 `CREATE TABLE IF NOT EXISTS` + 启动时 `ALTER TABLE ... ADD COLUMN` 的幂等补列。
- 新增 `Engine::probe_url(url, headers) -> Result<ProbeInfo>`，供 bridge 在不建任务的情况下探测。`ProbeInfo` 导出到 crate 根。
- 测试（`crates/core/tests/`）：本地服务器要求 `Cookie: token=1` 否则 403，验证带 headers 的任务能完成、不带的失败；验证 `probe_url` 返回文件名和大小。

### 5.2 `crates/app`

- 新文件 `bridge.rs`：在 tokio runtime 上起 hyper 服务，持有 `Arc<Engine>` 和一个 `mpsc::Sender<BridgeEvent>`。`BridgeEvent::Received { url, file_name, size, headers }`，`BridgeEvent::Error(String)`。
- `app.rs`：新增 `poll_bridge()`，与 `poll_clipboard` 并列。静默模式 `engine.add` + 吐司"已接收浏览器任务 xxx"；询问模式调用 `add_dialog.open_with(...)`（需扩展 `AddDialog` 支持预填文件名和 headers）。
- `settings.rs` 新键：`bridge_enabled`（默认开）、`bridge_port`（默认 51230）、`bridge_mode`（`silent` / `ask`，默认 `silent`）。
- `ui/settings.rs` 新增一组"浏览器集成"：开关、端口、接收模式、当前状态（最近一次扩展 ping 的时间）。
- 端口被占用时吐司报错并关闭集成，不阻塞启动。

### 5.3 `extension/`

```
extension/
├── package.json          # esbuild、typescript、vitest、@types/chrome
├── tsconfig.json
├── esbuild.config.mjs    # 产出 dist/，--watch 开发
├── public/
│   ├── manifest.json     # MV3
│   ├── popup.html / options.html / welcome.html
│   └── icons/
└── src/
    ├── background.ts     # service worker：下载接管流程、右键菜单、ping 心跳
    ├── api.ts            # /ping、/tasks 封装
    ├── rules.ts          # 纯函数：是否拦截（协议、排除域名、阈值）
    ├── cookies.ts        # 取 Cookie 拼 header
    ├── popup.ts          # 总开关、连接状态、最近一次结果
    ├── options.ts        # 端口、排除域名、大小阈值、默认关闭
    ├── welcome.ts        # 欢迎页：连接检测、功能速览
    └── rules.test.ts
```

- `manifest.json` 权限：`downloads`、`cookies`、`contextMenus`、`storage`、`alarms`、`activeTab`、`scripting`、`webRequest`；`host_permissions`：`<all_urls>`（Cookie / 响应元数据）、`http://127.0.0.1/*`。仅在用户点击入口时执行网页链接提取；媒体监听为非阻塞只读，不申请 `webRequestBlocking`。
- 每次接管前都现 ping 一次（800ms 超时）再决定是否拿住下载，不依赖可能过期的心跳缓存；本机端口未监听时连接被拒是瞬时的。
- 交给 Swoop 的是 `item.url`（原始链接）而不是 `finalUrl`：重定向后的签名链接常常几分钟就过期，续传时需要从原始链接重新走一遍。
- 隐身窗口的下载不接管；右键菜单在隐身窗口里可用但不带 Cookie，避免把隐身会话写进 `swoop.db`。
- 右键菜单"用 Swoop 下载"：`contexts: ["link", "image", "video", "audio"]`，直接 POST，不经过 `downloads`。
- 徽标：已连接无标；断开显示灰点；上一次交接失败显示 `!`，popup 里给出原因。
- 排除域名默认空；大小阈值默认关闭，规则保持可预期。

### 5.4 本页链接与双浏览器构建

- 弹窗与网页右键菜单均提供「下载本页链接…」。仅读取主文档的 `a[href]`、`area[href]`，支持相对路径及 `<base>`，不扫描脚本、图片、音视频资源或 iframe。
- 来源页面仍须为 HTTP(S)，目标保留 HTTP(S) 和 FTP(S) 文件链接；删除片段后去重，排除当前页面。HTTP(S) 查询参数保留；FTP(S) 含查询参数时拒绝。默认不勾选，支持名称/网址搜索、扩展名过滤、当前结果全选/取消；网页标题一律按纯文本渲染。
- 并发数可配置 1–6（默认 2），去重后链接上限 100–10000（默认 2000）；扫描最多为该上限的 10 倍，达到限制时显示截断。每条调用原有 `/tasks`，显示真实响应；停止仅停止后续提交，已发出项等待响应。
- 成功与失败结果保留在 `storage.session`，刷新列表仍能识别成功项；同一批次同一链接并发提交共享请求，失败项可以手动重试。超时或响应丢失无法证明桌面端没有建任务，界面提示先检查桌面任务列表，不自动重试。
- 链接上下文最多保留 4 批、有效期 24 小时，浏览器退出清空；不在扩展 session 中保存 Cookie。原页面关闭、URL 变化、链接不属于该批次时拒绝新的提交。停用时入口和提交均受限，运行中的批次停止后续任务。
- `npm run build` 同时产出两个包；`npm run build:chromium`、`npm run build:firefox` 可单独构建。Firefox 使用 `background.scripts`、稳定 Gecko ID、数据传输声明；Chromium 使用 `background.service_worker`。开发监视默认只构建 Chromium。
- Chromium 通过扩展管理页加载 `dist/`；Firefox 开发验证通过 `about:debugging` 临时加载 `dist-firefox/manifest.json`。Firefox 正式分发签名与商店发布未完成。[Firefox 清单规则](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/manifest.json/browser_specific_settings)

### 5.5 网页媒体候选

- 通过 `webRequest.onHeadersReceived` 查看 HTTP(S) GET 的 200/206 响应元数据，以 URL 后缀或 Content-Type 识别普通音视频、HLS、DASH。只读观察在 MV3 可用；无需阻塞权限。[Chrome webRequest 文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)
- 不取响应体、不获取密钥/许可证、不自动建立下载。忽略后台请求（tabId < 0）、隐身标签、排除域名、失败响应、HTML/JSON、常见媒体分片和 `.key`。仅有响应元数据不能判断清单是否含 DRM，具体支持与保护检查由桌面媒体流程处理。
- 候选按标签页和完整请求 URL（去片段、保留查询签名）去重，最新检测置前；Range 响应从 Content-Range 取总大小，不把局部 Content-Length 当总大小。HLS/DASH 不传清单文本大小。
- 弹窗「网页媒体」显示当前页候选数，点击打开可筛选多选的媒体列表。普通音视频进入 HTTP 下载；HLS/DASH 附 `media` 字段进入桌面媒体窗口。实际提交前再次检查原页/总开关/媒体开关，成功项遵循已有批次防重规则。
- 可配置检测开关（默认开）、每页 10–500 条（默认 100）、保留 1–1440 分钟（默认 60）。总缓存再限制为 24 个标签页、2000 条、约 2 MiB 候选 JSON；达到上限优先清理较旧标签。关闭标签、导航离开、关闭检测或全局接管时清理；心跳及打开弹窗时清理过期记录。只存在浏览器 session，不保存响应体或 Cookie。

### 5.6 欢迎页

- `welcome.html` 在首次安装、或主/次版本号变化的更新时由后台打开（`rules.ts` 的 `shouldOpenWelcome`；0.x 阶段次版本即视为大版本）。补丁更新、浏览器更新不打开。弹窗顶部「使用指南」可随时再进入。
- 内容：桌面端下载入口（GitHub Releases）、实时连接检测、四项功能速览（自动接管、右键、本页链接、网页媒体）和小贴士。页头可切换语言（立即写入 `storage.sync.language`）并进入设置。
- 连接检测复用后台的 `refresh` 消息（同时刷新徽标）；未连接时每 3 秒重试，标签页不可见时暂停，切回立即检测；已连接后停止轮询。总开关关闭时提示到弹窗开启，不发起检测。

### 5.7 中英文界面

- 设置页可选择「简体中文 / English」，对应 `storage.sync.language = "zh-CN" / "en-US"`，默认中文。缺失或非法值回到中文；选择后先预览，点击「保存」持久化，重开页面与重启浏览器后恢复。
- 弹窗、链接/媒体选择页、设置页、工具栏状态、右键菜单及扩展自身错误支持双语；保存语言时已打开的页面同步刷新。切换语言保留筛选、选择、进行中的批次和提交结果。文件名、网页标题、URL 与外部服务返回的错误原文保留。
- 安装、浏览器启动、语言变更都会刷新右键菜单；重建按顺序执行，菜单 ID 与消息/下载协议不变。网页提取函数仍自包含，不依赖翻译模块或扩展闭包。
- 两种构建均携带 `_locales/zh_CN`、`_locales/en` 和 `default_locale: "zh_CN"`。浏览器扩展管理页的清单描述由浏览器自身语言选择；扩展页面和菜单使用上面的独立设置。[Chrome i18n 规则](https://developer.chrome.com/docs/extensions/reference/api/i18n)、[Firefox default_locale](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/manifest.json/default_locale)。

## 6. 边界情况

| 情况 | 处理 |
| --- | --- |
| Swoop 未运行 | 心跳失败 → 立即放行（Chromium `suggest` / Firefox 不 `pause`），浏览器正常下载 |
| 下载来自 POST 表单 | Swoop 用 GET 探测失败 → 4xx → 浏览器 `resume` |
| 需要登录 | Cookie 已透传；仍 401 → 回退浏览器 |
| 服务器不支持 Range | 探测成功（200 + 单连接）→ Swoop 单线程下载 |
| `blob:` / `data:` | 放行 |
| FTP(S) 文件 | 仅手动右键/本页链接提交，无 HTTP 上下文；自动下载事件放行 |
| Firefox `pause` 报错（已完成） | 放行 |
| 同一 URL 已在 Swoop 队列 | 同一页面批次已成功项不重复提交；不同批次、右键与自动接管不作全局去重 |
| 端口被其它程序占用 | 启动吐司报错，设置页显示原因并可改端口；不把开关写成关闭，下次启动端口空出来会自动恢复 |
| GET 重探拿到网页 / 大小不符 | 422 → 浏览器 `resume` |
| Firefox 下载在 `pause` 前已完成（本机/极快的小文件） | `pause` 报错 → 放行 |

## 7. 分阶段与验收

1. **core：headers 透传**。`cargo test --workspace` 新增用例通过；现有 db 升级后 `headers` 列存在且旧任务可加载。
2. **app：bridge 服务 + 设置 + 吐司**。`curl -X POST 127.0.0.1:51230/tasks` 能建任务；带 `Origin: https://evil.com` 被拒；设置页能切换模式和端口。
3. **extension：Chromium 版**。开发者模式加载 `extension/dist/`；验收清单：
   - 页内打开 PDF 不触发任何事；在阅读器点保存 → Swoop 接管。
   - 普通 `attachment` 下载 → 浏览器下载栏闪一下后消失，Swoop 吐司出现。
   - 退出 Swoop 后下载 → 浏览器正常完成。
   - 需要登录的站点（如 GitHub 私有 release）→ Swoop 下载成功。
   - 右键链接 → "用 Swoop 下载"可用。
4. **Firefox**。已产出 `background.scripts` 变体；`browser.*` 命名空间使用 `globalThis.browser ?? chrome`。需加载到 Firefox 实测权限授予、Cookie、右键菜单、暂停/接管/回退。

## 8. 验证记录（2026-10-11）

以下第一组为既有接管功能的历史记录，不能代替新增批量入口与 Firefox 的实测：

- `cargo test --workspace`：含带 Cookie 的端到端下载、`probe_url`、旧库补列、桥接的 Origin/Host/请求体/端口占用等用例。
- 实机：Swoop（隔离 HOME）+ curl：建任务、文件一致、恶意 Origin/Host 403、探测 404 → 422、端口被占用只报错不崩。
- 扩展：Chrome for Testing 无头加载 `extension/dist/`，限速本地服务器上点 `download` 链接：
  - Swoop 在线 → 浏览器下载被取消并清除，Swoop 下完与源文件逐字节一致，UA / Referer 已透传；
  - Swoop 未运行 → 浏览器正常下载，徽标灰点；
  - Swoop 在线但探测 403 → 浏览器下载恢复（`paused: false`），徽标 `!`，popup 显示原因。
- 尚未实测：真实登录站点（如 GitHub 私有 release）、页内 PDF 阅读器点保存、右键菜单、Chrome「下载前询问保存位置」开启时的行为、询问模式下的对话框（需要实际看界面）。

本轮新增功能验证：

- 中英文收尾验证：`cd extension && npm test` 共 10 个测试文件、65 项通过；`npm run typecheck` 通过。测试包含设置页真实 `change/click` 事件、存储失败、保存后模块重载恢复语言、弹窗状态与错误切换、运行中批次切换语言保留选择/停止/结果、菜单安装/启动/并发语言事件、静态文案及占位符覆盖、自包含注入函数执行。测试中实际运行 esbuild 的双目标构建，核验两包 manifest、语言 JSON、页面脚本与图标资源齐全。测试使用 DOM 和浏览器 API 替身，不作为浏览器实机验收。
- `cd extension && npm test`：8 个测试文件、53 个测试通过，覆盖链接提取/去重/过滤、真实 DOM 交互、总开关、并发与停止、失败重试、刷新后成功防重、后台入口权限、Cookie 容器选择、来源策略、接口数据、媒体识别/Range 大小/去重/过期/并发捕获/禁用/关标签清理、HLS 显式媒体交接/旧桌面能力保护与双 manifest。新增 FTP(S) 文件目标过滤、右键/本页链接的空请求头、无 Cookie 查询、来源页限制、凭据拒绝与记录脱敏；自动接管仍排除 FTP(S)。
- `npm run typecheck`、`npm run build`：通过；产出 Chromium `dist/` 与 Firefox `dist-firefox/`。
- `cargo test -p swoop-app --bin swoop --locked bridge::tests::`：12 项通过，含真实回环 FTP 控制服务的匿名探测和 Ask 事件（文件名/大小/空 headers），以及 FTP/FTPS 携 UA、Cookie、Referer、内嵌凭据或媒体标记时在联网前拒绝。无外部 FTP 依赖；此处不代表 FTPS TLS 握手或浏览器菜单已经实机验收。
- 后台测试使用浏览器 API 与 HTTP 响应替身，验证请求 JSON 与 Rust 桥接协议一致；不代表 Firefox、Windows 或新增批量入口已通过真实浏览器验收。
- 待验：macOS Chrome/Firefox 的新批量、手动 FTP(S) 和媒体入口、真实播放响应捕获、清单进入桌面媒体窗口、真实 Cookie 登录/容器/分区 Cookie、浏览器权限拒绝与服务工作线程回收、中英文菜单/页面实机切换；Windows 暂无机器，保留全部浏览器与桌面联动实机验收项。

## 9. 待定项

- 是否把探测结果（文件名、大小、ETag）顺手传进 `AddOptions`，避免 `add` 再探测一次。优化项，不影响协议。
- 默认端口号 `51230` 只是占位，没有和已知软件冲突即可。
- 扩展是否发布到 Chrome Web Store。不影响代码，v0.3 先以开发者模式加载。
