# 签名更新

设置 → 检查与更新。v0.6.0 起官方更新源与 Ed25519 公钥内置在程序里：用户无需导入任何密钥，留空即使用官方源自动更新；「更新源」折叠栏中可填写自定义镜像源或公钥覆盖。自动检查默认关闭，稳定/预览通道及检查间隔可配置。清单只公布已签名的版本、通道、平台、包地址、长度与 SHA-256；不会接受清单自行替换信任公钥。

下载后再次验签并校验实际字节长度与哈希。只有用户明确选择「安装并重启」才启动更新助手；该动作需要当前任务/媒体/抓取/系统操作退出条件满足。主程序保留单实例锁，直到助手完成启动检查并发出匹配授权 token 的就绪记录后才退出；安装助手在单实例锁释放后才替换文件。

安装准备期间核心暂停接收新任务、续传及队列启动，桌面暂停浏览器桥接和下载入口。正在后台保存站点/代理凭据、导出链接或批量操作记录时不能开始安装；退出也会等待这些写入结束。取消或准备失败会恢复入口；助手确认就绪并且下载、保存与子进程全部结束后才允许主程序退出。

## 安装与恢复

- macOS 安装目标是完整 `Swoop.app`，若同级有 `swoop-cli` 则一并更新；Windows 安装目标是 `swoop.exe`、`swoop-cli.exe`、`build-info.txt`。其他同级用户文件保留。
- 准备阶段记录所有目标内容/目录结构指纹；安装前及实际移出原件后再次核对。CLI 或应用资源被其他程序修改也会阻止安装。
- GUI 使用 `run_helper_ready(plan_path, token, ready_path)`：助手先校验授权、计划形状、签名、更新包、原件及实例锁可用性，再以 `create_new` 写入计划同目录的固定文件 `helper-ready.json`，内容为 `{"token":"本次授权 token"}`。初检失败不发布就绪；已有记录不覆盖。GUI 只接受完整 JSON 和本次 token，并在此之前保持实例锁；取消或等待超时时应先结束自己启动的助手，再回收状态。助手拿到锁后仍重新校验更新包和原件，避免等待期间的外部改动。
- ZIP 先复制到私有暂存区，核对实际快照后解压。拒绝越界、链接、特殊文件、大小写别名、Windows 保留名称、超限文件和解压内容。
- 构建信息必须匹配应用 ID、签名版本、目标平台，以及实际 GUI/CLI 二进制哈希。
- 每个目标的新版本和备份在同一文件系统准备；发布使用“不替换已有目标”的系统原语。原件、新版及冲突副本均保留，避免删除外部改动。
- 首次移动前持久化 `journal.json`。进程中断后重跑同一助手会根据记录恢复原件；已经完成的记录可安全重入。外部改动发生在恢复期间时停止自动覆盖，并保留备份及恢复路径。
- 原 `run_helper(plan_path, token)` 保留供中断恢复使用，不要求首次移动前的原件仍全部位于原路径；新的就绪接口用于 GUI 发起的全新安装。
- 安装失败后，仅确认原版本完整恢复时才自动重启旧应用；无法确认时留下记录供恢复。不会自动提权，不会关闭其他进程。

当前备份与暂存区不自动删除，可在确认新版可用后由用户清理。应用没有写入安装目录的权限时会明确失败。

## 验证与发布边界

`cargo test -p swoop-updater --locked` 覆盖签名、版本、通道与平台，内置官方更新源/公钥默认值，空值与已轮换旧公钥的迁移及幂等，真实 HTTP 下载、错误哈希/短响应/取消、安装目标外改、ZIP 路径边界、no-replace 原语、三个发布中断边界的真实文件恢复、恢复时外部改动保护、已完成记录重入，以及安装失败后启动临时旧应用脚本。就绪握手使用真实文件锁验证：持锁期间发出就绪但不替换、十类初检失败不发布或覆盖就绪记录、释放锁后安装并启动新版、等待期间更新包外改时保留并启动旧版，以及旧接口恢复中断 journal。

本机测试只操作临时安装树，不替换当前开发应用。journal 测试模拟进程中断，没有宣称物理断电验证。完整 Windows 交叉构建受本机缺少 Windows SDK 阻塞（`aws-lc-sys: windows.h file not found`）；安装模块已通过独立 MSVC 目标类型检查，但 Windows 文件占用/移动/重启实机验收仍保留。没有完成 macOS notarization 或 Windows Authenticode 签名。

## 官方 GitHub 更新源（v0.6.0 起内置）

