# 命令行与单实例

Swoop 的普通桌面启动在打开数据库前获取跨进程文件锁。同一数据目录再次启动只唤醒已有窗口，不另建引擎；不同 `SWOOP_DATA_DIR` 使用独立锁和控制服务。锁文件保留在磁盘上，进程退出后操作系统释放锁，文件存在本身不代表实例仍在运行。

## 使用

macOS 可使用 `swoop` 的命令参数或包内的 `swoop-cli`；Windows 自动化必须使用 `swoop-cli.exe`，它使用 console subsystem，支持 stdout 重定向与可靠的 shell 等待/退出码。`swoop.exe` 仍是无控制台 GUI；带参数时尝试附着父控制台，但不作为 Windows 脚本入口。

```sh
swoop-cli --help
swoop-cli --version
swoop-cli status
swoop-cli add https://example.com/archive.zip --dir "$HOME/Downloads" --name archive.zip
swoop-cli pause 12
swoop-cli resume 12
swoop-cli queue-stop 1
swoop-cli queue-start 1
swoop-cli show
swoop-cli conflict-policy
swoop-cli conflict-policy ask
swoop-cli resolve 12 --action rename
swoop-cli media-add https://example.com/master.m3u8 --output '/path/to/video.mp4' --max-height 720
swoop-cli media-status
swoop-cli media-stop 1
swoop-cli media-retry 1
swoop-cli media-resolve 1 --action rename
swoop-cli grabber-create https://example.com/ --dir '/path/to/site' --depth 2 --pages 200
swoop-cli grabber-status
swoop-cli grabber-run
swoop-cli grabber-stop
swoop-cli grabber-open '/path/to/site'
swoop-cli --data-dir '/path/to/isolated data' status
```

先正常启动桌面应用。没有运行实例时控制命令明确失败；CLI 不会自动启动 GUI、headless daemon 或打开/创建数据库。启动独立桌面实例使用 `SWOOP_DATA_DIR` 环境变量，CLI 可使用同一变量或 `--data-dir`。相对目录参数相对于 CLI 当前目录解析，传给实例的是绝对路径。`add --queue ID` 可选择已有队列，默认主队列 ID 1。`--dir` 省略时按链接/指定文件名应用桌面当前分类规则，使用默认目录或分类目录；显式 `--dir` 覆盖分类规则；新增任务使用 HTTP(S)/FTP(S) 文件引擎。FTP 使用保存的连接默认值；登录信息需在桌面新建或任务属性中输入，URL 不接受用户名密码。

## 输出与退出码

控制命令始终输出一条 JSON；`--json` 还可让 help/version 输出 JSON。成功为 `{"ok":true,"data":...}`，失败为 `{"ok":false,"error":{"code":"...","message":"..."}}`，输出均在 stdout。无参数的 `swoop-cli` 显示帮助，无参数的 `swoop` 启动或唤醒桌面。

| 返回码 | 意义 |
| --- | --- |
| 0 | 请求已确认成功，或帮助/版本成功 |
| 2 | 命令、参数或 ID 无效 |
| 3 | 未找到运行实例、实例尚未就绪或通信未确认 |
| 4 | 服务明确拒绝命令，例如任务/队列不存在 |

`add` 成功只表示已加入引擎，下载状态通过 `status` 查看。同名文件默认等待确认，`status` 显示 `waiting_for_confirmation`；未决期间不会覆盖旧文件，也不占下载槽位。最终文件名/路径以状态为准，选择重命名后可能与 `--name` 不同。`status.data.tasks` 提供 ID、文件名/路径、状态、已下载字节、总量和速度；`queues` 提供队列 ID、名称、运行状态、并发数及成员。响应不输出源 URL、请求头或可能包含签名链接的失败详情。

通信失败后**不自动重发变更命令**。若 `add` 的响应丢失，先查看 `status`，避免重复创建任务。重复桌面启动在定位信息未就绪时等待最多约 3 秒，失败返回非零并保持数据库未打开。

## 本机控制协议

浏览器桥的 `GET /ping` 和 `POST /tasks` 保持原协议，见 `browser-extension.md`。CLI 不使用浏览器桥，不因关闭浏览器集成或修改其端口而失效。

实例另外绑定 `127.0.0.1` 的随机端口，将 `{protocol:1,port,token}` 写入数据目录下 `.swoop-control/locator.json`。该文件不用于判断所有权；客户端先检查 `.swoop-control/instance.lock` 的操作系统文件锁。正常退出清除 locator；启动持有锁后先清理崩溃留下的 locator。锁文件不删除，避免多个 inode 产生双实例。

