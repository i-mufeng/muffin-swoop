<p align="center">
  <img src="assets/brand/swoop/png/swoop-app-256.png" width="128" alt="Swoop logo">
</p>

<h1 align="center">Swoop</h1>

<p align="center"><strong>快速、高效、轻量的下载管理器——用 Rust 编写。</strong></p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

<p align="center">
  <a href="https://github.com/i-mufeng/muffin-swoop/releases/latest"><img src="https://img.shields.io/github/v/release/i-mufeng/muffin-swoop" alt="GitHub release"></a>
  <a href="https://github.com/i-mufeng/muffin-swoop/actions/workflows/ci.yml"><img src="https://github.com/i-mufeng/muffin-swoop/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/i-mufeng/muffin-swoop" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/rust-1.95%2B-orange" alt="Rust 1.95+">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-blue" alt="Platforms: macOS and Windows">
</p>

<p align="center">
  <a href="https://github.com/i-mufeng/muffin-swoop/releases/latest"><strong>下载</strong></a> ·
  <a href="#功能"><strong>功能</strong></a> ·
  <a href="#开发"><strong>开发</strong></a> ·
  <a href="docs/"><strong>文档</strong></a>
</p>

<p align="center">
  <img src="assets/screenshots/home-dark.png" alt="Swoop 主界面——深色主题，英文界面">
</p>

Swoop 是一个围绕多连接下载引擎构建的原生下载管理器。普通文件下载不需要任何额外运行时，它还能接管浏览器下载、录制流媒体、镜像整个站点——全部集成在一个小巧快速的二进制文件里。

## 下载

