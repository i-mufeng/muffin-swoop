# 站点认证

设置 → 站点认证。每项仅匹配完整来源（协议、主机、端口），默认不向 HTTP 明文连接发送凭据。可配置 Basic、Digest、NTLM、Negotiate；界面根据链接的 libcurl 能力禁用不可用方式。

- 本次会话：密码只在进程内存，退出后需重新输入。
- 系统凭据库：macOS Keychain / Windows Credential Manager。SQLite 仅保存来源、用户名、方式、随机凭据引用；密码不序列化，没有明文后备存储。
- 当前系统身份：Negotiate 使用已有 Kerberos/GSS-API/SSPI 身份；NTLM 当前身份仅允许 Windows SSPI。不会创建域账户或执行 kinit。票据、域配置及服务器兼容性需要企业环境验收。

HTTP 认证请求使用 libcurl 原生挑战处理，下载数据继续经过统一分段引擎、配额、限速、文件冲突和安全发布流程。macOS 使用系统 libcurl，Windows 使用带 NTLM/SPNEGO 支持的 vendored libcurl；均在运行时检查功能位。没有所需能力时明确失败，不降级为另一种认证。

跨来源跳转会移除认证、Cookie 和其他私密头，后续跳回原来源也不恢复。HTTPS 降为 HTTP、重定向 URL 带账号密码、同时配置站点认证与自定义 Authorization 均拒绝。代理使用现有网络策略；站点凭据不作为代理凭据发送。

修改认证只影响后续运行的网络快照。正在下载的旧快照保留其凭据引用，旧系统凭据在最后一个活动快照释放后清理；当前已保存的凭据不会在正常退出时删除。系统库删除失败会记录不含秘密的错误，可能留下未引用的加密条目。

## 验证边界

`cargo test -p swoop-core --test site_auth --locked` 使用本机 HTTP 夹具验证 Basic、Digest 的响应摘要及 NTLMv2 Type 1 → 2 → 3 的 NT proof，最终检查多分段下载内容。包含错误密码、跨源往返、Authorization 冲突、fake vault 的重启恢复、SQLite 写失败回滚。单元测试覆盖来源规范化、密码 Debug 隐藏、未知配置字段拒绝及活跃凭据版本生命周期。

测试不会修改真实系统凭据。macOS 系统库代码已经本机编译，Windows Credential Manager 适配器可做独立 MSVC 目标类型检查；两端的原生安全库授权交互、Windows 安装运行及真实企业 Negotiate/Kerberos 域登录继续保留实机验收项。该项不能以本地 NTLM 夹具替代真实域登录验收。

实现依据：[libcurl HTTPAUTH](https://curl.se/libcurl/c/CURLOPT_HTTPAUTH.html)、[libcurl 跨源认证边界](https://curl.se/libcurl/c/CURLOPT_UNRESTRICTED_AUTH.html)、[Windows CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew)。