控制请求是 `POST /v1/control`，要求精确的 loopback Host、`Content-Type: application/json` 及 `Authorization: Bearer TOKEN`。所有 `Origin`（包括浏览器扩展）以及 `Sec-Fetch-Site` 请求均拒绝，不提供 CORS。请求体上限 64 KiB，连接限时 5 秒，同时最多 16 条连接。客户端禁用代理和重定向，令牌只发到指定 loopback 端口，不写日志。

请求示例结构：`{"command":"status"}`、`{"command":"pause","id":12}`、`{"command":"add","url":"https://example.com/a.zip","directory":"/absolute/path","name":null,"queue":null}`。控制文件属于本机用户身份凭据，不应复制或共享。

Unix 控制目录权限为 `0700`，控制文件为 `0600`，拒绝其他用户可写的数据目录及控制文件符号链接。Windows 安装受保护 DACL，仅文件所有者拥有访问权限，并对新文件继承；ACL 设置失败则拒绝启动。自定义数据目录应位于当前用户拥有的本地目录；不支持把锁目录放到不能保证操作系统文件锁/ACL语义的共享文件系统。

## 验证边界

自动测试覆盖严格命令解析、真实子进程互斥/锁释放、目录隔离、过期 locator 拒绝、token/Origin 授权，以及真实 HTTP 控制端点到同一控制器线程的媒体并发添加、停止、重试、既有文件保护、抓取创建/停止/重新打开/完成/异步失败、状态脱敏和历史恢复。过期或接收方已关闭的命令不会在稍后执行。

`scripts/smoke-desktop.py PATH --with-cli-actions` 使用当前构建的真实桌面与 CLI，验证 HTTP 下载、完成扫描 fixture、媒体在清单阶段停止/重试、站点抓取项目完整生命周期，以及 SQLite 拒绝 settings 写入时的失败反馈。烟测必须针对修改后的双入口重新构建；脚本存在不等于已经通过。Windows 的文件锁、DACL、console 子系统与打包有实现和 CI 路径；没有 Windows 实机时保留验收项。

2026-10-11 同名策略批次 macOS 回归：GUI 71 项测试通过、1 项原生通知测试忽略，CLI 10 项测试通过；严格 workspace Clippy 通过。测试包含媒体三种同名决定、单次授权不跨重试保留、状态恢复、HTTP/媒体共享路径占用、零字节暂停分段保护及恢复文件错误在取消后仍可见。新 release 双入口真实应用烟测已通过，报告为 `target/iteration-validation/file-conflicts-release-smoke/desktop-smoke-bsj67ooo/report.json`，包含新增媒体同名和保存失败用例。媒体控制烟测停在清单阶段取消/重试，编码合并与覆盖发布由另行通过的 7 项真实 FFmpeg 测试验证。完整产物哈希见 [桌面验收记录](desktop-validation.md)。Windows 没有实机执行本轮流程。

当前没有 CLI 认证请求头、daemon 或退出命令；`status` 表示 HTTP(S)/FTP(S) 引擎任务与队列，媒体使用 `media-status`，抓取使用 `grabber-status`。CLI 与桌面共用控制器，不会创建第二套引擎。

2026-10-11 最新配置与协议批次：GUI 89、CLI 11 项通过，严格 workspace Clippy 通过；新增回归验证退出时显式释放文件锁，避免子进程继承句柄使过期 locator 被误认成存活实例。最新 release 包的 HTTP/CLI 与 FTP 真进程烟测均通过，报告分别为 `target/iteration-validation/configured-parity-release-smoke/desktop-smoke-o2nrg926/report.json` 和 `target/iteration-validation/configured-parity-ftp-release-smoke/ftp-desktop-uozkzz1b/report.json`。后者验证普通 `add` 使用保存的 FTP 默认值、同名询问及传输最终失败保留原件。两者均未验证原生 UI 点击；产物哈希及平台边界见 [桌面验收记录](desktop-validation.md)。

## 同名文件

默认策略为 `ask`（询问），桌面设置中的“同名文件”可改为“重命名 / 覆盖 / 跳过”。`conflict-policy` 查询；`conflict-policy ask|rename|overwrite|skip` 保存默认策略，只有数据库保存成功才生效。`status.data.file_conflict_policy` 同时返回当前策略。HTTP(S)/FTP(S) 文件任务与媒体任务共用这一默认策略。

遇到同名文件，任务进入 `waiting_for_confirmation`。桌面会唤回窗口，按任务 ID 稳定逐项显示实际文件名和目录；覆盖、重命名、取消对应单次决定，关闭弹窗仅取消当前任务。勾选“记住选择”时，先保存默认策略再执行当前决定；保存失败不会开始下载。等待确认会阻止“全部完成后”的退出/关机倒计时。

