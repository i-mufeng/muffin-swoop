# Swoop A「折光」正式资源

2026-10-11 用户选定 A。轮廓、默认间距和配色与 `logo-geometry-lab-v1` 的 A 一致。

`swoop.svg` 是正式母版：一个带相切圆角的凹四边形，复用一次并绕 `(120, 120)` 旋转 180°。两片分别使用蓝 `#176FFF` 和紫 `#7C3CF4`；轮廓旋转对称，颜色在旋转后互换。无需字体或外部资源。

- `swoop.svg`：透明底蓝紫主标，侧栏使用。
- `swoop-mono.svg` / `swoop-white.svg`：深色 / 白色单色版。
- `swoop-blue.svg`：系统蓝 `#4A6CF2` 单色版。
- `swoop-app.svg`：已展示的白标蓝色圆角底板，用于应用与扩展。
- `png/`：256 px 主标、44 px 菜单栏模板、16–1024 px 应用图标。
- `swoop.ico`：16 / 24 / 32 / 48 / 64 / 128 / 256 px 七帧。
- `swoop.icns`：macOS 应用图标。
- `index.html`：正式预览，可切换明暗背景与下载资源。
- `installer/`：Windows 安装器侧栏（164×314）与页眉（150×57）的 100%–250% 六档 PNG，由 `python3 scripts/make_installer_art.py` 生成，打包时转为 BMP。

重新生成：在仓库根目录运行 `node assets/brand/swoop/build-assets.mjs`。需要 `rsvg-convert`；macOS 使用系统 `iconutil` 生成 ICNS。脚本也同步扩展现有 SVG / PNG；正式母版不依赖历史演示目录。

## 应用入口

`crates/app/src/brand.rs` 载入内嵌 PNG；侧栏使用双色主标，窗口使用应用图标，托盘从单色资源读取透明度并按平台着色。macOS 保留模板图标模式。`scripts/package-macos.sh` 将 ICNS 放入应用包并设置 `CFBundleIconFile`。

扩展 `public/icons/` 已使用同源应用图标，现有 manifest 和页面无需修改。正式 SVG 仍是可编辑矢量；桌面与扩展 PNG 是平台消费格式。

Windows 的 `swoop.exe` 与 `swoop-cli.exe` 通过 `crates/app/build.rs` 嵌入这份 ICO 的七个尺寸，普通 Cargo 构建和 Windows 打包共用该入口；缺少图标或资源编译失败会阻断构建。窗口图标仍使用同源 PNG。`scripts/verify-windows-icon.py` 可逐帧比对 EXE 内的图标和母版，Windows CI 在打包后检查两个 EXE。

macOS 裸 `cargo run` 不采用 bundle 的 Dock 图标，需用打包后的 `.app` 查看。未做签名、公证、安装或发布。

## 本次验证

- `cargo test -p swoop-app --bin swoop --locked`：103 通过、0 失败、1 忽略（系统通知人工验收）。
- `cargo build -p swoop-app --bins --features screenshot --locked` 成功；独立数据目录的明暗主题截图已目视核对侧栏标志。
- 菜单栏模板 PNG 通过边距、有效覆盖、抗锯齿和着色不改变透明度的测试。几何旋转对称由 SVG 的共享路径及变换保证；不要求栅格化后的圆弧边缘 alpha 逐像素相等。
- 扩展 Chromium / Firefox 构建成功，现有构建资源测试 2 项通过；两份输出的图标与正式资源字节一致。
- 5 个 SVG 的 XML、母形复用和无位图检查通过；11 个 PNG 的尺寸正确；ICO 七帧可解析，ICNS 文件有效。
- macOS 本地 debug 包生成成功，图标声明和 ICNS 内容匹配；包内 GUI / CLI 与本次构建的二进制字节一致，ZIP CRC 完整。
- 正式预览的 1280 / 636 / 390 px 布局无横向溢出，明暗切换后单色下载随之切换，页面和下载资源均为 HTTP 200。

未进行浏览器扩展实装或 Windows 实机检查；本地 `.app` 的资源检查不等于 Finder / Dock 图标缓存刷新验收。

## Windows EXE 补齐验证

2026-10-11 在 macOS 上通过 `cargo xwin build --locked -p swoop-app --bins --target x86_64-pc-windows-msvc` 实际构建两个 x64 debug EXE。`verify-windows-icon.py` 提取主图标组，确认两个 EXE 的 16 / 24 / 32 / 48 / 64 / 128 / 256 px 七帧都与 A 的 ICO 元数据和图像字节一致。另用 `llvm-readobj --coff-resources` 确认 `ICON` 与 `GROUP_ICON`；非 PE 文件、图像字节不同的反例都会被检查脚本拒绝。

构建同时修复三处原有 Windows 编译阻塞：FTP 错误码遵循 libcurl 的平台类型、认证错误码在 MSVC 边界转为既有的 u32、macOS 专有托盘 API 改用编译期分支。没有改变下载或认证流程。

本机 core 单元测试 42 项、FTP 单元测试 3 项、桌面测试 103 项通过，1 项系统通知人工测试忽略；托盘着色测试复查通过。core / FTP 的 lib Clippy 与 app 的 all-targets Clippy 均通过 `-D warnings`。Windows CI 已加入打包后逐帧检查，本轮未提交、推送或触发远端 CI。Windows Explorer / 任务栏实机显示、Windows 原生打包和签名发布未执行。
