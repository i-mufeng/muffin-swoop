# Swoop

> 俯冲抓取。一个用 Rust + egui 写的高性能、好看、好用的下载器，目标是成为 IDM 的开源平替。

当前按 macOS / Windows 双平台对标 IDM；清单中的功能均已接入，平台实机与真实站点兼容验收单独记录。实现与验收范围见 [功能矩阵](docs/idm-parity.md)，桌面打包与实机检查见 [验收说明](docs/desktop-validation.md)。

## 当前功能

- **多连接分段下载**：默认 8 连接，设置中可调为 1–32；新建或继续任务时生效。文件预分配后各连接定位写入，无锁。
- **动态分段（IDM 招牌）**：某连接提前完成后，从剩余最多的区段"窃取"一半继续下，不会被尾部慢连接拖住。
- **HTTP 断点续传**：按区段记录进度，暂停/失败后从断点继续。
- **FTP / FTPS**：复用下载队列、限速、配额和同名保护；连接方式、TLS、代理、超时和 CA 可配置，支持会话登录。当前暂停重试从头下载，详见 [FTP 说明](docs/ftp.md)。
- **同名文件处理**：默认询问，支持覆盖、自动重命名、取消；设置中可保存默认策略，普通文件和媒体共用。覆盖先暂存，取消或失败保留原文件。见 [同名处理](docs/file-conflicts.md)。
- **全局与任务限速**：令牌桶，可运行时调整；文件下载配额按周期共享，重启保留用量。媒体和站点抓取使用各自的策略。
- **命名队列与计划**：队列并发、任务顺序、启动/停止时间、周期调度，重启保持；可启用完成文件的周期同步检查，改变后完整暂存并替换。
- **批量管理**：数字/字母 URL 范围展开、去重预览、文本/快捷方式导入、原生拖入链接与链接文件、拖出已完成文件，多选暂停/继续/移除记录、链接导出及持久化排序。
- **分类规则**：按扩展名和域名选择目录，规则可编辑和排序；默认关闭自动分目录，启用后作用于新任务。
- **任务属性**：描述、独立限速和刷新下载地址。换地址清除旧请求头，仅强 ETag 与大小一致时继续旧片段。
- **站点登录**：Basic、Digest、NTLM、Negotiate；可使用会话凭据、系统账户或系统凭据库，按精确来源隔离。企业域与系统授权验收见 [认证说明](docs/site-auth.md)。
- **网络策略**：连接/读取/总时长超时、重试与退避、HTTP(S)/SOCKS 代理、认证和排除主机。普通 HTTP(S) 的代理账号可保存在会话或系统凭据库，见 [代理认证](docs/proxy-auth.md)。跨源重定向移除站点凭据及自定义头。
- **界面**：egui 原生渲染，深/浅主题，卡片/表格切换，列显示、顺序和宽度可保存，工具栏可隐藏和排序，简体中文/English 可切换并保存；⌘N 新建、⌘V 粘贴链接直接添加。
- **详情面板**：点选任务从右侧滑出，最近 2 分钟速度曲线（平均/峰值）、续传支持、各区段进度；Esc 或再点一次收起。
- **菜单栏驻留**（macOS / Windows）：关闭窗口后隐藏到菜单栏继续下载（macOS 同时隐藏 Dock 图标），菜单里看总速度、全部开始/暂停、新建任务、退出；图标旁可显示速度。⌘Q 仍然直接退出。
- **系统通知**：下载完成／失败可分别开关，通知时机可选「仅后台」或「始终」；默认只在窗口失焦、最小化或隐藏时提醒，前台保留应用内吐司；通知声音独立配置，默认关闭。
- **完成后动作**：可选扫描、打开文件/目录和完成窗口；全部下载成功后可倒计时退出或请求关机，默认关闭并可取消。见 [动作说明](docs/completion-actions.md)。
- **用户级开机启动**：设置中显式启停，读取系统状态并提示失效路径；默认不启用，不要求管理员。
- **签名更新**：更新源、公钥、稳定/预览通道、自动检查和间隔可配置；下载验签、用户确认安装、备份与中断恢复。默认关闭，发行方需配置正式更新源，见 [更新说明](docs/updates.md)。
- **单实例与 CLI**：同一数据目录复用已有窗口；独立控制台入口可管理文件、媒体、抓取项目和队列，处理同名询问，命令不启动 GUI。见 [命令行说明](docs/cli.md)。
- **浏览器集成**：Chromium / Firefox 产物、下载接管与失败回退、本页链接筛选、被动媒体检测。候选由用户选择；HLS/DASH 交给独立媒体窗口。
- **媒体下载**：HLS、AES-128 HLS、无 DRM 的 DASH；清晰度/音轨/文本字幕选择，带时长上限的直播录制、MMSH/MMST，FFmpeg 转封装 MP4/MKV。HLS/DASH 点播与直播可用会话认证和显式代理；完整分片缓存经来源验证后复用。工具路径、重试、缓存预算、录制时长及默认质量/语言可配置。具体协议、字幕和恢复边界见 [媒体说明](crates/media/README.md)。
- **站点抓取**：HTML/CSS、SVG、网页清单、媒体/字幕引用与懒加载图片，域名/深度/类型/URL 过滤、robots、预算、恢复与离线镜像。可配置隔离 Chromium 渲染动态页面，也可导入渲染后的 HTML；浏览器另行安装，见 [抓取说明](docs/grabber.md)。
- **原生桌面主程序**：普通文件下载无需额外运行时；媒体下载另需 FFmpeg。
- **内嵌中文字体**：阿里巴巴普惠体 3.0 子集（GB2312 全部汉字 + 标点，约 1.5 MB），无需依赖系统字体；生僻字会显示为方框，详见 `assets/fonts/NOTICE.md`。

