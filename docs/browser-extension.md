# 浏览器扩展方案（v0.3）

状态：已确认方向，待实现。决策记录（2026-10-11）：

| 决策点 | 结论 |
| --- | --- |
| 目标浏览器 | Chromium 系优先（Chrome / Edge / Arc / Brave 同一个 MV3 包），Firefox 第 4 阶段按需适配，Safari 不做 |
| 接收行为 | 可配置，默认静默加入 + 吐司；可切换为弹出新建对话框 |
| 通信方式 | Swoop 内置本地 HTTP 服务（`127.0.0.1`），不用 Native Messaging |
| 扩展技术 | TypeScript + esbuild，产物目录 `extension/dist/` |

## 1. 目标与非目标

**要解决的 IDM 痛点**

- 页内打开 PDF / 图片 / 视频被抢走变成下载。
- 每次下载都弹对话框。
- 抓流媒体分片、几十 KB 小文件，徒增噪音。
- 登录后的下载链接拿不到 Cookie 而失败。

**非目标（v0.3 不做）**

- 视频嗅探 / 流媒体下载面板。
- 内容脚本劫持页面链接点击。
- Swoop 未运行时由扩展拉起 Swoop（本地 HTTP 做不到，留给以后的 Native Messaging 可选项）。
- 按住修饰键绕过拦截（`downloads` API 不暴露按键状态）。

## 2. 拦截点：只接管浏览器自己判定为下载的请求

IDM 工作在网络层（响应头 + 扩展名），所以 `Content-Type: application/pdf` 但没有 `Content-Disposition: attachment` 的页内 PDF 也会被抓。

Swoop 扩展只监听 `chrome.downloads.onCreated`。这个事件只在浏览器**已经决定要下载**时触发：`attachment`、用户"另存为"、浏览器无法渲染的类型。页内渲染 PDF、播放视频、显示图片都不经过它，所以机制上不会误抓。用户在 PDF 阅读器里点"保存"，才会产生一次下载事件，这时才交给 Swoop。

Chrome MV3 已经移除普通扩展的阻塞式 `webRequest`，网络层拦截对我们本来也不可行。

## 3. 交接协议：先暂停，确认成功再取消

```
onCreated(item)
  ├─ 过滤：总开关 / 仅 http(s) / 排除域名 / 大小阈值 / Swoop 在线
  ├─ downloads.pause(item.id)
  ├─ POST /tasks {url, headers, referrer, suggested_name}
  │     Swoop 侧：探测（上限 10s）→ 成功 200 / 失败 4xx
  ├─ 200 → downloads.cancel + downloads.erase   （浏览器下载消失，Swoop 接管）
  └─ 4xx / 网络错误 → downloads.resume          （浏览器继续，用户无感）
```

要点：

- **失败一定可回退**。`pause` 之后浏览器下载还在，只有 Swoop 探测成功才 `cancel`。POST 表单结果、`401`、服务器拒绝 Range 探测等情况都回到浏览器。
- `blob:` / `data:` 直接放行；`pause` 失败（小文件已完成）直接放行。
- **携带浏览器上下文**：扩展用 `cookies` 权限取该 URL 的全部 Cookie（含 HttpOnly），连同 `Referer`、浏览器 `User-Agent` 一起传给 Swoop。这是登录后下载成功的关键。
- 两种接收模式对扩展透明：Swoop 都先探测，成功后"静默"模式直接 `engine.add`，"询问"模式把探测结果塞进 channel，UI 下一帧弹出预填好文件名/大小的新建对话框。扩展只看 200。

## 4. 本地 HTTP 接口

