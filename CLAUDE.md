# Swoop（muffin-swoop）

Rust workspace，IDM 平替下载器。GUI 用 egui/eframe（wgpu），引擎自研。协议：HTTP/HTTPS、FTP/FTPS 走引擎；HLS/DASH/MMS 由 `swoop-media` 交给外部 FFmpeg 转封装；另有站点抓取。功能边界见 `docs/idm-parity.md`。

## 结构与边界

- `crates/core`（`swoop-core`）：下载引擎，**禁止依赖任何 UI crate**。对外 API：`Engine`、`AddOptions`、`TaskSnapshot`、`Event`。
- `crates/app`（`swoop-app`）：二进制 `swoop`（egui 界面）和 `swoop-cli`（控制台入口，通过单实例通道控制已运行的程序，不启动 GUI，见 `docs/cli.md`）。界面每帧调用 `engine.snapshot()` 读取只读快照，不直接碰引擎内部状态。
- `crates/media`（HLS/DASH/MMS 检查与 FFmpeg 下载）、`crates/grabber`（站点抓取）、`crates/ftp`（FTP/FTPS 传输，`core` 依赖它）、`crates/updater`（签名更新）：同样禁止依赖 UI crate。各自说明见 `crates/media/README.md`、`docs/grabber.md`、`docs/ftp.md`、`docs/updates.md`。
- 界面文案统一写 `swoop_i18n::tr!("中文原文")`，英文译文加到 `crates/i18n/locales/en-US.json`；缺译文编译期报错（`crates/i18n-macros`）。删文案时顺手删掉对应译文。
- `crates/app/src/bridge.rs`：浏览器桥接，只绑 127.0.0.1 的 hyper 服务（`/ping`、`/tasks`），协议见 `docs/browser-extension.md`；改协议要同步改 `extension/src/api.ts`。
- `extension/`：MV3 扩展（TypeScript + esbuild），`npm run build` 同时产出 Chromium 的 `extension/dist/` 和 Firefox 的 `extension/dist-firefox/`，`npm run package` 再打成 `target/packages/` 下的商店 zip；规则类逻辑写成纯函数放 `rules.ts` 并在 `rules.test.ts` 加测试。
- 品牌图标源文件在 `assets/brand/swoop`（应用图标、托盘、扩展图标都由此生成）；`assets/brand/logo-*/` 是概念稿，已加入 gitignore，不入库。
- 颜色/间距只在 `crates/app/src/theme.rs` 定义（`Palette`），UI 代码不写裸色值。
- 图标用 `egui_phosphor::regular::*`；中文字体内嵌 `assets/fonts/AlibabaPuHuiTi-3-55-Regular.subset.otf`（GB2312 子集，`include_bytes!`），不要在运行时加载系统字体（整字体几十 MB 常驻内存）。要扩充字符集改 `scripts/make_font_subset.py` 重新生成。

## 引擎关键约定

- 区段为半开区间 `[start, end)`，`end == u64::MAX` 表示大小未知。
- 写入前先在 `SegmentMap` 锁内预订范围再落盘，防止与动态分段窃取方重叠。
- 暂停 = `pause()` 先置 `Paused` 再 cancel；`spawn_run` 的收尾按状态区分"用户暂停"与"失败"。
- 持久化在 `core/src/store.rs`（rusqlite bundled）：`Engine::with_store` + `Engine::load(auto_resume)`；引擎负责写任务表，app 层通过 `Store::get_setting/set_setting` 写设置。改 schema 用 `CREATE TABLE IF NOT EXISTS` + 新列加默认值，保持向后兼容。
- 新增协议/功能请先加测试：单元测试在模块内，端到端测试在 `crates/core/tests/`（自带支持 Range 的本地 HTTP 服务器，不依赖外网）。认证、代理和 FTP 夹具是 `crates/*/tests/` 下的 Python 本地服务，要先装 `crates/ftp/tests/requirements.txt`（见下方命令）；依赖真实 FFmpeg 的媒体测试默认 `#[ignore]`。

## UI 约定

- 弹窗统一用 `ui/dialog.rs` 骨架：`dialog::modal`（非模态用 `dialog::window`）+ `header` + `body` + `group`/`row` + `footer`；footer 是右到左布局，主按钮先加（在最右），删除/覆盖等用 `danger_button` 放左侧。`popup::show` 的独立窗口有系统标题栏，用 `dialog::heading`。
- 控件：复选框用 `ui::checkbox` / `ui::Check`（egui 自带的沿用控件圆角，16px 方框会被画成圆）；成组单选用 `ui::segmented`，开关用 `ui::toggle`，整行导航用 `ui::nav_item`，顶栏按钮用 `ui::tool_button` 配 `egui::Popup::menu`。
- 主题只通过 `Palette::apply` 切换：它同时写 egui 的 dark/light 两套样式、锁定主题并同步原生标题栏；不要直接 `ctx.set_visuals`（只改当前那套，系统主题与应用主题不同时控件会用回 egui 默认样式）。
- 侧栏只放任务筛选、队列、分类和设置；媒体下载、站点抓取等进阶入口放顶栏「更多操作」（可在「视图 → 工具栏」固定到顶栏）。
- 吐司在 `ui/toast.rs`，右下角 `Area`；引擎事件（完成/失败）和剪贴板链接都走它，不要再加第二套应用内提示机制。系统通知（`notifications.rs`）只在窗口不在前台时补充提醒，不替代吐司。
- 看界面效果用 `scripts/screenshot.sh <视图> [dark|light]`（`--features screenshot`，视图列表见 `crates/app/src/shot.rs`）：打开指定弹窗或设置分类后截主窗口，吐司、Modal 都在，独立窗口（新建下载、进度/完成窗口）画在主窗口中央；用临时数据目录和演示数据，输出到 `target/screenshots/`，会短暂弹出窗口。eframe 自带的 `EFRAME_SCREENSHOT_TO` 只抓首帧，看不到任何 `Area`。
- `SWOOP_DEMO=1` 混入假任务与假吐司，只在 UI 层拼 `TaskSnapshot`，不经过引擎。
- 非绘制的每帧逻辑（刷新快照、轮询引擎/剪贴板/桥接/托盘事件、关窗拦截）放 `App::logic`，不放 `ui`：窗口隐藏到菜单栏（`tray.rs`）时 eframe 不跑 `ui`，只在有 repaint 请求时调 `logic`。后台线程要唤醒界面就 `ctx.request_repaint()`。
- 截图用默认窗口尺寸 1080×700，弹窗主体超出时会滚动；要看下半部分只能临时调整内容顺序。

## 常用命令

```bash
cargo run -p swoop-app
python3 -m venv target/ftp-fixture-venv && target/ftp-fixture-venv/bin/python -m pip install -r crates/ftp/tests/requirements.txt  # 首次
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings   # 与 CI 一致，警告即失败
cd extension && npm test && npm run typecheck && npm run package   # package 含 build
bash scripts/screenshot.sh settings:download light   # 截图看界面，输出 target/screenshots/
bash scripts/package-macos.sh   # zip + DMG；Windows 用 scripts/package-windows.ps1 出 zip + NSIS setup.exe；需先 cargo install cargo-packager --version 0.11.8 --locked
```

CI（`.github/workflows/ci.yml`）在 macOS 和 Windows 上跑以上检查并打包；Windows 只能靠 CI 验证。

## 交流

- 简体中文；回复以"好的沐风。"开头。
- 涉及范围/方案/不可逆操作先问再做。
