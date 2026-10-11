# Swoop（muffin-swoop）

Rust workspace，IDM 平替下载器。GUI 用 egui/eframe（wgpu），引擎自研，协议 v0.1 仅 HTTP/HTTPS。

## 结构与边界

- `crates/core`（`swoop-core`）：下载引擎，**禁止依赖任何 UI crate**。对外 API：`Engine`、`AddOptions`、`TaskSnapshot`、`Event`。
- `crates/app`（`swoop-app`，二进制 `swoop`）：egui 界面。每帧调用 `engine.snapshot()` 读取只读快照，不直接碰引擎内部状态。
- `crates/app/src/bridge.rs`：浏览器桥接，只绑 127.0.0.1 的 hyper 服务（`/ping`、`/tasks`），协议见 `docs/browser-extension.md`；改协议要同步改 `extension/src/api.ts`。
- `extension/`：Chromium MV3 扩展（TypeScript + esbuild），`npm run build` 产出 `extension/dist/`；规则类逻辑写成纯函数放 `rules.ts` 并在 `rules.test.ts` 加测试。
- 颜色/间距只在 `crates/app/src/theme.rs` 定义（`Palette`），UI 代码不写裸色值。
- 图标用 `egui_phosphor::regular::*`；中文字体内嵌 `assets/fonts/AlibabaPuHuiTi-3-55-Regular.subset.otf`（GB2312 子集，`include_bytes!`），不要在运行时加载系统字体（整字体几十 MB 常驻内存）。要扩充字符集改 `scripts/make_font_subset.py` 重新生成。

## 引擎关键约定

- 区段为半开区间 `[start, end)`，`end == u64::MAX` 表示大小未知。
- 写入前先在 `SegmentMap` 锁内预订范围再落盘，防止与动态分段窃取方重叠。
- 暂停 = `pause()` 先置 `Paused` 再 cancel；`spawn_run` 的收尾按状态区分"用户暂停"与"失败"。
- 持久化在 `core/src/store.rs`（rusqlite bundled）：`Engine::with_store` + `Engine::load(auto_resume)`；引擎负责写任务表，app 层通过 `Store::get_setting/set_setting` 写设置。改 schema 用 `CREATE TABLE IF NOT EXISTS` + 新列加默认值，保持向后兼容。
- 新增协议/功能请先加测试：单元测试在模块内，端到端测试在 `crates/core/tests/`（自带支持 Range 的本地 HTTP 服务器，不依赖外网）。

## UI 约定

- 吐司在 `ui/toast.rs`，右下角 `Area`；引擎事件（完成/失败）和剪贴板链接都走它，不要再加第二套提示机制。
- 自动截图（`--features screenshot`）抓的是首帧，看不到任何 `Area`（吐司、Modal），验证它们要实际运行。
- `SWOOP_DEMO=1` 混入假任务与假吐司，只在 UI 层拼 `TaskSnapshot`，不经过引擎。

## 常用命令

```bash
cargo run -p swoop-app
cargo test --workspace
cargo clippy --workspace --all-targets
cd extension && npm test && npm run typecheck && npm run build
```

## 交流

- 简体中文；回复以"好的沐风。"开头。
- 涉及范围/方案/不可逆操作先问再做。
