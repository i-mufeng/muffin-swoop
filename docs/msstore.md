# Microsoft Store 提交指南（MSIX）

Swoop 以 MSIX 桌面桥（full-trust）包提交 Microsoft Store。Store 收到上传的 MSIX 后由微软
重新签名并托管，因此**提交前不需要（也不应该）自签名**。

## 背景：从 Win32(EXE) 应用迁移

旧产品若是以 EXE/MSI 安装包 URL 形式提交的 Win32 应用（如认证报告 10.2.9 被拒的场景），
不能在 Partner Center 里原地改成 MSIX。微软要求的迁移路径是：

1. 在 Partner Center 中删除（或改名释放）旧 Win32 应用占用的应用名；
2. 新建 MSIX 打包应用，重新保留同一个应用名（如 "Swoop Download Manager"）；
3. 在新应用的提交中上传 MSIX 包。

## 打包

```powershell
pwsh -ExecutionPolicy Bypass -File scripts/package-msstore.ps1 -Profile release
```

产物：`target/packages/Swoop-<版本>-windows-<arch>-Store.msix`（未签名 + .sha256）。

可选参数：

| 参数 | 说明 |
| --- | --- |
| `-VersionOverride 0.7.1.0` | 四段版本号，每次上传必须严格递增 |
| `-PackageName` | 商店标识 Name，默认 `MufengDev.SwoopDownloadManager` |
| `-Publisher` | 发布者 CN，默认使用下方 Partner Center 标识 |
| `-PfxPath devcert.pfx` | 仅本地侧载测试用；**提交商店的包不要签名** |

## 首次提交前必须核对的两项标识

脚本默认值已是 Swoop Download Manager 在 Partner Center 的真实包标识，正常情况下无需
传参。如果将来换了商店保留名或发布者，按下述方式核对：

1. 在 Partner Center 打开应用的 **产品管理 → 产品标识（Product identity）** 页面；
2. 其中的 **Package/Identity/Name**（`MufengDev.SwoopDownloadManager`）对应 `-PackageName`；
3. 其中的 **Publisher**（`CN=A1AF5AE8-6831-460B-AA8A-8BD04BC2F9B0`）原样对应 `-Publisher`
   （含 `CN=` 前缀）。

当前标识：

| 项 | 值 |
| --- | --- |
| Package/Identity/Name | `MufengDev.SwoopDownloadManager` |
| Publisher | `CN=A1AF5AE8-6831-460B-AA8A-8BD04BC2F9B0` |
| PublisherDisplayName | `MufengDev` |
| DisplayName | `Swoop Download Manager`（必须与保留名一致，对应 `-DisplayName`） |

日常提交只需递增版本号：

```powershell
pwsh -File scripts/package-msstore.ps1 -VersionOverride 0.7.1.0
```

## 语言与商店页面

包清单声明了 8 种语言，覆盖商店中文/英文全部变体：

| 清单语言 | 对应的商店页面 |
| --- | --- |
| `zh-cn` | 中文(简体)(中国)、中文(中国)、中文(简体) |
| `zh-sg` | 中文(简体)(新加坡)、中文(新加坡) |
| `zh-hans` | 中文(简体) 通用 |
| `zh-tw` | 中文(繁体)(台湾)、中文(台湾)、中文(繁体) |
| `zh-hk` | 中文(繁体)(香港特别行政区)、中文(香港特别行政区) |
| `zh-mo` | 中文(澳门特别行政区)、中文(繁体)(澳门特别行政区) |
| `zh-hant` | 中文(繁体) 通用 |
| `en-us` | 英语(美国) |

**每声明一种语言，Partner Center 就要求完成该语言的商店页面**（描述为必填，截图为
可选但建议提供）。提交前确认「Store 一览」里上述语言没有「未完成」标记；中文页面之间
可以复用同一份文案。

## 提交

1. Partner Center → 新应用 → 开始提交 → **包** 页面上传 `.msix`；
2. 版本号不得与已上传过的重复或更低；
3. 商店托管后自动签名分发；云端认证（含 WACK）会在提交后运行。

## 本地预检（可选）

上传前可在本机跑 Windows App Certification Kit 预检：

```powershell
# 先用自签名 PFX 出一份可安装的包（仅测试用）
pwsh -File scripts/package-msstore.ps1 -PfxPath devcert.pfx -PfxPassword ...
# 安装开发证书（管理员、一次）：winapp cert install devcert.pfx 或 certlm.msc
Add-AppxPackage .\target\packages\Swoop-*-Store.msix
appcert.exe test -apppackagepath <msix路径> -reportoutputpath <报告目录>
```

MSIX 安装位置在 `Program Files\WindowsApps` 下；Swoop 的 per-user 数据逻辑路径仍为
`%APPDATA%\com.muffin.swoop`。下文列出的非虚拟化位置不会随卸载自动清理。

## 商店更新