CLI 对 HTTP(S)/FTP(S) 任务使用 `resolve ID --action rename|overwrite|skip`，对媒体任务使用 `media-resolve ID --action rename|overwrite|skip`：rename 保留原文件并避让新文件名，overwrite 先下载到暂存文件，成功且旧文件指纹未变才替换，skip 保留原文件并暂停任务。单次 resolve 不改变默认策略；修改默认策略不会代替已等待任务的明确决定。响应 accepted 只表示本次决定已保存/派发，仍需轮询任务状态；旧文件在等待期间被更改时可能再次等待确认。`ask` 不能作为单次 resolve 操作，不存在或非冲突状态的任务返回非零。

HTTP(S)/FTP(S) 和媒体 worker 共用输出路径占用锁，正在写入的路径不能被另一个任务覆盖；HTTP 已暂停但仍保留分段数据时，也需移除原任务或选择重命名。重命名同时避让现存文件与正在运行的目标。

控制协议示例：`{"command":"media-resolve","id":1,"action":"skip"}`、`{"command":"resolve","id":12,"action":"rename"}`、`{"command":"conflict-policy","policy":null}`、`{"command":"conflict-policy","policy":"ask"}`。仍使用原有 token/loopback 鉴权，未向浏览器桥开放这些权限。

## 媒体与站点抓取

`media-add URL --output FILE` 创建媒体下载记录，输出必须是 `.mp4` 或 `.mkv`。同名时默认先等待确认；未冲突或明确选择后进入下载。可选 `--duration SEC` 限制直播录制秒数、`--max-height N` 限制视频高度（1–8640）、`--audio-language TAG` 设置首选音轨语言；未指定时使用桌面媒体默认配置及 FFmpeg 路径。只有合法 HTTP(S) 链接可用，URL 不得内嵌用户名密码，CLI 不接受请求头。

创建响应为 `{"ok":true,"data":{"accepted":true,"id":1}}`，表示任务记录已保存并进入现有媒体控制器，媒体是否可下载、FFmpeg 是否可用及最终完成情况仍需查询。`media-status [ID]` 返回 `tasks`（ID、输出路径、状态、字节数、媒体时长）以及 `persistence_error`。状态为 `waiting/waiting_for_confirmation/authentication_required/downloading/cancelling/paused/completed/failed`；`authentication_required` 表示需在桌面重新输入会话登录信息，凭据不随历史保存。`persistence_error:true` 表示最近一次历史保存失败，即使文件已完成也不能认为历史落库成功。进度不逐帧落库。`media-stop ID` 只接收运行中的任务，停止后需等状态变为 `paused`；`media-retry ID` 只接受已停止或失败的任务，从头重新下载。不存在的 ID、无效状态、目标被其他任务占用、队列或记录保存失败返回非零；默认询问产生的同名记录则返回 accepted，并等待单次决定。取消后的覆盖授权不会保留，重新下载仍按当前默认策略处理。

`grabber-create URL --dir PATH` **只创建项目，不发起页面抓取**。可选 `--depth 0..100`、`--pages 1..100000`、`--bytes N`、`--concurrency 1..16` 覆盖对应项目配置；其余使用桌面当前抓取配置（含 robots、额外域名、扩展名及超时规则）。新项目目录不得已有项目。`grabber-open PATH` 打开项目；`grabber-run` 使用当前已创建/打开项目保存的配置运行，忽略未保存的界面编辑；`grabber-stop` 请求停止当前工作。实例同时只能有一个抓取项目操作，繁忙时拒绝新建/打开/重复运行。

抓取创建/打开/运行响应为 `{"ok":true,"data":{"accepted":true,"operation_id":1}}`，仅表示已派发后台操作。随后查询 `grabber-status`，匹配 `operation.id`，等待 `busy:false`：操作状态为 `running/cancelling/succeeded/cancelled/failed`，`error:true` 表示项目操作或设置保存失败；项目状态另外显示 `ready/running/complete/cancelled/limited`、页数、失败页数、接收字节和离线入口路径。操作 succeeded 表示 worker 和设置保存成功；需同时检查项目 complete、failed 页数，不能把额度 limited 当作完整镜像。源 URL、页面 URL、请求头和可能含签名链接的原始错误详情不在控制响应中输出，可在桌面查看错误。项目文件保存可跨重启恢复；操作 ID 属于当前进程，重启后需用 `grabber-open` 打开项目再查询。

桌面命令排队最多约 4 秒，携带截止时间；过期或应答通道关闭后尚未执行的命令会被丢弃。若操作已开始，但客户端未收到确认，响应 `error.code:unconfirmed` 或通信中断，退出码为 3；这不证明操作没有执行。先用对应 status 查询，不得直接重复创建命令。CLI 不自动重试。`accepted` 永远不表示下载或抓取完成。

