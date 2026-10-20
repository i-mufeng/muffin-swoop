# Swoop 桌面打包与双平台验收

应用名称为 **Swoop**，应用标识为 **`com.muffin.swoop`**。桌面版本直接读取 Cargo workspace，当前为 `0.1.0`；浏览器扩展使用自己的 manifest/package 版本，不能拿扩展版本替代桌面版本。

推荐每个平台原生构建和验证：macOS 生成 `.app`、zip 和 DMG，Windows 生成可解压运行的 zip 和按用户安装的 NSIS 安装包。安装包由 [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) 0.11.8 生成；zip 仍是签名更新使用的包格式，安装包只负责首次安装。脚本使用 `Cargo.lock`，不修改用户设置、系统注册表或安装目录，不签名、公证、发布、上传、启动应用，不覆盖已有产物。这里的可重现指相同锁文件和命令可再次构建、核对哈希；未承诺不同机器、不同工具链产物逐字节一致。

## 本地构建

### macOS

需要 Rust 1.95 或更新版本、Xcode Command Line Tools 和 cargo-packager（`cargo install cargo-packager --version 0.11.8 --locked`）。`Swoop.app` 由 cargo-packager 生成，zip 与 DMG 共用同一份；其余步骤使用系统自带的 Bash、`plutil`、`ditto`、`lipo`、`shasum`、`hdiutil`。

cargo-packager 首次生成 DMG 时会从 GitHub 下载固定提交的 `create-dmg` 脚本到用户缓存目录。本地运行时它用 AppleScript 让 Finder 摆放 DMG 窗口图标，系统可能询问是否允许终端控制 Finder；设置 `CI=true` 会跳过这一步（DMG 内容不变，只是窗口不排版）。脚本最后会只读挂载 DMG，核对其中的 `Swoop.app` 与 zip 内完全一致，并确认有「应用程序」快捷方式。

```bash
# 原生架构 release 构建；M 系列 Mac 产出 arm64，Intel Mac 产出 x86_64。
bash scripts/package-macos.sh

# 开发验收复用已有 debug 二进制，输出路径支持空格和中文。
cargo build -p swoop-app --locked
bash scripts/package-macos.sh --profile debug --skip-build \
  --output-dir "target/桌面验收 packages"
```

`--skip-build` 只核对文件存在、可执行和原生 CPU 架构，不证明二进制与当前源码一致。正式记录应先构建，再记录二进制哈希；共享工作区可能在构建后继续变化，包内不会声称 Git revision 就是精确源码快照。默认产物位于 Cargo target 目录的 `packages/`：

```text
Swoop-0.1.0-macos-arm64-release/
  Swoop.app/Contents/
    Info.plist
    MacOS/swoop
    Resources/FONT-NOTICE.md
  swoop-cli
  README.md
  cli.md
  desktop-validation.md
  build-info.txt
Swoop-0.1.0-macos-arm64-release.zip
Swoop-0.1.0-macos-arm64-release.zip.sha256
Swoop-0.1.0-macos-arm64-release.dmg
Swoop-0.1.0-macos-arm64-release.dmg.sha256
```

DMG 只含 `Swoop.app` 和「应用程序」快捷方式，打开后拖入即可；`swoop-cli` 只在 zip 中提供。也可用 Finder 把 zip 里的 `Swoop.app` 复制到自己的 `~/Applications/` 后打开，不必写入系统 `/Applications/`，不需要管理员权限。重新打包时指定新的输出目录；脚本会拒绝覆盖旧包。用 `shasum -a 256 -c <zip>.sha256` 核对压缩包，命令在校验文件所在目录执行。

两平台脚本都以输出目录内的 `.<包名>.lock/` 防止同名并发打包；已有同名包、zip、安装包或校验文件时，新包在临时目录完整生成后才覆盖它们，打包失败则旧包保持不动。正常退出会释放自己的锁，异常断电或强制结束后可能留锁，此时先确认没有打包进程，再选择新输出目录或检查残留锁。脚本不会自动删除其他进程的锁。

这是本地开发包，没有 Developer ID 签名与公证；Rust/linker 自带的临时签名不等于发布签名。从网络收到的包可能被 Gatekeeper 阻止，因此不能据此宣称普通用户下载后即可无提示安装。验收时记录系统实际提示，不要全局关闭 Gatekeeper 或清除整机安全设置。当前 `.app` 使用 A「折光」的 ICNS，托盘使用同轮廓的单色模板。

macOS 通知需要系统能解析应用标识。若日志明确显示 `set_application` 失败，先从 Finder 打开 `.app`，而非只运行其内部可执行文件；再检查系统“通知”中的 Swoop 权限并重新点“测试通知”。可用以下只读命令确认 LaunchServices 是否已有应用记录；有记录只证明应用身份可查，不证明横幅已经展示。本轮没有主动注册应用或改变通知权限。

