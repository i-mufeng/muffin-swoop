# FTP / FTPS 下载

下载引擎支持 `ftp://` 与 `ftps://` 文件地址，新建、批量范围、文本/`.url` 导入、剪贴板和 CLI 共用队列与同名策略。SFTP、MMS 不在本实现范围内。

## 连接与登录

在新建下载中输入 FTP 链接后显示登录栏和折叠的连接设置。支持被动/主动连接、主动地址及端口范围、明文/显式 TLS/隐式 TLS、代理与排除、连接/读取超时、自定义 CA。

- `ftp://` 默认明文；可选择显式 TLS。`ftps://` 始终使用隐式 TLS，不能降级。
- 默认跟随网络设置中的手动代理。FTP 不支持自动系统代理发现；若仅启用了系统代理，会明确要求指定 FTP 代理或选择直连，不会忽略代理设置。
- 独立代理支持 HTTP/HTTPS CONNECT、SOCKS5、本地/远端 DNS。主动连接不能配代理。
- “记住连接设置”保存到 `ftp_defaults`，后续普通添加入口和 CLI 共用。已有任务保留创建时配置；可暂停后在下载属性中修改。
- 用户名、密码通过独立输入栏提交；链接中嵌入用户名或密码会被拒绝。默认配置不保存登录身份。任务记录仅保存用户名，密码只留在会话中。
- 重启后私有任务显示需要重新登录；在下载属性中应用登录后点击继续。认证失败不会改用匿名账户。需要登录的批量链接必须属于同一协议、主机和端口。

证书和主机名验证始终启用；自定义 CA 同时适用于 FTPS 服务和 HTTPS 代理。原始服务端错误文本不会直接显示，以免泄漏服务端附加的敏感信息。

## 队列、暂停与文件保护

FTP 使用单连接传输，计入任务/队列并发、全局/任务限速与共用流量配额。取消时等待传输线程退出后才释放槽位和目标文件占用。

当前应用暂停后从头重新下载，重试的实际数据重新计入配额。虽然底层传输库具备 REST，应用不以 SIZE/MDTM 相同推断旧片段仍然有效；因此不会把 FTP 的大小/时间伪装成 HTTP 强 ETag。周期同步也会完整暂存并比较。

默认同名询问与 HTTP、媒体共用：可覆盖、重命名或跳过。覆盖和同步先完整暂存，检查 FTP 最终响应和文件归属后发布。收到全部字节后最终 `426` 仍视为失败，保留原文件；取消、配额中断和持久化失败也不能提前覆盖原件。

## 开发验证

首次运行 workspace 测试前安装本地 FTP 夹具：

```bash
python3 -m venv target/ftp-fixture-venv
target/ftp-fixture-venv/bin/python -m pip install -r crates/ftp/tests/requirements.txt
cargo test --workspace --locked
```

Windows 使用 `target/ftp-fixture-venv/Scripts/python.exe`，并将 `SWOOP_FTP_TEST_PYTHON` 设置为其绝对路径。CI 已配置 Python 与夹具依赖；缺少依赖时测试会失败，不会静默跳过。

实际应用冒烟验证：

```bash
cargo build -p swoop-app --bins --locked
python3 scripts/smoke-ftp-desktop.py target/debug/swoop
```

脚本使用独立数据目录和本地 FTP 服务，验证 CLI、队列、保存的 FTP 默认值、同名询问、覆盖以及最终失败保原件。它不代表原生弹窗手势、系统集成或 Windows 实机验收。
