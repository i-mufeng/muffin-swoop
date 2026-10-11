# 签名更新

设置 → 检查与更新。更新源和 Ed25519 公钥由用户或发行方在带外配置，默认均为空，自动检查默认关闭。稳定/预览通道及检查间隔可配置。清单只公布已签名的版本、通道、平台、包地址、长度与 SHA-256；不会接受清单自行替换信任公钥。

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

`cargo test -p swoop-updater --locked` 覆盖签名、版本、通道与平台，真实 HTTP 下载、错误哈希/短响应/取消、安装目标外改、ZIP 路径边界、no-replace 原语、三个发布中断边界的真实文件恢复、恢复时外部改动保护、已完成记录重入，以及安装失败后启动临时旧应用脚本。就绪握手使用真实文件锁验证：持锁期间发出就绪但不替换、十类初检失败不发布或覆盖就绪记录、释放锁后安装并启动新版、等待期间更新包外改时保留并启动旧版，以及旧接口恢复中断 journal。

本机测试只操作临时安装树，不替换当前开发应用。journal 测试模拟进程中断，没有宣称物理断电验证。完整 Windows 交叉构建受本机缺少 Windows SDK 阻塞（`aws-lc-sys: windows.h file not found`）；安装模块已通过独立 MSVC 目标类型检查，但 Windows 文件占用/移动/重启实机验收仍保留。没有配置或发布正式更新源，没有生成生产签名密钥，没有完成 macOS notarization 或 Windows Authenticode 签名。

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
