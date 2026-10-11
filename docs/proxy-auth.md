# 代理认证

设置 → 网络与重试 → 手动代理 → 代理认证。HTTP/HTTPS 代理可选 Basic、Digest、NTLM、Negotiate；SOCKS5/SOCKS5h 使用独立用户名密码。默认不启用认证，密码可以仅保留本次会话或存入 macOS Keychain / Windows Credential Manager；系统支持时，NTLM/Negotiate 可选当前系统身份。

代理地址仍只能填写协议、主机与端口，禁止把用户名密码写进 URL。凭据按规范化代理端点、认证方式、用户名和存储模式隔离；改变这些选项后需要重新输入密码。未修改身份且密码框留空时保留原凭据。会话密码不会跨应用重启恢复。

`NetworkPolicy.proxy_auth` 只保存身份和不透明系统凭据引用。`Engine::save_network_policy(policy, password)` 在后台准备新系统凭据，成功提交一条 `network_policy` 记录后再替换运行配置；数据库失败保留旧配置并清理新凭据。下载启动时冻结网络配置，正在运行的下载持有旧凭据，直到最后一个旧运行释放后才删除退役系统条目。`set_network_policy` 保留为加载和临时配置接口，不写入密码。

普通 HTTP(S) 下载通过系统 libcurl 的专用代理认证选项发送凭据，代理认证不进入源站请求头。跨站重定向继续使用同一个已授权代理，站点 Authorization、Cookie 和私有请求头仍剥离，跳回原站也不恢复。排除代理的目标既不读取代理密码，也不附带代理认证。HTTPS 代理和 HTTPS 源站均保持证书与主机名校验，不能关闭验证。NTLM、SPNEGO 和当前系统身份按运行库能力检测，不支持时明确失败。

本次认证代理适用于普通 HTTP(S) 下载。FTP 若继承认证代理，或独立指定同一个需要认证的代理，会明确拒绝；可显式选择 FTP 直连或其他无认证代理。媒体继承认证代理也会明确拒绝，不能把无密码请求当作已支持代理认证。

验证：

```bash
cargo test -p swoop-core --lib --test proxy_auth --test site_auth --test network_policy --locked
cargo test -p swoop-app ui::network::tests --locked
```

本地测试使用只监听回环地址的真实 HTTP 407 / SOCKS5 服务：Basic、Digest、NTLMv2 Type 1/2/3 证明验证、SOCKS5 密码握手、错误密码拒绝及分段下载；验证代理认证不透传源站、跨站跳转剥离站点秘密、代理排除规则不读取缺失的会话密码、数据库提交失败、系统凭据恢复（fake vault）及活动配置快照保护。没有操作真实系统凭据，也没有真实企业 Kerberos/SSPI 域环境或 Windows 实机验收。