商店版通过包渠道识别，启动时清除旧版保存的应用内自动检查开关，不访问站外更新源，
也不下载更新包或启动内部安装助手。设置页仅保留手动「检查更新」和商店入口。

手动检查使用 Windows `StoreContext.GetAppAndOptionalStorePackageUpdatesAsync`，
只查询当前包的可用更新。发现更新后显示「前往 Microsoft Store 更新」，跳转本应用
商品页，由商店负责下载和安装。断网、超时或身份错误会显示失败原因，不误报“已是最新”。
商店自身的自动更新设置仍由 Windows / Microsoft Store 管理。

该接口需要真实的商店包身份与授权；普通开发 exe 的单元测试覆盖渠道隔离和结果处理，
不能替代已签名商店包的在线更新验证。

## 浏览器集成

- 打包必须有 Node.js / npm，构建失败或负载缺失即中止。包内包含 CRX、XPI、扩展信息、
  Chromium 已解压目录和 `swoop-channel.txt` 渠道标记。
- 首次显示主窗口时尝试登记 Edge 自动安装；不在后台启动时弹出引导或 UAC。登记失败显示
  可展开的原因，用户可从欢迎页进入「设置 → 集成」重试或打开商店。登记成功并不等于
  浏览器已安装，Edge 仍需运行、联网并接受相关策略。
- Chrome 使用开发者模式加载 `swoop-extension` 文件夹。新包直接包含此目录；若扩展负载只有 CRX，新版引导会从 CRX3 包解压到用户数据目录后再打开该文件夹，不会提示安装 CRX。Firefox 正式版需要 Mozilla
  签名的 XPI。构建生成的 XPI 本身不带 Mozilla 签名，不能宣称可在正式版中静默安装。
- MSIX 将扩展同步到 `%APPDATA%\com.muffin.swoop\extension`，避免版本化 WindowsApps
  路径在更新后失效。同版本重打包、文件丢失或复制中断也会修复。
- 使用 `unvirtualizedResources` 让包外浏览器看到登记和负载。Windows 11 仅排除扩展目录、
  Edge 的 ExtensionInstallForcelist 和 Chromium 扩展登记键；Windows 10 使用 desktop6
  的注册表/AppData 非虚拟化回退。最低系统版本因此为 Windows 10 1903。
  **提交 Partner Center 时需要说明该受限能力用于浏览器互操作，是否获批以商店审核为准。**
- 排除位置不会随 MSIX 卸载自动清理，可用 `swoop-cli browsers uninstall` 移除扩展登记。

机制参考：[MSIX flexible virtualization](https://learn.microsoft.com/en-us/windows/msix/desktop/flexible-virtualization)。

## 内存验证

Windows 默认使用 OpenGL 渲染，避免空闲下载器同时初始化 Vulkan、DX12 及多套厂商驱动。
兼容性排查可设置 `SWOOP_RENDERER=wgpu`；wgpu 默认使用 DX12、低功耗显卡和节省内存的分配策略，
也可用 `WGPU_BACKEND` 显式覆盖后端。

主窗口隐藏且无下载、抓取、更新、独立弹窗等活动时，等待 5 秒后请求 Windows 回收一次
工作集。每段连续空闲期只做一次，不用定时清空制造低读数；下载结束后重新等待空闲再回收。
唤回窗口后按需恢复页面。
**工作集下降代表减少物理驻留，不代表所有提交内存已经释放。**

复测应同时记录任务管理器对应的私有工作集、总工作集和 Private Bytes，并分别覆盖
启动、进入托盘后 10/30/60 秒、唤回、开始下载和下载完成。测试使用独立 `SWOOP_DATA_DIR`；
不要结束用户正在运行的商店进程来腾出桥接端口。

2026-10-21 本机验证（MiB；独立数据目录、发布版、无可见窗口、浏览器桥接开启，
使用独立端口验证 `/ping`；剪贴板监听、通知及浮窗关闭，避免干扰正在使用的桌面）：

| 场景 | 私有工作集 | 总工作集 | Private Bytes |
| --- | ---: | ---: | ---: |
| 已安装商店 0.7.0，空闲 | 约 232 | 约 350 | 约 544 |
| 新发布版，后台 15 秒 | 2.32 | 6.91 | 81.98 |
| 新发布版，后台 60 秒 | 3.29 | 9.53 | 78.79 |
| 后台下载，约 1 MiB/s | 3.89 | 12.82 | 78.90 |
| 下载完成后空闲 | 2.83 | 8.73 | 78.91 |

后台下载使用本机 HTTP 服务提供的 16 MiB 固定内容，完成后核对大小及 SHA-256。
低于 20 MiB 的目标在这次托盘空闲工作集测试中达到；不是提交内存、所有显卡、
大任务列表或打开浮窗场景的保证。发布版按未注册的 exe 运行；商店身份、扩展写入
可见性及在线 Store 更新查询仍需在签名后的 MSIX 中复核。
