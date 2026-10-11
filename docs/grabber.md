# 站点抓取与动态页面

入口为工具栏「站点抓取」。项目、原始对象与镜像保存在用户选择的目录；每次生成独立镜像，不覆盖旧镜像和其他文件。支持 HTTP(S) 公开页面，默认仅起始 origin（协议、主机和端口均相同），可增加精确允许域名。

## 可配置行为

| 选项 | 默认 | 含义 |
| --- | --- | --- |
| 深度 / 页面与资源总数 / 并发 | 2 / 200 / 4 | 页面导航增加深度，引用资源和递归 CSS 保持当前深度 |
| 抓取预算 / 单文件 / 请求超时 | 100 MiB / 16 MiB / 20 秒 | 原始抓取、robots 和导入 DOM 累计，重启不清零 |
| robots | 开启 | 网络抓取的每次请求及重定向检查 |
| 重定向上限 | 5 | 可设 0–20 |
| 资源扩展名 | 不限制 | 对引用资源过滤；页面导航不受扩展名过滤 |
| 脚本 / 音视频与字幕 / 嵌入文档 / 网页清单 / 懒加载图片 | 开启 | 控制后续链接发现；已有下载对象不会被删除 |
| 从镜像移除 script 元素 | 关闭 | 只删除 script 标签；事件属性等仍保留，不是 HTML 安全过滤或隔离执行 |
| URL 排除 | 空 | 每行一条，`*` 任意字符、`?` 一个字节；`/` 开头匹配路径和查询，其他匹配完整 URL；区分大小写 |

所有选项随项目保存，桌面最近项目和新项目默认值保存在应用设置中。未知旧字段缺省为兼容值。

## 资源范围

除 HTML 链接、iframe、脚本、CSS 的 `url()`/`@import`、图片和 srcset 外，还处理：

- CSS `image-set()` / `-webkit-image-set()` 的 URL 字符串（`type("image/png")` 不当成资源）。
- 内联及独立 SVG 的 image/use/feImage 的 href、xlink:href，保留本地 `#id`，递归处理 SVG 内样式引用。
- input 图片按钮、传统 background 属性、音视频 source/poster、track 字幕、object/embed。
- 常见 data-src、data-original、data-srcset 懒加载图片属性。重写属性不代替页面自身的懒加载逻辑。
- `link rel=manifest` 声明的 JSON（不要求 `.webmanifest` 后缀），以及正确声明类型的 webmanifest；处理图标、截图、start_url、快捷方式页面与图标。scope/id 保留线上绝对标识，不注册 Service Worker。

成功引用改为本地相对路径，未下载/失败/超界引用保留绝对线上地址；HTML base 元素移除。独立 SVG 采用 HTML 兼容解析，不支持任意 XML DTD、实体扩展、xml:base 和 XML 专用编码声明。媒体仅抓文件引用；流媒体分段和受保护播放请使用媒体下载功能。模块脚本 import、运行时 API 缓存、canvas 像素、blob URL、完整 PWA/SPA 离线应用不属于静态镜像能力。

## 可选自动动态渲染

「动态页面 → 使用隔离浏览器渲染」默认关闭。启用后须显式填写本机 Chrome、Chromium 或 Edge 的可执行文件绝对路径，例如 macOS 的 `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`，Windows 则选择实际安装的 `chrome.exe`/`msedge.exe`。浏览器不是随应用打包的依赖，缺失或启动失败会记录该页面失败，不退回静态 HTML 冒称渲染成功。

| 渲染选项 | 默认 | 范围 |
| --- | --- | --- |
| 渲染范围 | 仅首页 | 可启用每个 HTML 页面 |
| 虚拟时间预算 | 3000 ms | 100–60000 ms；不等价于全部网络已空闲 |
| 总超时 | 30 秒 | 1–120 秒 |
| DOM 大小 | 8 MiB | 1 字节–64 MiB，同时受项目单文件上限约束 |
| 浏览器网络预算 | 100 MiB | 单独累计和显示，重启继续累计；HTTPS 包括 TLS 开销 |

每次渲染创建新的临时用户目录，保留 Chromium sandbox，关闭扩展/同步和常见后台联网；不读取现有浏览器 profile，不传入应用登录凭据或 Cookie。页面 JavaScript 会执行。渲染完成后抓取的是实际输出 DOM 中的资源，DOM 入账项目字节预算；成功浏览器快照可恢复复用，修改渲染配置后重新渲染所选范围。

浏览器通过绑定 127.0.0.1 随机端口的有界代理访问网络，关闭代理回退、QUIC 和非代理 WebRTC UDP，并阻止直接 DNS。代理最多 32 条连接，只接纳配置允许的 origin/额外域。HTTP 只允许 GET/HEAD，继续检查 URL 排除和 robots，且不传 Cookie/Authorization。HTTPS 使用 CONNECT 加密隧道，仅能检查目标 origin，**无法检查隧道内路径、方法、Cookie、robots 和排除规则**；新 profile 仍可接收当前页面产生的匿名会话 Cookie。浏览器流量和 robots 等抓取流量分别计数，不声称捕获器预算是网卡流量精确上限，也不将这些 Chromium 开关视为操作系统网络沙箱。

取消、超时、输出过大和预算耗尽都会终止本次浏览器进程组/进程树并等待直接子进程回收，然后停止代理连接。失败页面有明确错误，不生成伪成功 DOM。脚本导航到不同 URL、运行期登录/挑战、直播应用、Service Worker 离线缓存仍不保证可镜像；对需要精确最终 URL 的页面使用下方手动快照方式。

## 导入已渲染 HTML

已有项目可在「动态页面」选择项目中的 URL，再点「导入已渲染 HTML」。选择 UTF-8 HTML 文件后，应用在后台导入并继续抓取其中资源。应导入浏览器渲染后的 DOM，浏览器“查看源代码”的 HTML 不代表渲染结果。文件内容会原样写入项目，导入前自行移除不希望保存的表单值等页面内容。

库 API：`Grabber::import_rendered_html(url, html)`。该 API 不启动浏览器、不执行 JS、不导入浏览器凭据；URL 必须已在项目中且仍满足允许域/排除配置，受项目锁、页数/深度及字节预算约束。重复导入相同快照不会重复计费；新快照保留旧对象与旧镜像，下一次 `run` 生成新镜像。`PageRecord.source` 区分 `Network`、`RenderedSnapshot` 与 `BrowserRendered`，旧项目默认 `Network`。

## 验证与保留项

普通回归：`cargo test -p swoop-grabber --locked`；实际本地 HTTP 夹具覆盖资源引用、配置过滤、跨 origin/重定向、预算、取消、恢复、快照导入和损坏 manifest。

实际 Chrome 回归（须显式选择本机浏览器）：

```sh
SWOOP_GRABBER_CHROMIUM='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
  cargo test -p swoop-grabber --locked --test http -- --ignored --nocapture
```

2026-10-11 macOS 的 3 项真实 Chrome 测试通过：JS 动态节点形成并进入镜像、跨 origin 请求未到达服务端；无限 JS 超时、DOM/网络预算错误；运行中取消后项目锁可重新使用。默认测试不自动启动浏览器。Windows 浏览器启动、进程树终止、代理兼容及原生文件选择仍保留实机验收；HTTPS CONNECT 的站点兼容、任意线上网站/PWA 完整离线不在这些本地夹具的证明范围。