控制协议新增 command 为 `media-add/media-status/media-stop/media-retry/media-resolve/grabber-create/grabber-open/grabber-run/grabber-stop/grabber-status`，仅通过已有认证 `/v1/control` 使用，不扩展浏览器 `/tasks` 权限。例如 `{"command":"media-add","url":"https://example.com/a.m3u8","output":"/absolute/a.mp4","duration":null,"max_height":720,"audio_language":null}` 和 `{"command":"grabber-open","directory":"/absolute/site"}`。未知字段、越界参数和非绝对目标路径均拒绝。

## 浏览器扩展

`browsers list` 扫描已安装的浏览器（注册表卸载登记 / URL 协议 / 已知安装路径三路并集），输出每个浏览器的扩展登记状态：已登记 / 未安装 / 未知。仅 Windows 可用，不连接桌面实例。

`browsers install --all|--id ID [--silent] [--ext-dir PATH]` 把随安装包携带的扩展登记到识别到的浏览器，安装包（crx / xpi / swoop-extension-info.json）默认取 swoop-cli 所在目录，可用 `--ext-dir` 覆盖。策略按浏览器区分：Edge 写企业策略 `ExtensionInstallForcelist`（重启后从商店强制静默安装）；Firefox 写官方企业策略 `distribution/policies.json`（可靠）；360 极速/安全、QQ 等 Chromium 系写外部扩展注册表（是否生效取决于浏览器内核版本，可能无效）；Chrome 引导拖拽。`--silent` 时跳过引导类策略（不弹浏览器窗口）。

注意策略写入的权限：`HKCU\Software\Policies` 的默认 ACL 只允许管理员写入，Firefox 的 `distribution\policies.json` 也在 Program Files 下，未提权执行会得到“写入浏览器策略被拒绝”（退出码 1）。设置页的“一键安装”会在被拒时自动用 UAC 提权拉起 `swoop-cli browsers install --id <ID> --silent` 补登记；用户取消授权则降级为引导安装（Edge 打开商店页，Firefox 打开 about:addons 并定位 xpi）。

`browsers guide --id ID` 打开对应浏览器的扩展页并定位 crx 文件，引导用户拖拽安装；`browsers uninstall` 移除全部已登记项（注册表键 / policies.json 条目）。安装与卸载均输出 JSON 且尽力而为，单浏览器失败不影响其他浏览器。

Windows 安装包在安装收尾时自动执行 `browsers install --all --silent`（结果记入安装日志，失败会附一行提示），卸载时自动执行 `browsers uninstall`。安装器以 Highest 执行级别启动：管理员账号启动时即完成 UAC 提权，收尾登记能直接成功；普通账号不弹窗、此步失败属预期，用户可在设置页用“一键安装”提权补登记。

## 登录启动

设置中的“登录时启动”读取真实用户启动项；仅点击“应用”才写入或删除。普通启动和自动刷新均不修改系统配置。默认未配置，开发态二进制不可启用；macOS 要求 `.app/Contents/MacOS` 及 Info.plist，Windows 要求完整包中的 `swoop.exe`、`swoop-cli.exe` 和 build-info.txt。启动路径移动或丢失后会显示失效，可从新位置重新应用。

macOS 使用 `~/Library/LaunchAgents/com.muffin.swoop.plist` 和用户 `launchctl` enable/disable override，不使用管理员权限、不执行 bootstrap/bootout，不因启停登录项再次启动或关闭当前程序；下一次登录生效。Windows 使用 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 Swoop 值。Task Manager 的 StartupApproved 属系统内部格式，程序只识别已知启用/禁用状态，未知格式明确报错，始终不自行覆盖系统禁用标记；禁用过的应用可能需要在 Windows 的“启动应用”中允许后刷新。

这是当前用户唯一的 Swoop 启动项，只启动程序的默认数据目录；不继承启动设置界面时的临时 `SWOOP_DATA_DIR`。异步配置完成或失败都会反馈，退出会等配置线程结束。fixture 测试只写临时目录，包含 XML 特殊字符/中文空格路径、移动失效、关闭与失败保留原文件；没有启用开发机器真实登录启动项。


## MMS 录制

`media-add` 也接受 `mms://`、`mmsh://`、`mmst://`；必须显式提供 `--duration`（秒）和 `.mkv` 输出。`mms://` 使用媒体设置中保存的 HTTP/TCP 传输选择，默认 HTTP。地址不能内嵌账号密码；目前 MMS 不接受代理或会话认证，不能把失败误当作自动直连。

```sh
swoop-cli media-add mms://example.test/live --output /absolute/path/recording.mkv --duration 60
```

媒体重试会重新转封装；启用缓存时仅复用通过来源验证的完整分片，不把残缺分片或已有输出文件直接追加为成功。