```bash
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -dump \
  | rg -n -C 3 'com\.muffin\.swoop'
```

### Windows

构建机需要 MSVC 版 Rust 1.95 或更新版本、Visual Studio C++ Build Tools / Windows SDK、PowerShell 5.1 或更新版本，以及 cargo-packager（`cargo install cargo-packager --version 0.11.8 --locked`）。cargo-packager 首次生成安装包时会把 NSIS 3.09 工具链和插件下载到用户缓存目录（带哈希校验）。安装编译工具可能需要管理员，但运行产物不需要这些构建工具。

```powershell
# 从仓库根目录执行。默认 release，也可 -Profile debug。
# Windows 默认禁止运行脚本，-ExecutionPolicy Bypass 只对本次进程生效；`powershell`（5.1）同样可用。
pwsh -ExecutionPolicy Bypass -File scripts/package-windows.ps1
pwsh -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Profile debug -OutputDir "target/桌面验收 packages"
```

脚本运行期间把控制台输出编码切到 UTF-8，退出时恢复，避免中文 Windows 按 GBK 解码 `cargo metadata` 的 JSON 而解析失败。脚本显式按本机 MSVC target 构建，并请求静态链接 CRT，减少对系统级 VC 运行库安装的依赖；原有 `RUSTFLAGS` 会在退出时恢复。若设置了优先级更高的 `CARGO_ENCODED_RUSTFLAGS`，脚本会报错，避免静默绕过静态链接参数。目标为 x64 或 ARM64，不交叉编译，不把 macOS 构建当作 Windows 包。

两个 EXE 均在 Cargo 构建时嵌入 A「折光」文件图标，唯一来源是 `assets/brand/swoop/swoop.ico`。无需随包放置 ICO，资源编译器缺失或编译失败时会报错。Windows CI 打包后使用以下检查逐帧比对七种尺寸（检查需要 Python 3，打包本身不依赖 Python）：

```powershell
python scripts/verify-windows-icon.py path/to/swoop.exe path/to/swoop-cli.exe
```

该检查证明 PE 内图标与母版一致；Explorer、快捷方式和任务栏的实际显示及旧图标缓存仍需 Windows 桌面验收。

```text
Swoop-0.1.0-windows-x86_64-release/
  swoop.exe
  swoop-cli.exe
  README.md
  cli.md
  desktop-validation.md
  FONT-NOTICE.md
  build-info.txt
Swoop-0.1.0-windows-x86_64-release.zip
Swoop-0.1.0-windows-x86_64-release.zip.sha256
Swoop-0.1.0-windows-x86_64-release-setup.exe
Swoop-0.1.0-windows-x86_64-release-setup.exe.sha256
```