## 结构

```
crates/core   swoop-core  下载引擎（纯库，无 UI 依赖）
crates/app    swoop-app   egui 桌面 swoop + 控制台 swoop-cli（浏览器桥接 / 本机控制）
crates/media  swoop-media HLS/DASH/MMS 检查、可取消的 FFmpeg 下载
crates/grabber swoop-grabber 可恢复站点抓取与离线镜像
crates/ftp    swoop-ftp   可取消的 FTP/FTPS 传输
crates/updater swoop-updater 签名更新、安装与恢复
crates/i18n    swoop-i18n 桌面语言资源（编译期检查）
extension/    Chromium / Firefox MV3 扩展（TypeScript + esbuild）
docs/         设计文档（browser-extension.md）
assets/fonts  内嵌字体子集与声明
scripts/      make_font_subset.py：从本机普惠体重新生成字体子集
```

## 开发

使用 Rust 1.95 或更新版本（eframe / egui 0.36 的最低要求）；本轮实际使用 Rust 1.98.1，未单独运行最低版本工具链。

```bash
cargo run -p swoop-app            # 启动界面
python3 -m venv target/ftp-fixture-venv
target/ftp-fixture-venv/bin/python -m pip install -r crates/ftp/tests/requirements.txt
cargo test --workspace --locked  # 单元 + 本地 HTTP/FTP 服务器端到端测试
RUST_LOG=debug cargo run -p swoop-app
```