正式更新源托管在 GitHub Releases，v0.6.0 起客户端「设置 → 检查与更新」无需任何配置即可使用（自动检查默认关闭，由用户自行开启）：

| 配置项 | 值 |
| --- | --- |
| 主更新源（内置默认） | `https://muffin-cdn.b0.upaiyun.com/swoop-release/feed.json`（又拍云 CDN，国内直连） |
| 备用更新源（自动回退） | `https://github.com/i-mufeng/muffin-swoop/releases/latest/download/feed.json`（主源请求失败时自动切换，两源同一把内置公钥验签） |
| Ed25519 公钥（内置） | `20cf8eda8be4fd6654093f05c5bd5abe794b60c12762832152ffe440c08a3673` |
| 通道 | stable |

客户端「设置 → 关于」留空即用上述内置源；主更新源传输或清单失败时自动回退备用源，用户主动取消不回退。`swoop-release/feed.json` 是稳定地址，每次发版覆盖它即成为「最新」；带版本号的副本保留在 `swoop-release/vX.Y.Z/` 便于追溯。

`releases/latest/download/` 始终解析到最新 Release；每个 Release 上传固定文件名的 `feed.json`（签名清单）与 `swoop-update-public-key.hex`（公钥），同时保留带版本号的副本便于追溯。清单只覆盖有产物的平台。

**密钥轮换**：v0.7.0 起内置公钥轮换为 `20cf8eda…`（v0.6.0 使用的旧公钥 `76f5ee41…` 的生产私钥已不可用，作为 `LEGACY_PUBLIC_KEY` 保留迁移逻辑）。**0.6.0 及更早版本的内置公钥仍是旧值，无法验签 v0.7.0 清单，需手动下载 v0.7.0 安装包升级，之后即可恢复自动更新**。存量配置中等于旧公钥的值在加载时自动迁移到内置新公钥（`UpdateConfig::migrate`，幂等）。更早的历史：v0.1.0 曾公布旧公钥 `f2946392673b322aacfc47046a7ab71e205fc75efe845c61c56e3b24c73d6a7a`，v0.6.0 起已轮换。

发布流程（无 CI 额度时全本地执行）：打包脚本产出 ZIP（Windows ZIP 条目名使用正斜杠，符合 ZIP 规范与验签要求）→ 用 `sign_feed` 与生产私钥签名生成 `feed.json` → 扩展包与应用同版本构建（`extension/package.json` 与 `public/manifest.json` 版本必须等于应用版本，`scripts/package.mjs` 会校验）→ 打 tag 推送 → `gh release create` 上传应用包、扩展包、清单与公钥。生产签名密钥由发行方本地保管于项目外目录，权限仅限发行账户，不入库；公钥以内置常量、上表和 Release 资产形式分发。

在启用应用程序控制策略（WDAC）的机器上，`target\examples` 下的独立签名 exe 可能被拦截；此时改用 `sign_feed` 内置的环境变量门控测试入口执行同一签名流程（`SWOOP_RELEASE_SIGN=1` 加上 `SWOOP_SIGN_*` 参数，见 `crates/updater/examples/sign_feed.rs`），测试宿主不受该限制。

## 国内更新源（又拍云主源 + GitHub 自动回退）

GitHub 在国内访问不稳定，又拍云（Upyun）云存储 + CDN 是内置**主更新源**，GitHub 官方源作为**备用源**：主源请求失败时客户端自动回退，两源清单用同一把内置 Ed25519 公钥验签。清单带签名，托管方无需可信，CDN 回源或缓存不一致不会影响验签安全，最多导致检查失败。

- 内置主源地址：`https://muffin-cdn.b0.upaiyun.com/swoop-release/feed.json`（`crates/updater/src/lib.rs` 的 `DEFAULT_FEED_URL`）。
- 又拍云需要**绑定已备案域名**后才对外开放访问（默认 `b0.upaiyun.com` 域名默认不可见）。绑定后在控制台开放默认域名访问，或把 `DEFAULT_FEED_URL` 换成绑定的域名；换域名只需改这一处常量，备用回退逻辑不变。
- 每次发版除了上传 `swoop-release/vX.Y.Z/` 版本目录，还要覆盖稳定地址 `swoop-release/feed.json`：
  `bash scripts/publish-upyun.sh --dir /releases/0.7.0 --remote-prefix swoop-release/v0.7.0 --cdn-base <你的域名>`
  再用同一凭据把 `feed.json` 上传到 `swoop-release/feed.json`。