`-setup.exe` 是 NSIS 安装包（`installMode = both`）：安装器以 Highest 执行级别启动，管理员账号会看到一次 UAC 授权（用于写入 Edge/Firefox 的扩展登记策略），普通账号不弹窗、仍可纯按用户安装。默认按当前用户装到 `%LOCALAPPDATA%\Swoop\`（可在安装模式页选“为所有用户安装”到 Program Files），写入对应范围的「已安装应用」卸载项。安装目录与 zip 一样把 `swoop.exe`、`swoop-cli.exe`、`build-info.txt` 放在一起，签名更新可以原地替换。安装界面按系统语言显示简体中文或英文。卸载时可勾选删除应用数据 `%APPDATA%\com.muffin.swoop`，默认不勾选；已下载的文件不会被删除。静默安装用 `setup.exe /S /NS`（默认当前用户，可加 `/AllUsers`），`/NS` 表示不建快捷方式，`/D=路径` 可改安装目录（必须放在最后且不加引号）。CI 会做一次静默安装、校验文件哈希、运行已安装的 CLI，然后静默卸载。

安装界面用的是自定义模板 `scripts/windows-installer/installer.nsi`，在 cargo-packager 0.11.8 默认模板的基础上修改，改动处都标了 `Swoop:`；升级 cargo-packager 时要对照上游模板合并。主要改动：声明 DPI 感知，高分屏上不再被系统按位图拉伸；中文界面用 Microsoft YaHei UI，英文用 Segoe UI，替换语言文件里默认的宋体 / MS Shell Dlg 点阵字体；页面精简为欢迎 → 目录 → 安装 → 完成，开始菜单快捷方式直接放进「开始」的应用列表，完成页可勾选创建桌面快捷方式、安装后启动。侧栏和页眉的品牌图由 `python3 scripts/make_installer_art.py` 生成 100%–250% 共六档 PNG（`assets/brand/swoop/installer/`），打包脚本再转成 24 位 BMP；安装器运行时按控件的实际像素尺寸挑选一档，避免小图被放大。

已知限制：自更新只替换文件，「已安装应用」里显示的版本号停留在安装时的版本；卸载不会清除应用自己写入的开机启动项，也不会删除更新器留在安装目录中的 `.swoop-backup-*` 备份，这时目录会被保留。

把 zip 解压到当前用户可写目录，例如 `%LOCALAPPDATA%\Programs\Swoop\` 或 `%USERPROFILE%\Downloads\Swoop 验收\`，直接双击 `swoop.exe`。不使用“以管理员身份运行”，不注册服务、不改系统 PATH。哈希用 `Get-FileHash <zip> -Algorithm SHA256` 与 `.sha256` 比较。debug 版本出现控制台是预期行为，release 版本使用 Windows GUI 子系统。

Windows zip 与安装包都没有 Authenticode 签名，从网络下载后运行可能触发 SmartScreen 提示；应用标识写入包的 `build-info.txt`，不代表已经注册了 Windows Toast 的系统身份。SmartScreen、缺失 DLL、通知身份与实际标准用户运行都必须在干净 Windows 桌面验证。不得把静态 CRT 参数视作“所有运行时依赖均已验证”。

### 浏览器扩展

扩展独立构建，不自动安装到浏览器；扩展更新时保留原加载目录。

```bash
cd extension
npm ci
npm test
npm run typecheck
npm run build
```

Chromium 系浏览器在扩展管理页加载 `extension/dist/`。Firefox 适配若提供独立 `dist-firefox/`，按其清单与临时加载流程验证；不能只凭 Chromium 测试通过就标记 Firefox 已验收。桌面桥接端口与扩展设置必须一致，默认 `51230`。

## CI

`.github/workflows/ci.yml` 在 macOS、Windows 原生 runner 分别执行：

1. `cargo test --workspace --locked`。
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`。
3. `cargo build --workspace --locked --release`。
4. 扩展 `npm ci`、`npm test`、`npm run typecheck`、`npm run build`。
5. 本平台打包脚本（含 DMG / NSIS 安装包）；Windows 另做安装包静默安装与卸载烟测。

Windows job 设置明确的本机 Cargo target 和静态 CRT 参数，构建、测试、Clippy 与打包采用同一目标。任何步骤失败都会使该 job 失败，没有忽略错误或降低 Clippy 门槛。两个平台独立完成，某一平台失败不会取消另一平台。

工作流允许 push、pull request 和手动运行；新增配置文件本身不等于已运行 CI。默认不上传产物；仅手动运行时勾选 `upload_artifacts` 才上传未签名包和扩展 zip（`swoop-extension-{chromium,firefox}-<版本>.zip` 及 `.sha256`，由 `npm run package` 生成）到该次 GitHub Actions run，保留 7 天，不创建 Release。Action 引用固定到已核实的 commit，运行权限为 `contents: read`。Runner 编译通过仍不能替代原生桌面验收。

## 两个平台都要执行的真实桌面清单

每次记录平台/版本/CPU、Git revision 与工作区改动、Rust/Node 版本、包哈希、浏览器版本、操作步骤、日志与实际结果。以下各项分别填写 macOS 与 Windows 结果，未做写“未执行”，不要以单元测试或另一个平台的结果代填。

| 场景 | 操作与通过条件 |
| --- | --- |
| 标准用户首次运行 | 使用未提权账户从含空格、中文的安装目录启动；界面、字体、托盘正常；记录 Gatekeeper/SmartScreen/缺 DLL 等实际提示。 |
| 配置与数据路径 | 改主题、下载目录、连接数、限速、通知开关、关闭到托盘、自动续传和桥接端口；完整退出后重启，确认设置保留。独立验收可设 `SWOOP_DATA_DIR`，但默认路径也必须测。 |
| 中文和空格路径 | 下载到如 `下载 验收/报告 中文 01.bin`；列表、打开文件、打开目录、重新打开程序与续传均保留正确路径；校验最终哈希。Windows 同时检查含 Unicode 的用户目录。 |
| 关闭后后台继续 | 启动一个限速的大文件任务，开启关闭到托盘，关闭窗口；托盘存在、速度继续变化、服务端持续收到数据；完整下载并校验内容。关闭该选项后再关窗应退出。 |
| 重新打开窗口 | 通过托盘“显示”或“新建”唤回，正在下载的任务和设置仍在；macOS 检查 Dock 图标恢复。再用 Finder/开始菜单入口打开已运行应用，记录是否正确唤回或产生多实例；尚未实现的行为应明确记失败。 |
| 后台通知 | 将通知时机设为“仅后台”，失焦/最小化/隐藏时分别完成、制造失败；观察实际系统横幅与身份名称。前台不应出现后台模式通知；完成/失败独立开关生效；暂停、删除、恢复历史任务不冒出旧通知。 |
| 通知权限受限 | 点“测试通知”，允许通知后检查横幅；再关闭系统权限或开启勿扰，下载不能卡住。API 返回成功但没显示横幅不算展示通过；记录系统原因与用户可见结果。 |
| 暂停与断点续传 | 对支持 Range 的本地受控 HTTP 文件下载、暂停、继续；完整退出并重启后再次继续。记录 Range/ETag 请求、已完成字节与 SHA-256；资源变化应安全重下，不拼接新旧内容。 |
| 浏览器桥接下载 | 在各平台 Chrome/Edge 下载本地受控文件；桌面收到后浏览器不重复下载。需要 Cookie/Referer 的受控资源也应成功，日志不展示凭据。桥接关闭/端口不一致/桌面未运行时浏览器应正常回退。 |
| 隐藏时桥接确认 | 设置“弹出确认”、关闭窗口到托盘，再从扩展发起下载；应唤回窗口并显示预填任务，确认后只有一份下载。 |
| 完整退出与再次启动 | 从托盘菜单退出（macOS 另测 ⌘Q），进程、端口与托盘资源均释放；再次启动能读取历史任务，不出现数据库锁或幽灵图标。 |