从 [GitHub Releases](https://github.com/i-mufeng/muffin-swoop/releases/latest) 获取最新构建：

| 平台 | 包 |
| --- | --- |
| macOS（Apple Silicon） | `.dmg` / 签名更新 `.zip` |
| Windows（x64 / ARM64） | 用户级 NSIS 安装包 |

- 默认按当前用户安装，不强制写入系统目录；仅登记浏览器扩展等系统级操作会请求一次管理员授权（UAC）。
- Windows 安装包已在 Windows 平台原生构建发布，暂无 Authenticode 签名，首次运行可能触发 SmartScreen；交互桌面验收项保留，详见 [验收说明](docs/desktop-validation.md)。
- 内置**签名自动更新**（Ed25519，稳定/预览通道）；更新源、公钥与检查间隔均可配置。见 [更新说明](docs/updates.md)。
- **只有媒体下载（HLS/DASH/MMS）需要 FFmpeg**，普通文件下载完全运行在 Swoop 自研引擎上。

## 功能

### 下载引擎

- **多连接分段下载**——默认 8 连接，每任务可调 1–32。
- **动态分段再平衡**——先完成的连接会从剩余最大的区段「窃取」一半继续下载，单个慢连接永远不会拖住整个文件。
- **HTTP 断点续传**——按区段持续落盘进度；暂停、退出后从断点继续。续传前重新校验 ETag 与文件大小，远端文件变化则完整重下。
- **安全的原地写入**——文件预分配，经 `.swoop` 临时文件写入；移除未完成任务自动清理部分数据。
- **FTP / FTPS** 与 HTTP 共用队列、限速、配额与同名处理。见 [FTP 说明](docs/ftp.md)。

### 媒体与站点

- **流媒体录制**——HLS（含 AES-128）与无 DRM 的 DASH，可选清晰度、音轨与字幕；带时长上限的直播录制；MMSH/MMST；经 FFmpeg 转封装为 MP4/MKV。校验过的分片会缓存复用。见 [媒体说明](crates/media/README.md)。
- **站点抓取**——镜像 HTML/CSS/SVG 页面、媒体与字幕引用、懒加载图片；支持域名/深度/类型过滤、robots.txt、预算限制与可恢复项目。动态页面可用隔离的 Chromium 实例渲染。见 [抓取说明](docs/grabber.md)。

### 浏览器集成

- **Chromium 与 Firefox 扩展（MV3）**在浏览器下载面板弹出*之前*接管下载——Chromium 在文件名确定阶段介入，Swoop 无法接管时自动回退给浏览器。见 [协议与安全](docs/browser-extension.md)。
  Edge 用户可从 [Edge 加载项商店一键安装](https://microsoftedge.microsoft.com/addons/detail/swoop/ecoapkmgaijhekboiplkojnpefddffkh)；360 极速/安全、QQ 等国产 Chromium 浏览器可将 Releases 中的 `.crx` 拖入扩展管理页安装（360 安全浏览器用 `se://extensions/`，360 极速用 `chrome://myextensions/`）。
- **被动媒体检测**记录页面上的音视频候选供你选择；不会自动开始下载。
- 右键任意链接、图片、视频即可发送到 Swoop；扩展弹窗可列出当前页全部链接。

### 组织与控制

- **命名队列与计划**——每队列并发、任务顺序、启动/停止时间与周期调度，重启后全部保留。
- **分类**按扩展名或域名把文件归入对应目录（默认关闭）。
- **批量工具**——数字/字母 URL 范围展开与去重预览、文本与快捷方式文件导入、原生拖放链接与文件、多选操作、链接导出。
- **限速**——令牌桶算法，全局与每任务独立，运行中可调；按周期共享下载配额。
- **网络策略**——连接/读取/总时长超时、重试与退避、HTTP(S)/SOCKS 代理及认证、按站点凭据（Basic、Digest、NTLM、Negotiate；会话、账户或系统凭据库）。见 [站点认证](docs/site-auth.md) 与 [代理认证](docs/proxy-auth.md)。
- **同名冲突**——询问、覆盖、自动重命名或取消；覆盖采用暂存机制，成功后才替换。见 [同名处理](docs/file-conflicts.md)。

### 使用体验

- **原生 UI**（egui + wgpu），深/浅主题，卡片/表格布局，可自定义列与工具栏，简体中文 / English。
- **详情面板**右滑展开：实时速度曲线（平均/峰值）、续传支持、各区段进度。
- **菜单栏 / 托盘驻留**——关闭窗口下载继续；菜单显示总速度与快捷操作。
- **系统通知**完成与失败事件可分别开关（前台聚焦时不打扰），声音可选。
- **完成后动作**——扫描、打开文件/目录，或全部完成后倒计时退出/关机。见 [动作说明](docs/completion-actions.md)。
- **单实例与 CLI**——重复启动交给已有窗口；`swoop-cli` 在终端管理文件、媒体、抓取项目与队列。见 [命令行说明](docs/cli.md)。
- **内嵌 CJK 字体子集**（约 1.5 MB），中文在任何系统上渲染一致。见 [字体声明](assets/fonts/NOTICE.md)。

## 项目结构

```
crates/core     swoop-core      下载引擎（纯库，无 UI 依赖）
crates/app      swoop-app       egui 桌面应用 + swoop-cli 控制台
crates/media    swoop-media     HLS/DASH/MMS 检查与可取消的 FFmpeg 下载
crates/grabber  swoop-grabber   可恢复站点抓取与离线镜像
crates/ftp      swoop-ftp       可取消的 FTP/FTPS 传输
crates/updater  swoop-updater   签名更新、安装与恢复
crates/i18n     swoop-i18n      桌面语言资源（编译期检查）
extension/      Chromium / Firefox MV3 扩展（TypeScript + esbuild）
crates/browser-bridge  浏览器识别与扩展安装策略（策略表：crates/browser-bridge/browsers.toml）
docs/           设计与验收文档
assets/         品牌资源、截图、内嵌字体子集
scripts/        打包、安装包与字体子集工具
```

## 开发

需要 **Rust 1.95+**（eframe/egui 0.36 的最低要求）。

```bash
cargo run -p swoop-app              # 启动应用
cargo test --workspace --locked     # 单元测试 + 本地 HTTP/FTP 端到端测试
```

调试界面的常用开关（Windows 用 `set` 设置环境变量）：

```bash
SWOOP_DEMO=1 cargo run -p swoop-app                        # 混入 8 条覆盖所有状态的假任务
SWOOP_LANG=en-US cargo run -p swoop-app                    # 本次运行强制英文界面，不写回设置
SWOOP_THEME=light cargo run -p swoop-app                   # 以浅色主题启动
SWOOP_DATA_DIR=tmp/manual-check cargo run -p swoop-app     # 独立数据库，便于手工验收
scripts/screenshot.sh home dark out.png                    # 截取任意界面后退出（演示数据、临时目录）
```

浏览器扩展：

```bash
cd extension
npm install
npm run build       # Chromium -> dist/，Firefox -> dist-firefox/
npm test            # 纯函数单元测试
npm run package     # 产出商店包 zip 到 target/packages/（附 .sha256）
```

每次推送与 Pull Request 都会在 macOS 和 Windows 上运行测试与打包。通过本地夹具测试**不代表**所有网站、媒体格式或企业环境都兼容——已实现项与验收范围逐项记录在 [功能矩阵](docs/idm-parity.md)。

## 配置

所有设置保存在平台应用支持目录下按用户隔离的 SQLite 数据库（WAL）中；设置以事务方式成组保存，默认值与说明汇总见 [配置说明](docs/configuration.md)。常用项：

| 设置 | 默认值 | 生效时机 |
| --- | --- | --- |
| 每任务连接数 | 8（1–32 可调） | 新任务与继续时；运行中的连接数不变 |
| 完成 / 失败通知 | 开 | 后续事件；暂停不提醒 |
| 通知时机 | 仅后台 | 可选「始终」 |
| 通知声音 | 关 | 后续系统通知 |
| 完成后扫描 / 打开 / 弹窗 | 关 | 后续完成的下载 |
| 全部完成后退出 / 关机 | 关（倒计时 30 秒，可取消） | 所有任务成功后 |

## 隐私说明

- 数据库只存放在你自己的用户目录下。
- 浏览器交来的任务会把请求头（含 Cookie）以明文存入任务表——续传所必需——且仅限该本地数据库。
- 系统通知只显示文件名；错误中的 URL 与请求头留在应用内。
- 按站点凭据可以选择存放在系统凭据库中，而不是数据库里。

## 参与贡献

欢迎 Issue 与 Pull Request。提交 PR 前请确保：

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cd extension && npm test && npm run typecheck
```

## 许可证

[MIT](LICENSE)