绑定 `127.0.0.1:51230`（可在设置里改），hyper 实现（reqwest 已依赖 hyper，编译成本近零）。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/ping` | `{ "app": "swoop", "version": "0.3.0" }`，扩展每 30s 轮询一次更新徽标 |
| `POST` | `/tasks` | 请求体见下；200 返回 `{ "id": 12, "file_name": "...", "size": 1234 }`，4xx 返回 `{ "error": "..." }` |

```json
{
  "url": "https://example.com/file.zip",
  "referrer": "https://example.com/page",
  "suggested_name": "file.zip",
  "headers": [["Cookie", "a=1; b=2"], ["User-Agent", "Mozilla/5.0 ..."]]
}
```

**安全边界**

- 只绑定回环地址。
- `Host` 必须是 `127.0.0.1` / `localhost`（防 DNS rebinding）。
- `Origin` 若存在且为 `http(s)://` 一律拒绝；允许 `chrome-extension://` / `moz-extension://` 和无 Origin（本机脚本）。网页无法伪造 Origin，Chrome 的 Private Network Access 也会拦网页访问 localhost。
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
│   ├── popup.html / options.html
│   └── icons/
└── src/
    ├── background.ts     # service worker：onCreated 流程、右键菜单、ping 心跳
    ├── api.ts            # /ping、/tasks 封装
    ├── rules.ts          # 纯函数：是否拦截（协议、排除域名、阈值）
    ├── cookies.ts        # 取 Cookie 拼 header
    ├── popup.ts          # 总开关、连接状态、最近一次结果
    ├── options.ts        # 端口、排除域名、大小阈值、默认关闭
    └── rules.test.ts
```

- `manifest.json` 权限：`downloads`、`cookies`、`contextMenus`、`storage`；`host_permissions`：`<all_urls>`（取 Cookie）、`http://127.0.0.1/*`。
- 右键菜单"用 Swoop 下载"：`contexts: ["link", "image", "video", "audio"]`，直接 POST，不经过 `downloads`。
- 徽标：已连接无标；断开显示灰点；上一次交接失败显示 `!`，popup 里给出原因。
- 排除域名默认空；大小阈值默认关闭，规则保持可预期。

## 6. 边界情况

| 情况 | 处理 |
| --- | --- |
| Swoop 未运行 | 心跳失败 → 不 `pause`，浏览器正常下载 |
| 下载来自 POST 表单 | Swoop 用 GET 探测失败 → 4xx → 浏览器 `resume` |
| 需要登录 | Cookie 已透传；仍 401 → 回退浏览器 |
| 服务器不支持 Range | 探测成功（200 + 单连接）→ Swoop 单线程下载 |
| `blob:` / `data:` | 放行 |
| `pause` 报错（已完成） | 放行 |
| 同一 URL 已在 Swoop 队列 | v0.3 不去重，照常新建；后续可加 |
| 端口被其它程序占用 | 启动吐司报错，设置页可改端口 |

## 7. 分阶段与验收

1. **core：headers 透传**。`cargo test --workspace` 新增用例通过；现有 db 升级后 `headers` 列存在且旧任务可加载。
2. **app：bridge 服务 + 设置 + 吐司**。`curl -X POST 127.0.0.1:51230/tasks` 能建任务；带 `Origin: https://evil.com` 被拒；设置页能切换模式和端口。
3. **extension：Chromium 版**。开发者模式加载 `extension/dist/`；验收清单：
   - 页内打开 PDF 不触发任何事；在阅读器点保存 → Swoop 接管。
   - 普通 `attachment` 下载 → 浏览器下载栏闪一下后消失，Swoop 吐司出现。
   - 退出 Swoop 后下载 → 浏览器正常完成。
   - 需要登录的站点（如 GitHub 私有 release）→ Swoop 下载成功。
   - 右键链接 → "用 Swoop 下载"可用。
4. **可选：Firefox**。`background.scripts` 变体由 esbuild 产出第二份 manifest；`browser.*` 命名空间用 `globalThis.browser ?? chrome` 兼容。

实施顺序上，阶段 2 会改 `app.rs` / `main.rs` / `settings.rs`，这些文件当前有未提交的详情面板改动，先等那部分提交再动。

## 8. 待定项

- 是否把探测结果（文件名、大小、ETag）顺手传进 `AddOptions`，避免 `add` 再探测一次。优化项，不影响协议。
- 默认端口号 `51230` 只是占位，没有和已知软件冲突即可。
- 扩展是否发布到 Chrome Web Store。不影响代码，v0.3 先以开发者模式加载。