默认数据库路径：macOS 为 `~/Library/Application Support/com.muffin.swoop/swoop.db`，Windows 为 `%APPDATA%\com.muffin.swoop\swoop.db`。覆盖目录示例：

```bash
SWOOP_DATA_DIR="$PWD/target/验收 数据" \
  "target/桌面验收 packages/Swoop-0.1.0-macos-arm64-debug/Swoop.app/Contents/MacOS/swoop"
```

```powershell
$env:SWOOP_DATA_DIR = Join-Path $PWD 'target/验收 数据'
& '.\target\packages\Swoop-0.1.0-windows-x86_64-release\swoop.exe'
Remove-Item Env:SWOOP_DATA_DIR
```

## 验收边界

本文件提供开发打包与验证流程，不宣称已完整对标 IDM。代码签名、公证、签名安装器、更新发布、浏览器商店上架、Windows 原生通知身份注册及完整系统集成仍需各自实现与实证。单实例与开机启动已实现，Windows 系统行为仍需原生验证。macOS 本地脚本产物布局、Rust/扩展检查、Windows CI 和双平台实机交互分别记账，只有对应证据齐全才能把该项标记通过。

### 2026-10-11 本地打包验证记录

| 检查 | 结果 |
| --- | --- |
| macOS arm64 debug 打包 | `bash scripts/package-macos.sh --profile debug --skip-build --output-dir 'target/桌面验收 packages'` 退出码 0；复用已有开发二进制，未运行 release 编译。 |
| Bundle 结构与标识 | `plutil -lint` 为 OK；标识 `com.muffin.swoop`，版本 `0.1.0`；包内程序与源二进制 SHA-256 一致。 |
| zip 完整性与哈希 | `unzip -t` 无错误；`shasum -a 256 -c` 为 OK。 |
| 参数与覆盖保护 | 无效 profile、缺输出参数、重复相同输出均退出码 1；重复打包后旧 zip 哈希未改变。 |
| CI 文件 | Ruby YAML 解析通过，确认双平台矩阵和固定 Action commit；未向 GitHub 推送或触发工作流。 |
| Windows 脚本 | 已静态检查流程；本机没有 PowerShell 或 Windows，未执行 PowerShell 语法解析、编译、打包或 Windows 实机测试。 |
| 桌面交互 | 本记录仅验证打包；托盘、通知、桥接与续传的实机结果应按上表另附证据。 |

本次 debug 程序 SHA-256：`6182691a0c2f56acae3e2ad5e5e00fb3718388e0717efa2ed3b6b616a1614aae`。本次 zip SHA-256：`15227a528b05cbd4c2ee17b628718299fdda49ad4f60a4dedb339ecf9d70e510`。此记录只对应上述一次产物，后续构建应另记新哈希。