Windows 的 FTP 测试解释器设置见 [FTP 开发验证](docs/ftp.md#开发验证)。

调界面时的两个开关：

```bash
SWOOP_DEMO=1 cargo run -p swoop-app                     # 混入 8 条假任务，覆盖所有状态，并默认打开第一条的详情面板
SWOOP_THEME=light cargo run -p swoop-app                # 以浅色主题启动
SWOOP_PROGRESS=cells|lanes|overlay cargo run -p swoop-app   # 进度条样式（默认 cells，设置里也可切换）
SWOOP_DATA_DIR=tmp/manual-check cargo run -p swoop-app   # 独立任务/设置数据库，便于本地验收
# 自动截图后退出（需要 glow 后端）：
SWOOP_DEMO=1 SWOOP_RENDERER=glow EFRAME_SCREENSHOT_TO=tmp/shot.png \
  cargo run -p swoop-app --features screenshot
```

注意：自动截图抓的是第一帧，而 egui 的 `Area`（右下角吐司、设置/新建弹窗）在首帧总是隐藏，
所以截图里看不到它们，需要实际运行查看。

## 浏览器扩展

```bash
cd extension
npm install
npm run build       # Chromium: dist/；Firefox: dist-firefox/
npm run package     # 构建并打包为 target/packages/swoop-extension-{chromium,firefox}-<版本>.zip（附 .sha256）
npm test            # 规则等纯函数的单元测试
npm run typecheck
```

Chrome / Edge / Arc / Brave：打开 `chrome://extensions` → 开启「开发者模式」→「加载已解压的扩展程序」→ 选 `extension/dist/`。

- Swoop 侧在「设置 → 浏览器集成」里开关、改端口（默认 51230）、选择「直接下载」或「弹出确认」，并能看到扩展是否已连接。
- 「弹出确认」在窗口隐藏到菜单栏时会唤回窗口，再显示预填好的新建任务对话框。
- 下载接管在 Chromium 上监听 `downloads.onDeterminingFilename`（文件名确定前拿住，接手成功就取消，浏览器下载面板不弹出），Firefox 监听 `downloads.onCreated` 先暂停；Swoop 探测成功才取消，否则放行。被动媒体检测只记录候选，不自动开始下载。
- 右键链接/图片/视频 →「用 Swoop 下载」。
- 扩展弹窗可打开「本页全部链接」或媒体选择页；缓存上限、检测开关、过期时间、批量并发等均可配置。
- `npm run package` 产出的 zip 以 `manifest.json` 为包根，可直接上传 Chrome Web Store / AMO；版本取 `package.json`，与 `public/manifest.json` 不一致时报错。同版本重复打包会覆盖，且产物逐字节一致。
- Firefox 本地测试加载 `extension/dist-firefox/manifest.json`；浏览器商店签名与发布尚未执行。
- 协议与安全边界见 `docs/browser-extension.md`。

## 数据与设置

- 数据库：`~/Library/Application Support/com.muffin.swoop/swoop.db`（SQLite，WAL）。
- 任务新增/状态变化即时落盘，下载中每秒刷新一次区段表；重启后按设置自动续传，续传前比对 ETag 与文件大小，变了就从头下。
- 设置、分类、网络策略、媒体历史及最近抓取项目存在同一个库的 `settings` 表；队列及任务属性单独持久化。普通设置成组事务保存，失败会显示错误。
- 浏览器交来的任务会把 Cookie 等请求头明文存进任务表（重启后续传需要），数据库只在本用户目录下。
- 剪贴板监听：每秒轮询一次，只对启动后新复制的 http(s) 链接弹吐司（可在设置里关闭）；窗口隐藏到菜单栏时暂停轮询。

配置入口和默认值汇总见 [配置说明](docs/configuration.md)。以下为常用项：

| 设置 | 默认值 | 生效时机 |
| --- | --- | --- |
| 每任务连接数 | 8（可调 1–32） | 新建或暂停后继续；保留已有分段进度，不改变正在运行的连接数 |
| 完成通知 | 开 | 后续完成事件 |
| 失败通知 | 开 | 后续失败事件；暂停不提醒 |
| 通知时机 | 仅后台 | 后续事件；可选始终 |
| 通知声音 | 关 | 后续系统通知 |
| 完成后扫描/打开/弹窗 | 关 | 后续成功下载，扫描先于打开 |
| 全部完成后退出/关机 | 关；倒计时 30 秒 | 新完成事件后且所有工作成功，可取消 |
| 列表显示和密度 | 完整信息、舒适密度 | 立即生效并保存 |

系统通知只显示文件名，不携带错误里的 URL 或请求头；具体错误仍在应用内查看。
通知发送在独立线程执行，系统拒绝或繁忙不影响下载，失败原因写入日志。
「测试通知」用于确认本机系统权限：系统禁用横幅或专注模式可能让通知不可见。
macOS 使用 `cargo run` 或直接运行裸二进制时没有应用容器，通知借用「终端」的系统标识和权限；
在已注册的 `.app` 中运行时使用该应用自己的标识。Windows / Linux 的系统展示需在对应桌面环境验收。

通知策略与设置持久化测试包含在 `cargo test --workspace` 中；真实系统发送测试默认跳过，手动执行：

```bash
cargo test -p swoop-app notifications::tests::native_notification_smoke -- --ignored --nocapture
```

该命令成功仅代表通知 API 调用成功；横幅是否显示仍需实际查看。

## 路线图

以 [IDM 功能矩阵](docs/idm-parity.md) 为准。已实现项与验收范围逐项记录；不会把本地夹具通过视为所有网站、媒体格式、企业环境或系统集成都兼容。

Windows 暂无实机，保留验收项；macOS 原生交互与通知横幅未完成自动验收。当前 CI 配置已提供，未把本地测试当成远端 CI 已运行。
