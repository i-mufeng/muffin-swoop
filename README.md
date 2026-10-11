# Swoop

> 俯冲抓取。一个用 Rust + egui 写的高性能、好看、好用的下载器，目标是成为 IDM 的开源平替。

## 特性（v0.1 骨架）

- **多连接分段下载**：默认 8 连接，文件预分配后各连接定位写入，无锁。
- **动态分段（IDM 招牌）**：某连接提前完成后，从剩余最多的区段"窃取"一半继续下，不会被尾部慢连接拖住。
- **断点续传**：按区段记录进度，暂停/失败后从断点继续。
- **全局限速**：令牌桶，可运行时调整。
- **界面**：egui 原生渲染，深/浅主题，分段进度图，⌘N 新建、⌘V 粘贴链接直接添加。
- **详情面板**：点选任务从右侧滑出，最近 2 分钟速度曲线（平均/峰值）、续传支持、各区段进度；Esc 或再点一次收起。
- **菜单栏驻留**（macOS / Windows）：关闭窗口后隐藏到菜单栏继续下载（macOS 同时隐藏 Dock 图标），菜单里看总速度、全部开始/暂停、新建任务、退出；图标旁可显示速度。⌘Q 仍然直接退出。
- **浏览器扩展接管下载**（Chromium 系）：只接管浏览器自己判定为下载的请求，带上 Cookie / Referer / UA；Swoop 没开或探测失败时自动交回浏览器，用户无感。
- **单二进制**，启动即开，内存占用远低于 WebView 方案。
- **内嵌中文字体**：阿里巴巴普惠体 3.0 子集（GB2312 全部汉字 + 标点，约 1.5 MB），无需依赖系统字体；生僻字会显示为方框，详见 `assets/fonts/NOTICE.md`。

## 结构

```
crates/core   swoop-core  下载引擎（纯库，无 UI 依赖）
crates/app    swoop-app   egui 桌面界面，二进制名 swoop（含浏览器桥接的本地 HTTP 服务）
extension/    Chromium MV3 扩展（TypeScript + esbuild）
docs/         设计文档（browser-extension.md）
assets/fonts  内嵌字体子集与声明
scripts/      make_font_subset.py：从本机普惠体重新生成字体子集
```

## 开发

```bash
cargo run -p swoop-app            # 启动界面
cargo test --workspace            # 单元 + 本地 HTTP 服务器端到端测试
RUST_LOG=debug cargo run -p swoop-app
```

调界面时的两个开关：

```bash
SWOOP_DEMO=1 cargo run -p swoop-app                     # 混入 8 条假任务，覆盖所有状态，并默认打开第一条的详情面板
SWOOP_THEME=light cargo run -p swoop-app                # 以浅色主题启动
SWOOP_PROGRESS=cells|lanes|overlay cargo run -p swoop-app   # 进度条样式（默认 cells，设置里也可切换）
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
npm run build       # 产物在 extension/dist/；npm run watch 边改边打包
npm test            # 规则等纯函数的单元测试
npm run typecheck
```

Chrome / Edge / Arc / Brave：打开 `chrome://extensions` → 开启「开发者模式」→「加载已解压的扩展程序」→ 选 `extension/dist/`。

- Swoop 侧在「设置 → 浏览器集成」里开关、改端口（默认 51230）、选择「直接下载」或「弹出确认」，并能看到扩展是否已连接。
- 扩展只监听 `chrome.downloads.onCreated`：页内打开的 PDF / 图片 / 视频不会被抓；先暂停浏览器下载，Swoop 探测成功才取消，否则恢复。
- 右键链接/图片/视频 →「用 Swoop 下载」。
- 协议与安全边界见 `docs/browser-extension.md`。

## 数据与设置

- 数据库：`~/Library/Application Support/com.muffin.swoop/swoop.db`（SQLite，WAL）。
- 任务新增/状态变化即时落盘，下载中每秒刷新一次区段表；重启后按设置自动续传，续传前比对 ETag 与文件大小，变了就从头下。
- 设置（主题、默认目录、进度条样式、限速、剪贴板监听、自动续传）存在同一个库的 `settings` 表。
- 浏览器交来的任务会把 Cookie 等请求头明文存进任务表（重启后续传需要），数据库只在本用户目录下。
- 剪贴板监听：每秒轮询一次，只对启动后新复制的 http(s) 链接弹吐司（可在设置里关闭）；窗口隐藏到菜单栏时暂停轮询。

## 路线图

- v0.1 骨架：引擎（分段/续传/限速）+ 基础列表 UI ✅
- v0.2 好用：任务持久化（SQLite）✅、剪贴板监听吐司 ✅、设置持久化 ✅、完成/失败吐司 ✅、详情面板（速度曲线）✅、托盘 ✅、系统通知
- v0.3 生态：浏览器扩展接管下载 ✅（Chromium）、CLI、Firefox 适配
- v0.4+：HLS/m3u8 视频、BT/磁力（librqbit）