CI Action 用法核对来源：[checkout](https://github.com/actions/checkout)、[setup-node](https://github.com/actions/setup-node)、[upload-artifact](https://github.com/actions/upload-artifact)、[rust-toolchain](https://github.com/dtolnay/rust-toolchain)。

### 2026-10-11 中断后复核

仅复用上述已有 debug 二进制，没有触发 Cargo 编译。`bash -n` 通过；旧包 `Info.plist`、`unzip -t` 和二进制哈希再次核对通过，旧 zip 哈希未改变。

新增同名输出锁后，两个并发调用的退出码分别为 `0` 和 `1`，失败输出为 `Package output is locked`。无效 profile、缺参数、已有产物、预存锁、断链符号链接目标分别验证为拒绝；预存锁仍保留，正常结束的临时目录和自己的锁已清理。成功产物的 `unzip -t` 与 `shasum -a 256 -c` 均通过。Windows 使用相同锁定流程，但仍未执行 Windows/PowerShell 验证。

复核产物路径：`target/桌面 打包复核-7f6ko7h6/Swoop-0.1.0-macos-arm64-debug/Swoop.app`。复核 zip SHA-256：`480628119f728c49be5d865f80d20e44369f82133575ffb19918bb1492fe95ee`，程序 SHA-256 仍为 `6182691a0c2f56acae3e2ad5e5e00fb3718388e0717efa2ed3b6b616a1614aae`。该包内文档是打包时副本，本节记录在打包后加入，以仓库文档为准。

四个固定 Action commit 均在官方仓库确认存在，其 `action.yml` 读取结果均为 HTTP 200，工作流所用输入名称也已核对：[checkout](https://github.com/actions/checkout/commit/3d3c42e5aac5ba805825da76410c181273ba90b1)、[setup-node](https://github.com/actions/setup-node/commit/949feb2413d6458794dcd2491c4babbbce0c15c1)、[upload-artifact](https://github.com/actions/upload-artifact/commit/cf430e030ddbb5b0abf93d22962f4752f3646cd9)、[rust-toolchain](https://github.com/dtolnay/rust-toolchain/commit/686976e191b89faba57d3206551f0f330d8cb249)。没有推送或触发远端 CI。

只读 LaunchServices 查询已找到 `com.muffin.swoop` 和对应 `NOTIFICATION#` 记录；本轮没有点击系统通知测试、修改通知权限、安装或启动应用，不能把这条记录当作横幅、托盘或桌面交互通过。


### 2026-10-11 统一回归与 release 基线

后续开发开始前，实际执行 `cargo test --workspace --locked`：128 项通过、4 项默认忽略；3 项真实 FFmpeg 用例另行显式执行并全部通过，剩余 1 项为人工原生通知用例。严格 Clippy 通过；扩展 48 项、TypeScript 与 Chromium/Firefox 构建通过。

`CARGO_BUILD_JOBS=4 cargo build -p swoop-app --release --locked` 退出码 0。基线包位于 `target/iteration-validation/release-baseline/Swoop-0.1.0-macos-arm64-release/Swoop.app`；zip 完整性通过、程序与刚构建 release 二进制 SHA-256 相同。程序 SHA-256：`c150b0c7ae26e286c2977e83333b57f1d74e1b1b1dfda892fa06cfcdf7db3b53`；zip SHA-256：`93fa924eacfdd9985b2cc448f0ffda07f3b716f472c23c1f57da4dd4b33aa6f6`。这是后续 CLI、同步队列、完成动作、离线资源增强之前的基线，不代表最新源码。

实际启动该包内部程序，使用独立数据目录和受控 HTTP Range 服务执行：

```bash
python3 scripts/smoke-desktop.py 'target/iteration-validation/release-baseline/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop'
```

退出码 0。浏览器桥接创建任务，分类路由到 `Fixtures`，队列成员和完成状态写入 SQLite；1,048,595 字节文件逐字节一致。媒体桥接交给媒体窗口草稿，没有创建普通 HTTP manifest 下载。报告 `target/iteration-validation/desktop-smoke-oys7qv7g/report.json`，文件 SHA-256：`65bdbc667e92a48c71b6955bcaee73b358af34c2e8c91f21d02948e3d458c4ba`。

原生自动化返回 `Computer Use permissions are still pending`（辅助功能与屏幕录制授权未完成），未通过替代自动化绕过。开发渲染截图 `target/iteration-validation/swoop-overview.png` 验证主布局，使用演示任务，不代替真实原生交互。Windows 与双浏览器加载接管未执行。

### 2026-10-11 同名配置、字幕与文件保护验收包

本批源码冻结后运行 `cargo test --workspace --locked`：231 passed、8 ignored；另用 `target/media-tools/subtitles/ffmpeg-9.0.1/{ffmpeg,ffprobe}` 显式执行全部 7 项真实媒体测试，7 passed。剩余忽略项为人工系统通知。严格 workspace Clippy、扩展 48 项测试、TypeScript、Chromium/Firefox 构建均通过。Rust 实际版本为 1.98.1；Windows MSVC 目标下同名目标登记与文件工具代码独立 Clippy 通过，完整 Windows 应用构建和实机仍未执行。

`CARGO_BUILD_JOBS=4 cargo build -p swoop-app --release --bins --locked` 退出码 0；随后打包脚本退出码 0。新产物：

- `target/iteration-validation/file-conflicts-release/Swoop-0.1.0-macos-arm64-release/Swoop.app`
- 同目录独立 `swoop-cli`；压缩包为其上级目录的 `Swoop-0.1.0-macos-arm64-release.zip`。
- GUI SHA-256：`993c1dccf00808b9095eebcfb498c7e1ce38bb731fa6e52370e9a0c017cd8900`。
- CLI SHA-256：`36b77c85fcc0b7413091f1653793a9901e3ed798ad3b11e0edf6b3778e0c0d4d`。
- zip SHA-256：`ee054ec21406fd8a40184c2e1b64858ab4d07540c1f1f3a9e7660cf3e7c300c3`。

包内两个二进制与新构建哈希相同，Info.plist、zip 完整性和校验文件均通过。旧验收包未替换；本节记录在新包生成后加入，以仓库文档为准。该包仍未签名、公证或上传。

使用新包运行：

```bash
python3 scripts/smoke-desktop.py \
  target/iteration-validation/file-conflicts-release/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop \
  --with-cli-actions --output-dir target/iteration-validation/file-conflicts-release-smoke
```

退出码 0，报告 `target/iteration-validation/file-conflicts-release-smoke/desktop-smoke-bsj67ooo/report.json`。真实应用进程验证了桥接与分类、单实例/CLI、HTTP 和媒体同名三种决定、策略保存及数据库拒绝写入、取消/重试、站点抓取控制与无害扫描器夹具。普通文件 1,048,595 字节逐字节一致，SHA-256 为 `65bdbc667e92a48c71b6955bcaee73b358af34c2e8c91f21d02948e3d458c4ba`。

报告明确 `native_ui_verified: false`；未执行原生弹窗按钮、托盘/横幅、系统关机、实际开机启动设置、真实双浏览器加载接管或 Windows 实机。所有进程使用隔离数据目录，仅清理烟测自己启动的进程。

### 2026-10-11 FTP、媒体认证与可配置列表验收包

本批增加 FTP/FTPS 统一引擎及桌面连接配置、卡片/表格与列/工具栏配置、HLS 点播认证和代理。默认同名询问及文件保护同时覆盖 HTTP(S)、FTP(S) 和媒体。源码冻结后执行：

| 检查 | 实际结果 |
| --- | --- |
| `cargo test --workspace --locked` | 295 passed、0 failed、11 ignored；GUI 89、CLI 11、core 131、FTP 20、grabber 14、media 30。 |
| 真实 FFmpeg | 显式执行 `swoop-media --test real_media -- --ignored`，10 passed；测试工具为 `target/media-tools/subtitles/ffmpeg-9.0.1/{ffmpeg,ffprobe}`。仅人工系统通知仍未执行。 |
| 严格 Clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` 退出码 0。 |
| 扩展 | 53 项测试通过；TypeScript、Chromium 与 Firefox 构建通过。 |
| release 双入口 | `CARGO_BUILD_JOBS=4 cargo build -p swoop-app --release --bins --locked` 退出码 0，实际 Rust 1.98.1。 |
| macOS 包 | 打包脚本退出码 0；两个程序与刚构建二进制哈希一致，Info.plist、zip 内容和 SHA-256 校验通过；`otool -L` 仅系统库与框架。 |

主要日志为 `target/iteration-validation/configured-parity-final-{tests,clippy,real-media}.log`、`configured-parity-release-build.log`、`ftp-extension-final.log`。新包使用独立输出目录，旧包未替换：

- 应用：`target/iteration-validation/configured-parity-release/Swoop-0.1.0-macos-arm64-release/Swoop.app`。
- CLI：同目录 `swoop-cli`；zip：其上级目录 `Swoop-0.1.0-macos-arm64-release.zip`。
- GUI SHA-256：`cba2c3114668b0165a90c63a7eee44f6170dd9540d7d102e874a4010acfc66f5`。
- CLI SHA-256：`b326245a83e9469e3b3e97e6f6b6d25c459214c787940ea6ccc6fa03541858a2`。
- zip SHA-256：`aaff08e27b307a100e12ca7e1377b71c3c3721c872a6fb26d0153d918fec31ab`。

对该包内部程序实际运行两组隔离烟测，退出码均为 0：

```bash
python3 scripts/smoke-desktop.py \
  target/iteration-validation/configured-parity-release/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop \
  --with-cli-actions --output-dir target/iteration-validation/configured-parity-release-smoke
python3 scripts/smoke-ftp-desktop.py \
  target/iteration-validation/configured-parity-release/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop \
  --output-dir target/iteration-validation/configured-parity-ftp-release-smoke
```

HTTP/CLI 报告：`target/iteration-validation/configured-parity-release-smoke/desktop-smoke-o2nrg926/report.json`。真实应用验证桥接分类、单实例/CLI、普通与媒体同名决定、设置写入失败、媒体取消/重试、抓取控制和扫描器夹具；1,048,595 字节逐字节一致，SHA-256 为 `65bdbc667e92a48c71b6955bcaee73b358af34c2e8c91f21d02948e3d458c4ba`。

FTP 报告：`target/iteration-validation/configured-parity-ftp-release-smoke/ftp-desktop-uozkzz1b/report.json`。真实应用验证保存的 FTP 默认配置、队列停止/启动、默认同名询问与覆盖，以及收完数据后服务器返回 426 时任务失败并保留用户原件；1,048,576 字节逐字节一致，SHA-256 为 `fbbab289f7f94b25736c58be46a994c441fd02552cc6022352e3d86d2fab7c83`。

两份报告均为 `native_ui_verified: false`。原生弹窗/拖动/托盘/通知、真实浏览器交接、Windows 原生编译与桌面验收仍未执行；macOS 上尝试 FTP Windows 交叉构建在 `libz-sys` 阶段报 `sys/types.h file not found`，不计作通过。CI 已补 Python FTP 夹具依赖，但未推送或触发远端工作流。未启用系统登录启动、未关机、未签名公证或上传；所有烟测使用隔离数据，仅结束自身进程。本节在打包后补录，包内文档为打包时副本，以仓库最新记录为准。


### 2026-10-11 IDM 收尾、认证、动态媒体与签名更新验收包

默认同名策略继续为 **询问**。本批接入站点/代理认证、动态站点渲染、媒体直播与完整分片恢复、MMS、原生拖放、中英文、签名更新与配置入口；实际支持范围见 [对标矩阵](idm-parity.md) 与 [配置汇总](configuration.md)。

| 检查 | 实际结果 |
| --- | --- |
| `cargo test --workspace --all-targets --locked` | **361 passed、0 failed、21 ignored**。GUI 103、CLI 12、core 148、FTP 20、grabber 19、i18n 2、media 34、updater 23（含离线签名工具 5）。 |
| 显式外部工具用例 | 真实 FFmpeg **16**、真实 Chromium **3**、实际 ZIP 安装 **1** 均通过；仅人工系统通知未执行。ZIP 父/子宿主输出不重复计数。 |
| 严格 Clippy / rustdoc | 两项命令退出码均为 0；rustdoc 当前无可执行示例，不计额外测试数。 |
| 浏览器扩展 | **65** 测试、TypeScript、Chromium 与 Firefox 构建通过。 |
| 语言资源 / 内部渲染 | AST 检查 803 项界面文案，英文无缺失；英文详情标签宽度修复后内部截图通过。 |
| release / 打包 | GUI 与 CLI release 构建、打包脚本退出码 0；实际 Rust 1.98.1。zip 完整性、校验文件、Info.plist、包内与新构建程序哈希相同；`otool -L` 仅系统库与框架。 |
| 实际应用进程 | HTTP/CLI/媒体/抓取及独立 FTP 两组隔离烟测退出码均为 0。 |

日志：`target/iteration-validation/idm-completion-final-{tests,clippy}.log`、`idm-completion-doc-tests.log`、`media-completion-final-real.log`、`idm-final-chromium.log`、`idm-completion-release-build.log`、`idm-completion-package-update-smoke.log`。英文布局检查为 `target/iteration-validation/i18n/english-preview-89jk0vlg/overview-fixed.png`，使用演示数据，不属于原生手势验收。

独立交付目录，未覆盖旧包：

- 应用：`target/iteration-validation/idm-completion-release/Swoop-0.1.0-macos-arm64-release/Swoop.app`。
- CLI：同目录 `swoop-cli`；zip：上级目录 `Swoop-0.1.0-macos-arm64-release.zip`（15,001,773 字节）。
- GUI SHA-256：`b9cf958b82749573e89f62a5eb0cf1af52def07fee39b90d5917dcc9bd71ddb4`。
- CLI SHA-256：`358380d80fc0d32666a59b8f1bc5f9b356a6569a1505a1fe7aed21bd34e306eb`。
- zip SHA-256：`d6efbf65ad0ee3613e0868ba9c98761004549cc66bcbf1a3e6edd286af89978e`。
- 校验明细：`target/iteration-validation/idm-completion-release/verification.json`。包内带配置、认证、媒体、抓取和更新说明，保留原文档目录结构。

实际执行：

```bash
python3 scripts/smoke-desktop.py \
  target/iteration-validation/idm-completion-release/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop \
  --with-cli-actions --output-dir target/iteration-validation/idm-completion-release-smoke
python3 scripts/smoke-ftp-desktop.py \
  target/iteration-validation/idm-completion-release/Swoop-0.1.0-macos-arm64-release/Swoop.app/Contents/MacOS/swoop \
  --output-dir target/iteration-validation/idm-completion-ftp-release-smoke
SWOOP_UPDATE_PACKAGE="$PWD/target/iteration-validation/idm-completion-release/Swoop-0.1.0-macos-arm64-release.zip" \
  cargo test -p swoop-updater --locked --test package_smoke -- --ignored --nocapture
```

HTTP/CLI 报告：`target/iteration-validation/idm-completion-release-smoke/desktop-smoke-vi8wg0qc/report.json`。覆盖桥接分类、独立 CLI 与单实例、媒体交接和控制、抓取控制、扫描器夹具、设置写入失败、普通与媒体同名处理。1,048,595 字节逐字节一致，SHA-256：`65bdbc667e92a48c71b6955bcaee73b358af34c2e8c91f21d02948e3d458c4ba`。

FTP 报告：`target/iteration-validation/idm-completion-ftp-release-smoke/ftp-desktop-00tls6kc/report.json`。保存的默认配置、队列起停、同名询问及覆盖、服务器最后返回失败时保留原件均通过；1,048,576 字节逐字节一致，SHA-256：`fbbab289f7f94b25736c58be46a994c441fd02552cc6022352e3d86d2fab7c83`。

实际 ZIP 更新检查使用原样包、公开测试 seed 和相同生产源码的临时 `0.0.0` 宿主，执行验签、准备、应用与幂等重入。完整应用文件与执行权限、独立 CLI、2 份原件备份、其他用户文件及原始 ZIP 不变均通过。没有启动或替换用户现有安装；这是当前源码更新流程与包兼容证明，不是历史正式版升级实测。

两份应用报告明确 `native_ui_verified: false`。Windows 完整原生编译/打包/桌面、macOS 弹窗/拖放手势/托盘/通知、双浏览器真实加载接管、系统凭据授权、企业域、MMST 服务器及正式代码签名/公证/更新源/商店发布继续保留。媒体的代理账号、TTML/封装 wvtt、任意分片内字节续传和带宽限制仍不支持，MMS 仅直连；细项以媒体说明为准。未启用登录启动、关机、部署、上传、提交或推送。本节在打包后补录，包内验收文档是打包时快照，以仓库最新记录为准。

### 2026-10-11 安装包（cargo-packager）验证记录

| 项目 | 结果 |
| --- | --- |
| macOS arm64 release | `CI=true bash scripts/package-macos.sh --profile release` 退出码 0，产出 zip、DMG 及各自 SHA-256。DMG 通过 `hdiutil verify`，只读挂载后其中的 `Swoop.app` 与 zip 内完全一致，并有「应用程序」快捷方式。Info.plist 版本为 `0.1.0`，标识为 `com.muffin.swoop`，包内 `swoop` 与新构建的二进制哈希相同。 |
| 更新包兼容 | 用临时 seed 运行 `sign_feed` 对新 zip 签名，包内 `build-info.txt` 的版本、平台、GUI/CLI 哈希检查通过。签名后已删除临时 seed，没有生成生产密钥。 |
| Windows NSIS | 本机只用假二进制验证了 cargo-packager 生成的 `installer.nsi`：按用户安装，带三件套、`FONT-NOTICE.md`、简体中文/英文界面，应用数据仅在勾选后删除。Homebrew 的 makensis 3.13 在 macOS 上编译任意包含 `StrFunc.nsh` 的脚本都会 `bad_alloc` 崩溃，所以本机没有编出 setup.exe。真实安装包、静默安装/卸载烟测和界面检查都要等 Windows CI 或实机。 |
| 未执行 | 未在本机做 Finder 窗口排版（`CI=true` 时跳过）；未签名、公证，也未上传。 |

### 2026-10-12 Windows 安装界面验证记录

| 项目 | 结果 |
| --- | --- |
| 本机编译 | 用假二进制和 PIL 转出的 BMP 跑 `cargo packager --formats nsis`，自定义模板经 makensis 3.13 编译通过，只有非 Windows 主机上 `File /a` 的 3 条警告，原版模板也有这 3 条。生成的 setup.exe 中 manifest 含 `dpiAware true`（原版没有）；`SetFont` 对 2052 / 1033 分别生效；六档侧栏和页眉 BMP 都已打进安装器与卸载器。 |
| 更正 | 上一节「makensis 3.13 遇到 `StrFunc.nsh` 就 `bad_alloc`」的结论不准确：在 `C` locale 下连空脚本也会崩溃，设置 `LC_ALL=en_US.UTF-8` 后可正常编出完整的 setup.exe。 |
| 未执行 | 未在 Windows 上实际查看界面（字体渲染、各档 DPI 下的选图、卸载页复选框位置），也未跑静默安装烟测，这些要等 Windows CI 或实机验证。 |