- 签名 `feed.json` 时各 `--artifact` 的 URL 应使用又拍云域名（国内用户下载安装包也走 CDN），例如 `https://<你的域名>/swoop-release/v0.7.0/Swoop-0.7.0-windows-x86_64-release.zip`。

脚本按「安装包 → feed.json」的顺序上传（客户端永远不会看到指向未完成上传的清单），`--cdn-base` 会在上传后自动刷新 feed 与各包的 CDN 缓存。建议为发布单独创建一个仅云存储权限的 API 操作员，密码只放在发行账户的环境变量里。

## 离线生成签名清单

仓库提供 `crates/updater/examples/sign_feed.rs`。它只读取本地发行 ZIP 和显式指定的私钥文件，计算长度与 SHA-256，生成现有客户端可直接读取的 `SignedFeed`；不会联网、上传、生成生产密钥，也不在终端打印任何密钥。一个清单可重复传入 `--artifact`，同时包含 macOS 与 Windows 包。

私钥文件须事先安全准备，格式为 **32 字节 Ed25519 seed** 或 **64 个十六进制字符**（允许末尾换行），不是 PEM、OpenSSH 文件或 64 字节扩展私钥。Unix 文件权限必须禁止组及其他用户访问，例如 `0600`；Windows 请将文件 ACL 限制为发行账户。不要把私钥放进仓库或下载发布目录。工具会清零读取私钥的临时缓冲区，签名对象使用库自带的 zeroize 支持。

先将项目版本提升并使用现有打包脚本生成对应版本 ZIP。工具会检查包内 `build-info.txt` 的版本、应用 ID、平台 triplet 及 GUI/CLI 哈希，参数与包不一致时拒绝签名。示例（路径、URL 和版本按实际发行修改）：

```bash
cargo run -p swoop-updater --example sign_feed --locked -- \
  --private-key /secure/swoop-ed25519-seed.hex \
  --version 0.2.0 --channel stable \
  --artifact macos-aarch64 /releases/Swoop-0.2.0-macos-arm64-release.zip https://updates.example.com/Swoop-0.2.0-macos-arm64-release.zip \
  --artifact windows-x86_64 /releases/Swoop-0.2.0-windows-x64-release.zip https://updates.example.com/Swoop-0.2.0-windows-x64-release.zip \
  --notes-file /releases/notes.txt \
  --output /releases/feed-0.2.0.json \
  --public-key-out /releases/feed-public-key-0.2.0.hex
```

`--notes-file` 可省略，通道默认 `stable`；预发布版本使用 `--channel preview`。支持的平台标识为 `macos-aarch64`、`macos-x86_64`、`windows-x86_64`、`windows-aarch64`。输出目录必须已存在，两项输出均不能覆盖已有文件。若写第二项输出时发生磁盘错误，先前成功写出的公钥文件会保留；改用新的输出文件名重试。

生成的公钥文件用于独立可信渠道分发，客户端填入其去掉末尾换行后的十六进制内容。不要将 feed 自报公钥当作信任依据。正式发布 ZIP、feed 与公钥的渠道仍由发行方另行安排；本工具不执行发布动作。

验证命令：

```bash
cargo run -p swoop-updater --example sign_feed --locked -- --help
cargo test -p swoop-updater --example sign_feed --locked
```

示例测试在临时目录创建已公开的测试 seed，完成「本地 ZIP → SignedFeed → 生产 `verify` → 生产 `prepare`」闭环，确认旧应用未被替换；另验证原始/十六进制 seed 一致、不覆盖输出、平台不符、不安全 URL 及宽松私钥权限拒绝。临时测试密钥随测试目录清理，不是生产密钥。

## 真实打包产物的兼容检查

macOS 可对打包脚本生成的原样 ZIP 执行：

```bash
SWOOP_UPDATE_PACKAGE="$PWD/target/packages/Swoop-0.1.0-macos-arm64-release.zip" \
  cargo test -p swoop-updater --test package_smoke --locked -- --ignored --nocapture
```

测试使用临时安装目录与公开的固定测试 seed，对实际 ZIP 执行生产 `verify → prepare → apply`，逐文件检查完整应用内容、执行权限、CLI 哈希、原版本备份和其他用户文件保留。包与测试程序同版本时，测试在临时 workspace 中使用相同生产源码构建版本号为 `0.0.0` 的旧宿主，保持生产“只允许升级”校验；不改变原 ZIP，不启动或修改用户现有安装，不冒充旧正式发行版的升级实测。独立编译缓存位于 `target/updater-package-smoke`，依赖解析离线执行。
