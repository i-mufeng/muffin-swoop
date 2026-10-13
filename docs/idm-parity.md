# IDM 功能对标与验收

更新：2026-10-11。目标：macOS 与 Windows 同时验收，用户可控制的策略优先提供持久化设置。
本表记录实际实现和证据；某一模块通过单测不代表整体对标完成。

## 来源

- [IDM 官方功能](https://www.internetdownloadmanager.com/features2.html)
- [选项与配置](https://www.internetdownloadmanager.com/support/using_idm/options.html)
- [主窗口与分类](https://www.internetdownloadmanager.com/support/using_idm/using_idm.html)
- [队列调度](https://support.internetdownloadmanager.com/support/idm-scheduler/idm_scheduler.html)
- [站点抓取](https://www3.internetdownloadmanager.com/support/idm-grabber/idm_grabber.html)

## 验收口径

每项分别检查：核心行为、配置与重启持久化、用户可达的界面入口、自动化测试、macOS 实机、Windows 实机。
主目标为现代 macOS 和 Windows 10/11；老版 Windows 支持没有得到验证。
Windows 专属的系统能力应提供对应平台实现或明确的平台差异，不能用隐藏按钮冒充完成。
系统通知 API 成功、交叉编译成功、CI 通过，都不能代替横幅、托盘和浏览器的实机验收。
安装包签名、自动更新源、浏览器商店发布需要实际发布基础设施，当前尚未配置。

## 功能矩阵

| 编号 | 能力 | 当前情况 | 验收要点 |
| --- | --- | --- | --- |
| D01 | HTTP/HTTPS、多连接、动态分段 | 已有实现，连接数配置已补 | 本地 Range 服务逐字节校验；1–32 连接；不同运行隔离 |
| D02 | 暂停、续传、异常恢复 | 已有实现，快速暂停/继续竞态已修复 | 立即多次切换、进程重启、ETag/大小变化、无 Range 回退 |
| D03 | 全局与任务限速、时段流量配额 | HTTP/FTP 文件下载已接入，媒体/抓取使用独立策略 | 多任务共用额度、窗口到期恢复、账本持久化与写入失败阻止下载 |
| D04 | 命名队列、排序、并发、定时起停 | 核心与界面已实现 | 所有添加入口接入；停止等待任务不启动；重启保持 |
| D05 | 周期同步队列 | 队列同步模式、周期检查和安全替换已实现 | 远端验证器、文件归属指纹、暂存发布、数据库与进程中断恢复 |
| D06 | 批量添加、数字/字母 URL 范围、导入导出 | 已实现范围、文本/.url 导入、多选链接导出 | 预览、去重、上限、部分失败不重复成功任务；导出不覆盖文件 |
| D07 | 分类、按类型/域名选择保存目录 | 可编辑规则、顺序和侧栏筛选已实现 | 自动分目录默认关闭；按 URL/已知文件名匹配，手工链接尚不按响应 MIME 重新路由 |
| D08 | 任务属性、改地址续传、描述 | 核心及详情入口已实现 | 地址更新后的强 ETag/大小校验、重置断点、凭据清除 |
| D09 | 列表排序、多选批量操作、清理完成项 | 已实现，可筛选完成项后批量移除记录 | 选择状态、排序持久化；已完成的保留文件，未完成的连同 `.swoop` 部分文件一起删除 |
| D10 | 浏览器自动接管与回退 | Chromium / Firefox 双产物与 API 测试通过 | 真实浏览器接管与安装仍待实机；Cookie/Referer/UA、设置关停 |
| D11 | 本页全部链接、过滤和选择 | 已实现 | 用户选中后才提交、逐项反馈、来源信息、并发限制 |
| D12 | 浏览器视频/音频捕获与质量选择 | 响应检测、去重、缓存限制、候选选择及桌面质量检查已实现 | 真实播放器与双浏览器交接待实机；旧桌面通过能力门拒绝媒体请求 |
| D13 | HLS/DASH 分片、合并及音视频分离 | 点播/直播认证代理、清晰度/音轨/文本字幕、完整分片恢复缓存已接入 | FFmpeg 真实产物、正文/语言/时间验证；动态 DASH 三段滑动窗口及跨窗口时间戳已通过实测，缓存仅复用经强验证的完整资源 |
| D14 | 站点抓取、项目和离线镜像 | HTML/CSS/SVG/网页清单/媒体引用、动态 Chromium 渲染与 DOM 导入已实现 | 域名/深度/robots/流量/超时、取消恢复；3 项真实 Chrome 通过，完整 PWA/登录应用不保证离线 |
| D15 | FTP/MMS | FTP/FTPS 统一引擎和桌面配置；MMSH/MMST 录制已接入 | MMSH 两阶段握手与 ASF→MKV 真实产物；MMST 服务器兼容待验；FTP 应用暂停重试从头下载 |
| D16 | HTTP/SOCKS/FTP 代理与排除规则 | 普通文件/FTP、HLS/DASH 点播与直播代理已接入；普通 HTTP(S) 支持代理认证 | Basic/Digest/NTLMv2/SOCKS5 真握手与分段下载；凭据不发送源站，FTP/媒体不继承认证代理 |
| D17 | 站点认证、Basic/NTLM/Kerberos/Negotiate | Basic/Digest/NTLM/Negotiate、会话与系统凭据库、系统身份已接入 | Basic/Digest/NTLMv2 真实网络通过；跨源清凭据、SQLite 无密码；系统库授权与企业域待实机 |
| D18 | 系统通知、声音、完成对话框 | 通知、声音配置及完成窗口已实现 | 完成/失败/时机独立配置、前后台、系统权限 |
| D19 | 托盘驻留与后台任务 | 已有实现，询问模式唤回已补 | 关窗不退出、菜单操作、重新显示、真正退出 |
| D20 | 完成后扫描、打开文件/目录、退出/关机 | 文件/媒体完成动作与全局倒计时已实现 | 扫描失败阻止打开、参数隔离、退出等待；实际系统关机未执行 |
| D21 | 剪贴板、拖入链接、拖出文件 | 原生 URL / 多个链接文件拖入、已完成文件多选复制拖出已实现 | 可配置并持久化；macOS 独立 pasteboard 与真实程序启动通过；实际鼠标手势及 Windows 待验 |
| D22 | 自定义工具栏/列表、主题、语言 | 深浅主题、卡片/表格、列/工具栏配置、桌面与扩展中英文已实现 | 桌面编译期语言资源与占位符检查；扩展 65 测试与双构建通过，原生语言切换待验 |
| D23 | 安装包、开机启动、单实例、自动更新 | 双平台打包、单实例、用户级启动、签名更新及离线签名工具已实现 | 检查周期/源/公钥/通道可配置，默认不联网检查；助手就绪握手、备份/中断恢复；正式签名发布与 Windows 待验 |
| D24 | CLI 与系统下载入口 | 文件/媒体/抓取/队列控制、同名策略及单次处理已实现 | 不起 GUI、鉴权/Origin 拒绝、跨进程锁、返回码、数据库失败阻止启动；macOS 真进程复用通过 |
| D25 | 同名询问与覆盖策略 | 默认询问，覆盖/重命名/跳过可保存；文件与媒体共用 | 临时下载后替换、失败保原件、目标占用与重启；见 [说明](file-conflicts.md) |

## 实施顺序

1. 完成当前通知、连接数配置和续传修复；建立双平台构建/打包检查。
2. 接通队列调度、批量管理和分类，扩展补齐本页链接与 Firefox 产物。
3. 增加网络策略、地址刷新、导入导出；完善列表操作与任务属性。
4. 实现媒体下载和站点抓取，并用可控服务器与媒体夹具验证。
5. 完善平台集成、安装更新与剩余配置，执行两平台实机清单。

未实现的条目必须保留，不能将其改为完成以满足里程碑。

## 当前证据

- 本轮完整回归：`cargo test --workspace --all-targets --locked` **361 passed、0 failed、21 ignored**；严格 workspace Clippy 与 rustdoc 检查退出码 0。测试包括 GUI 103、CLI 12、core 148、FTP 20、grabber 19、i18n 2、media 34、updater 23（含离线签名工具 5）。日志 `target/iteration-validation/idm-completion-final-{tests,clippy}.log`、`idm-completion-doc-tests.log`。rustdoc 当前没有可执行示例，不计作额外测试数。
- 21 个默认忽略项中，真实 FFmpeg 16 项与真实 Chromium 3 项已显式执行并通过；日志 `media-completion-final-real.log`、`idm-final-chromium.log`。实际发行 ZIP 的安装夹具也已显式通过，仅人工系统通知未执行；新包详细结果见桌面验收记录。
- 扩展 **65 项**测试、TypeScript 检查、Chromium / Firefox 双构建通过。桌面 AST 扫描 803 项界面文案，英文资源无缺失；英文主界面内部渲染检查修复了详情栏标签重叠，未代替原生语言切换操作。
- 代理认证专项 65 项与网络编辑界面 2 项均包含于上述 361 项，不另外累加。覆盖 Basic/Digest/NTLMv2/SOCKS5 真实握手、分段下载、绕过代理、数据库保存失败和旧活动凭据保留。
- macOS release 双入口构建通过；新包路径、哈希、真实进程下载与真实 ZIP 安装结果集中记录于 [桌面验收记录](desktop-validation.md)。Windows、原生手势与真实双浏览器交接不由这些自动化结果代验。

以下保留较早批次证据，不能与最新测试数累加：

- 上一配置与协议批次：`cargo test --workspace --locked` 共 **295 项通过，11 项默认 ignored**；10 项真实 FFmpeg 测试另行显式执行并全部通过，剩余 1 项为人工系统通知测试。严格 workspace Clippy 通过。日志为 `target/iteration-validation/configured-parity-final-{tests,clippy,real-media}.log`。
- 295 项包括：GUI 89、CLI 11、core 131（38 单元 / 16 FTP Engine / 38 HTTP / 12 网络与 SQLite / 20 同步 / 7 文件归属）、FTP 20（3 单元 / 17 真实 FTP/FTPS/代理）、grabber 14、media 30（23 单元 / 5 认证代理网络 / 2 HTTP）。FTP 已接入 workspace、统一队列和桌面；表格/工具栏配置、FTP 草稿凭据隔离、媒体连接取消回收及单实例锁继承均有回归。
- 该批扩展 53 项测试、TypeScript 检查及 Chromium / Firefox 构建通过；FTP 手动交接不读取或发送网页 Cookie/Referer。日志 `target/iteration-validation/ftp-extension-final.log`。真实浏览器安装及交接仍待实机。
- 该批 macOS release 包的 HTTP/媒体/抓取控制及 FTP 真进程烟测通过，包含默认同名询问和传输失败保留原件；包路径、哈希及报告见 [桌面验收记录](desktop-validation.md)。原生按钮/手势与 Windows 桌面验收仍未执行。



- 同名策略批次统一回归：`cargo test --workspace --locked` 共 **231 项通过，8 项默认 ignored**；其中 7 项真实 FFmpeg 测试另行显式执行并全部通过，剩余 1 项为人工系统通知测试。严格 workspace Clippy 通过。
- 231 项包括：GUI 71、CLI 10、core 115（38 单元 / 38 HTTP / 12 网络与 SQLite / 20 同步 / 7 文件归属）、grabber 14、media 21。日志 `target/iteration-validation/workspace-final-{tests,clippy}.log`，真实媒体日志 `media-publication-real.log`。
- 文件竞争实证：HTTP 1,000 次、媒体独立夹具 30,000 次外部原子保存竞争均保留用户修改；媒体 16 次恢复冲突保留恢复路径。删除未确认原件、写库失败先停写、移动原件后进程中断恢复另有真实 SQLite/HTTP 回归。
- 同名与目标登记的 Windows MSVC 条件代码已作独立交叉 Clippy 检查；这不等于完整 Windows 编译或实机验收。该 231 项旧基线尚未计入 FTP 库；当前接入情况见上方最新记录。
- 同名策略批次扩展：48 项测试通过，TypeScript 检查和 Chromium / Firefox 两份构建通过。
- 起始基线：17 app 单元/桥接测试、16 core 单元测试、9 HTTP 集成测试通过。
- 通知与连接数批次：25 app 测试、16 core 单元测试、13 HTTP 集成测试通过；1 项会发送原生通知的测试默认跳过。
- 快速暂停/继续回归：修复前真实失败 `Failed(TaskId(1), "任务已取消")`，修复后连续 12 次立即暂停/继续并逐字节校验通过。
- 起始 Chromium 扩展：13 项测试、类型检查、构建通过。
- 原生 macOS 自动化被辅助功能/屏幕录制授权阻挡；开发渲染截图只验证主界面布局，不代替交互、横幅、托盘验收。
- 媒体普通测试与真实 FFmpeg 测试分开：真实用例默认 ignored，需显式执行；本机已构建独立 HLS/DASH 测试工具，未替换系统 FFmpeg。
- Windows 实机：用户确认暂时没有，先完成开发并保留验收项。

## 收尾与保留项

动态 DASH 滑动窗口与普通下载代理认证已接入并通过专项回归；配置入口、默认值与生效时机见 [配置汇总](configuration.md)。

以下项目需要相应环境或真实操作，不能用本地夹具替代：Windows 完整原生构建/打包/桌面、macOS 原生弹窗与拖放手势/托盘/通知、双浏览器真实加载接管、Keychain/Credential Manager 授权、企业 Negotiate/Kerberos 域登录、MMST 服务器兼容、正式代码签名/公证/更新源和商店发布。

媒体格式、完整动态网站和代理认证的支持范围以对应文档为准；不会宣称兼容所有线上站点、任意流媒体或完整替代 IDM 的所有平台行为。
