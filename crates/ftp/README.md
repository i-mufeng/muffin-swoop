# swoop-ftp

独立 FTP/FTPS 传输库。只负责连接、探测和有界数据交付；不创建目标文件、不保存配置或密码、不调用系统 `curl`。
core 负责队列、文件归属、覆盖授权、持久化、重试，以及每个数据块的限速和配额。MMS、SFTP 不在本库范围内。

## 接入

```rust,no_run
use swoop_ftp::{FtpClient, FtpConfig, FtpRequest};

# async fn example() -> swoop_ftp::Result<()> {
let client = FtpClient::new(FtpConfig::default())?;
let request = FtpRequest::new("ftp://example.com/archive.bin")?;
let mut probe = client.probe(request.clone())?;
let metadata = probe.finish().await?;

// 偏移由调用方验证后提供；默认从 0 开始。
let mut transfer = client.download(request, 0)?;
while let Some(chunk) = transfer.next_chunk().await {
    // 调用方在此等待任务/全局限速和配额，然后写入 chunk。
    // 写入失败时：transfer.cancel().await?，确认底层已经退出。
    let _ = chunk;
}
// 必须检查最终结果。数据 EOF 或字节数正确都不代表 FTP 最终响应成功。
transfer.finish().await?;
# Ok(())
# }
```

`Probe::finish` 和 `Download::finish` 可以重复等待；取消这些等待 future 不会丢失底层 JoinHandle。
`cancel().await` 在 worker 退出、curl handle 销毁后才返回；`Drop` 仅请求取消，不能作为清理完成的凭据。
`Download::finish` 在尚未读到 `next_chunk() == None` 时返回 `NotDrained`，防止有界队列死锁或跳过未消费数据。

每个操作使用一个阻塞池 worker，所有 libcurl 调用和 handle 生命周期都在该 worker 上完成。
异步消费者通过有界通道收取最多 16 KiB 的块，默认缓冲 4 块，可配置 1–64 块。
缓冲满后 libcurl 暂停接收；恢复回调重交的数据只会入队一次。libcurl 与操作系统仍有自身缓冲，
因此这不是网络层的零预读承诺。core 继续按既有口径计量实际写入；若另做内容验证下载，须明确其配额口径。

## 配置和认证

- `FtpConfig` 与其模式、代理配置支持 serde；缺失字段使用默认值。
- 默认被动模式，优先 EPSV，失败可回退 PASV；忽略 PASV 中第三方 IP，使用控制连接目标。
- 主动模式支持 IP 地址及包含两端的本地端口范围；不传地址使用控制连接地址。
- 直连为默认；不读取系统代理环境变量或 `.netrc`。代理显式配置为 HTTP/HTTPS CONNECT、SOCKS5 本地解析或代理解析，可配置排除主机。
- 主动模式与代理同时配置返回 `ActiveProxy`，不静默绕过代理。
- 连接、FTP 控制响应、数据停滞和整体传输超时均可配置。消费者背压暂停不算数据停滞；整体传输超时仍包括暂停时间。
- `ftp://` 默认明文，可用 `with_tls(Explicit)` 强制 AUTH TLS，或 `with_tls(Implicit)` 使用立即 TLS 握手。
  `ftps://` 默认且必须是隐式 TLS。缺省端口分别为 21、990；显式指定端口会保留。
- TLS 验证没有关闭接口；始终检查证书及主机名，显式 TLS 失败不降级。私有 CA 可通过 `ca_certificate` 提供，同时用于 HTTPS 代理。
- URL 不允许 userinfo。认证通过 `FtpCredentials::new` + `request.with_credentials` 单独提供，密码不实现 serde。
  有凭据时认证失败直接报错，不回退匿名。完全没有凭据才使用匿名；core 必须在“保存了用户名但密码缺失”时先报需要认证。
- 公开错误只含静态说明、curl 数字错误码和 FTP 数字状态；不包含原始服务端响应、URL、用户名或密码。
  请求和凭据的 Debug 同样脱敏。库不记录请求日志。
- 只下载二进制文件；目录 URL、控制字符及非 UTF-8 路径被拒绝。` ;type=` URL 传输指令不接受，字面量分号可编码为 `%3B`。

## 元数据与续传

`FtpMetadata { size, modified_unix }` 来自 SIZE/MDTM，字段可能缺失，始终是弱元数据。
下载结果的 size 表示整个文件大小，不是 REST 后剩余字节数。没有伪造 ETag，也不宣称提供强内容验证。

`download(request, offset)` 只执行调用方授权的 REST；服务器拒绝时直接失败，不静默从 0 交付并冒充该偏移。
库不判断原文件是否改变，不自动跨失败重试。若调用方无法确认旧片段仍属于当前内容，必须重新完整下载；
只有 SIZE/MDTM 相同不能证明内容一致。周期同步应完整暂存并校验后再由 core 发布，沿用目标文件保护。

## 构建和真实测试

声明 MSRV 1.95；本批实跑工具链为 rustc 1.98.1，不等同于已用 1.95 编译器验证。
依赖 `curl 0.4` / `curl-sys 0.4`，启用 `protocol-ftp`、静态 libcurl、静态 OpenSSL 和 macOS 系统证书信任。
macOS 使用源码构建的 OpenSSL，避免 Homebrew 动态库路径进入产物；Windows 默认使用 Schannel，仍需原生 MSVC/Windows SDK 构建 C 依赖。
运行时要求 libcurl 具备 FTP、FTPS 和异步 DNS；不会使用一个缺少协议支持的系统 libcurl 冒充成功。

当前已接入 workspace 与统一 `Engine`，验证命令在仓库根目录运行：

```bash
python3 -m venv target/ftp-fixture-venv
target/ftp-fixture-venv/bin/python -m pip install -r crates/ftp/tests/requirements.txt
cargo +stable test -p swoop-ftp -p swoop-core --locked
cargo +stable clippy -p swoop-ftp -p swoop-core --locked --all-targets -- -D warnings
```

`crates/core/tests/ftp_engine.rs` 使用同一真实 FTP/FTPS fixture，并额外启动真实 HTTP 端点与 SQLite 库，验证：

- 单连接匿名下载、零字节文件也必须取得最终 FTP 成功响应，短体和最终 426 不报完成。
- 显式/隐式 TLS 的私有 CA 验证、错误密码脱敏、认证恢复必须重新提交会话密码。
- 任务/全局限速、FTP 与 HTTP 共用配额、配额等待取消、停止队列不启动等待项。
- 暂停和失败重试均从 0 重读，SIZE/MDTM 不变也不复用旧片段；重复写入累计计入额度。
- 配置/默认值持久化先于内存应用；真实 SQLite 失败不会提前截断文件或启动 RETR。
- 同名询问、暂存覆盖、取消和原文件编辑保护，以及周期同步的完整内容比较。

2026-10-11 本机执行上述 workspace 测试：core 131 项（其中新增 FTP Engine 16 项），FTP 库 20 项（3 单测、17 真实服务测试）全部通过，0 跳过；
日志为 `target/ftp-core-tests.log`。严格 Clippy 记录在 `target/ftp-core-clippy.log`。这些是本机验证，不代表 Windows 实机验收。

公开接入接口及 GUI/CLI 用法见 [`docs/ftp.md`](../../docs/ftp.md)。底层库默认直连；Engine 默认选项采用 `FtpProxyPolicy::Network`，
跟随全局手动代理。全局自动系统代理无法安全映射时会明确失败，调用方可显式保存 FTP 直连或独立代理为默认。
`Engine::set_ftp_defaults` 拒绝用户名；任务用户名可以持久化，密码只保留会话内存。`configure_ftp_task` 一次应用配置与会话凭据，
不自动继续任务。认证、配置和代理只对下一次运行生效。

测试默认使用上述 venv；也可用 `SWOOP_FTP_TEST_PYTHON` 指定独立 Python 解释器（Windows 指向 venv 的 `Scripts/python.exe`）。
缺少 fixture 依赖会明确失败，不静默跳过。每个测试启动临时 127.0.0.1 FTP 服务及代理，退出时清理进程；无系统服务。
FTPS 使用现场生成的短期自签证书，只有显式信任该 CA 的路径能成功；也测试不受信证书和主机名不匹配。

Windows 在本 macOS 主机的交叉构建失败于 `libz-sys` 的 C 编译：`sys/types.h file not found`，缺 Windows SDK。
因此 Windows 完整构建、Schannel、主动模式防火墙行为及 Windows 实机测试均待验；不能用已安装 Rust target 代替这些验收。
