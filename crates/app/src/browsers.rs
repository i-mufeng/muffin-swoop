//! 浏览器扩展识别与安装：swoop-cli `browsers` 子命令与 GUI 设置页共用。

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::io::Read;

#[cfg(windows)]
use swoop_browser_bridge::{DetectedBrowser, ExtensionInfo, ExtensionStatus, InstallOutcome};

/// GUI 列表里的一行。
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct BrowserRow {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub status: ExtensionStatus,
    pub action: RowAction,
}

/// 手动安装引导所需的可用文件和浏览器信息。
#[cfg(windows)]
pub struct GuideInfo {
    pub id: String,
    pub name: String,
    pub unpacked: Option<PathBuf>,
}

/// 这一行能给用户的动作。
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// 无可做的（已登记，或平台不支持）。
    None,
    /// 静默登记安装（注册表 / policies.json）。
    Install,
    /// 引导拖拽安装（打开扩展页 + 定位 crx）。
    Guide,
    /// 打开官方商店页。
    Store,
}

/// CLI 子命令动作。
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    List,
    Install { all: bool, id: Option<String>, silent: bool, ext_dir: Option<PathBuf> },
    Uninstall { ext_dir: Option<PathBuf> },
    Guide { id: String, ext_dir: Option<PathBuf> },
}

/// 打包渠道标记文件：MSIX 打包脚本会在 exe 旁写入 `msix-store`（scripts/package-msstore.ps1）。
#[cfg(windows)]
const CHANNEL_MARKER: &str = "swoop-channel.txt";

/// 当前进程是否运行在 MSIX 包里（Microsoft Store / 侧载）。
/// 首选 exe 旁的渠道标记；开发期直接跑包目录里的二进制时退回路径特征判断。
#[cfg(windows)]
pub fn is_msix() -> bool {
    static MSIX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MSIX.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else { return false };
        let Some(dir) = exe.parent() else { return false };
        if channel_marker(dir).is_some_and(|c| c.starts_with("msix")) {
            return true;
        }
        exe.to_string_lossy().contains("\\WindowsApps\\")
    })
}

/// 读取目录里的渠道标记内容（没有标记时返回 None）。
#[cfg(windows)]
fn channel_marker(dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(dir.join(CHANNEL_MARKER)).ok()?;
    Some(content.trim().to_string())
}

/// 把扩展负载（crx / xpi / info.json）从安装目录同步到稳定目录。
/// MSIX 包的安装路径带版本号，更新后旧目录被删除；注册表里的登记必须指向
/// 每用户稳定位置，否则应用一更新浏览器里的扩展就失效。返回是否有文件被更新。
#[cfg(windows)]
fn sync_payload(source: &Path, target: &Path) -> Result<bool, String> {
    use swoop_browser_bridge::{CRX_FILE, INFO_FILE, XPI_FILE};
    let staged_info = source.join(INFO_FILE);
    let staged = std::fs::read(&staged_info).map_err(|e| format!("{}: {e}", staged_info.display()))?;
    // 同版本重打包、文件丢失或上次复制中断也要修复；info 最后写，不能把半份负载标记为完成。
    let payload = [CRX_FILE, XPI_FILE]
        .into_iter()
        .map(|name| std::fs::read(source.join(name)).map(|bytes| (name, bytes)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let unpacked = source.join("swoop-extension");
    let directory_updated = if unpacked.is_dir() {
        sync_directory(&unpacked, &target.join("swoop-extension")).map_err(|e| e.to_string())?
    } else {
        false
    };
    if std::fs::read(target.join(INFO_FILE)).is_ok_and(|current| current == staged)
        && payload.iter().all(|(name, bytes)| std::fs::read(target.join(name)).is_ok_and(|current| current == *bytes))
    {
        return Ok(directory_updated);
    }
    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
    for (name, bytes) in payload {
        std::fs::write(target.join(name), bytes).map_err(|e| e.to_string())?;
    }
    std::fs::write(target.join(INFO_FILE), staged).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(windows)]
fn sync_directory(source: &Path, target: &Path) -> std::io::Result<bool> {
    std::fs::create_dir_all(target)?;
    let mut updated = false;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let dest = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            updated |= sync_directory(&entry.path(), &dest)?;
        } else if entry.file_type()?.is_file() {
            let bytes = std::fs::read(entry.path())?;
            if !std::fs::read(&dest).is_ok_and(|current| current == bytes) {
                std::fs::write(dest, bytes)?;
                updated = true;
            }
        }
    }
    Ok(updated)
}

/// 首次进入时只尝试 Edge 的自动登记，不弹 UAC 或浏览器窗口。
/// 权限不足等失败由欢迎页显示，用户可在设置中主动安装。
#[cfg(windows)]
pub fn prepare_first_run(auto_edge: bool) -> (Vec<BrowserRow>, Option<String>) {
    let ext = match load_ext(&None) {
        Ok(ext) => ext,
        Err(error) => return (scan_rows(), Some(error)),
    };
    let defs = swoop_browser_bridge::browsers();
    let detected = swoop_browser_bridge::detect_installed(&defs);
    let mut error = None;
    if auto_edge
        && let Some(edge) = detected.iter().find(|b| b.id() == "edge")
        && swoop_browser_bridge::status(edge, &ext) != ExtensionStatus::Installed
        && let Err(e) = swoop_browser_bridge::install_extension(edge, &ext)
    {
        error = Some(e.to_string());
    }
    (scan_rows(), error)
}

/// 默认扩展目录：安装包把 crx/xpi 释放在可执行文件旁边；MSIX 包则同步到
/// 数据目录下的 `extension/`（稳定、每用户可写），登记与状态查询都指向那里。
#[cfg(windows)]
fn default_ext_dir() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or_else(|| swoop_i18n::tr!("无法确定可执行文件目录").to_string())?;
    if !is_msix() {
        return Ok(exe_dir);
    }
    let target = crate::instance::data_dir().join("extension");
    sync_payload(&exe_dir, &target)?;
    Ok(target)
}

#[cfg(windows)]
fn load_ext(ext_dir: &Option<PathBuf>) -> Result<ExtensionInfo, String> {
    let dir = match ext_dir {
        Some(dir) => dir.clone(),
        None => default_ext_dir()?,
    };
    ExtensionInfo::load(&dir).map_err(|e| e.to_string())
}

/// 老版商店包只有 CRX，没有可供 Chrome「加载已解压的扩展程序」使用的文件夹。
/// 将 CRX3 头部之后的 ZIP 解到每用户目录；manifest 最后写，失败时下次可以修复。
#[cfg(windows)]
fn unpack_crx(crx: &Path, target: &Path) -> Result<(), String> {
    use std::path::Component;

    const LIMIT: u64 = 16 * 1024 * 1024;
    let bytes = std::fs::read(crx).map_err(|e| e.to_string())?;
    if bytes.len() < 16 || bytes.len() > LIMIT as usize || &bytes[..4] != b"Cr24" || bytes[4..8] != 3_u32.to_le_bytes()
    {
        return Err(swoop_i18n::tr!("扩展 CRX 格式无效").into());
    }
    let header = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let start = 12_usize
        .checked_add(header)
        .filter(|start| *start + 4 <= bytes.len())
        .ok_or(swoop_i18n::tr!("扩展 CRX 格式无效"))?;
    if &bytes[start..start + 4] != b"PK\x03\x04" {
        return Err(swoop_i18n::tr!("扩展 CRX 格式无效").into());
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes[start..]))
        .map_err(|_| swoop_i18n::tr!("扩展 CRX 格式无效"))?;
    if zip.len() > 256 {
        return Err(swoop_i18n::tr!("扩展文件数量异常").into());
    }
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|e| e.to_string())?;
        let path = entry.enclosed_name().ok_or(swoop_i18n::tr!("扩展包含无效路径"))?;
        if path.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err(swoop_i18n::tr!("扩展包含无效路径").into());
        }
        if entry.is_dir() {
            continue;
        }
        if !seen.insert(path.to_string_lossy().to_lowercase()) {
            return Err(swoop_i18n::tr!("扩展包含重复文件").into());
        }
        let declared_size = entry.size();
        total =
            total.checked_add(declared_size).filter(|size| *size <= LIMIT).ok_or(swoop_i18n::tr!("扩展文件过大"))?;
        let mut content = Vec::new();
        entry.take(LIMIT + 1).read_to_end(&mut content).map_err(|e| e.to_string())?;
        if content.len() as u64 != declared_size {
            return Err(swoop_i18n::tr!("扩展文件大小不符").into());
        }
        files.push((path, content));
    }
    let manifest = files
        .iter()
        .find(|(path, _)| path == Path::new("manifest.json"))
        .ok_or(swoop_i18n::tr!("扩展缺少 manifest.json"))?;
    let parsed: serde_json::Value = serde_json::from_slice(&manifest.1).map_err(|e| e.to_string())?;
    if parsed.get("manifest_version").and_then(serde_json::Value::as_u64) != Some(3) {
        return Err(swoop_i18n::tr!("扩展清单格式无效").into());
    }
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for (path, content) in files.iter().filter(|(path, _)| path != Path::new("manifest.json")) {
        let dest = target.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(dest, content).map_err(|e| e.to_string())?;
    }
    std::fs::write(target.join("manifest.json"), &manifest.1).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(windows)]
fn chrome_guide_ext(ext: &ExtensionInfo) -> Result<(ExtensionInfo, PathBuf), String> {
    let source = ext.crx.parent().ok_or(swoop_i18n::tr!("无法确定扩展目录"))?;
    let bundled = source.join("swoop-extension");
    if manifest_matches_version(&bundled, &ext.version) {
        return Ok((ext.clone(), bundled));
    }
    let stable = crate::instance::data_dir().join("extension");
    let unpacked = stable.join("swoop-extension");
    if !manifest_matches_version(&unpacked, &ext.version) {
        unpack_crx(&ext.crx, &unpacked)?;
    }
    std::fs::create_dir_all(&stable).map_err(|e| e.to_string())?;
    let copied_crx = stable.join(swoop_browser_bridge::CRX_FILE);
    if !std::fs::read(&copied_crx)
        .is_ok_and(|current| std::fs::read(&ext.crx).is_ok_and(|original| original == current))
    {
        std::fs::copy(&ext.crx, &copied_crx).map_err(|e| e.to_string())?;
    }
    let mut guided_ext = ext.clone();
    guided_ext.crx = copied_crx;
    Ok((guided_ext, unpacked))
}

#[cfg(windows)]
fn manifest_matches_version(dir: &Path, version: &str) -> bool {
    std::fs::read(dir.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|manifest| manifest.get("version").and_then(serde_json::Value::as_str).map(str::to_owned))
        .is_some_and(|found| found == version)
}

#[cfg(windows)]
fn guide_browser(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<(InstallOutcome, Option<PathBuf>), String> {
    if browser.id() == "chrome" {
        let (guided_ext, folder) = chrome_guide_ext(ext)?;
        let outcome = swoop_browser_bridge::guide(browser, &guided_ext).map_err(|e| e.to_string())?;
        Ok((outcome, Some(folder)))
    } else {
        let outcome = swoop_browser_bridge::guide(browser, ext).map_err(|e| e.to_string())?;
        Ok((outcome, None))
    }
}

/// 扫描并生成 GUI 行。
#[cfg(windows)]
pub fn scan_rows() -> Vec<BrowserRow> {
    scan_rows_impl()
}

#[cfg(windows)]
fn scan_rows_impl() -> Vec<BrowserRow> {
    let defs = swoop_browser_bridge::browsers();
    let detected = swoop_browser_bridge::detect_installed(&defs);
    let ext = default_ext_dir().ok().and_then(|dir| ExtensionInfo::load(&dir).ok());
    detected
        .iter()
        .map(|b| {
            let status = ext.as_ref().map(|e| swoop_browser_bridge::status(b, e)).unwrap_or(ExtensionStatus::Unknown);
            BrowserRow {
                id: b.id().to_string(),
                name: b.name().to_string(),
                version: b.version.clone(),
                status,
                action: action_for(b, status),
            }
        })
        .collect()
}

#[cfg(windows)]
fn action_for(browser: &DetectedBrowser, status: ExtensionStatus) -> RowAction {
    use swoop_browser_bridge::InstallStrategy;
    if status == ExtensionStatus::Installed {
        return RowAction::None;
    }
    match browser.def.install {
        InstallStrategy::StoreLink => RowAction::Store,
        InstallStrategy::GuidedDrag => RowAction::Guide,
        InstallStrategy::FirefoxPolicy | InstallStrategy::ChromiumExternalExt | InstallStrategy::StoreForceList => {
            RowAction::Install
        }
    }
}

/// 对指定 id 的浏览器执行一键安装（GUI 按钮用）。
/// 流程：先试静默登记；策略键被拒绝（默认 ACL 下 HKCU\Software\Policies 只读）时，
/// 自动用 UAC 提权拉起 swoop-cli 完成登记；用户取消或提权失败再降级为打开商店页。
/// 可能弹 UAC、等待用户操作，调用方应放到后台线程执行。
#[cfg(windows)]
pub fn install_by_id(id: &str) -> Result<String, String> {
    let defs = swoop_browser_bridge::browsers();
    let detected = swoop_browser_bridge::detect_installed(&defs);
    let browser = detected.iter().find(|b| b.id() == id).ok_or_else(|| swoop_i18n::trf!("未检测到浏览器：{id}"))?;
    let ext = load_ext(&None)?;
    match swoop_browser_bridge::install_extension(browser, &ext) {
        Ok(outcome) => Ok(outcome_label(&outcome).to_string()),
        Err(swoop_browser_bridge::BridgeError::AccessDenied(reason)) => {
            tracing::info!("静默登记被拒（{reason}），尝试提权安装 {}", browser.id());
            match elevated_install(id) {
                ElevatedInstall::Done => Ok(outcome_label(&InstallOutcome::Registered).to_string()),
                ElevatedInstall::Cancelled => guide_fallback(browser, &ext, swoop_i18n::tr!("已取消管理员授权").into()),
                ElevatedInstall::Failed(reason) => {
                    guide_fallback(browser, &ext, swoop_i18n::trf!("自动安装失败（{reason}）"))
                }
                ElevatedInstall::Unavailable(reason) => {
                    guide_fallback(browser, &ext, swoop_i18n::trf!("无法请求管理员权限（{reason}）"))
                }
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 提权失败后的兜底：打开商店页（Edge）或扩展页并定位安装包（Firefox / Chromium 系），
/// 引导用户手动完成；引导也失败则把原因一并告知。
#[cfg(windows)]
fn guide_fallback(browser: &DetectedBrowser, ext: &ExtensionInfo, reason: String) -> Result<String, String> {
    use swoop_browser_bridge::InstallStrategy;
    let note = match browser.def.install {
        InstallStrategy::StoreForceList | InstallStrategy::StoreLink => {
            swoop_i18n::trf!("{reason}，已打开商店页，点击“获取”完成安装")
        }
        _ => swoop_i18n::trf!("{reason}，已打开扩展页并定位安装包，请按提示完成安装"),
    };
    match guide_browser(browser, ext) {
        Ok(_) => Ok(note),
        Err(e) => Err(swoop_i18n::trf!("{note}（引导打开失败：{e}）").to_string()),
    }
}

/// 提权拉起 swoop-cli 的结果。
#[cfg(windows)]
enum ElevatedInstall {
    /// 已登记（退出码 0）。
    Done,
    /// 用户在 UAC 对话框点了“否”。
    Cancelled,
    /// 提权进程跑了但失败（退出码非 0 或超时）。
    Failed(String),
    /// 根本没能发起提权（找不到 swoop-cli.exe / ShellExecute 失败）。
    Unavailable(String),
}

/// 用 runas 动词拉起 `swoop-cli browsers install --id <id> --silent`，等待其结束。
/// 隐藏控制台窗口；UAC 对话框由系统弹出，用户取消时 ShellExecuteEx 直接失败。
#[cfg(windows)]
fn elevated_install(id: &str) -> ElevatedInstall {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    /// 提权进程本身只做两次注册表写入，正常毫秒级；留足用户看 UAC 弹窗后的进程调度余量
    const WAIT: Duration = Duration::from_secs(120);

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let cli = match std::env::current_exe() {
        Ok(exe) => match exe.parent().map(|dir| dir.join("swoop-cli.exe")) {
            Some(path) if path.is_file() => path,
            _ => return ElevatedInstall::Unavailable(swoop_i18n::tr!("找不到命令行组件 swoop-cli.exe").into()),
        },
        Err(e) => return ElevatedInstall::Unavailable(e.to_string()),
    };
    let file = wide(&cli.to_string_lossy());
    let verb = wide("runas");
    // 传稳定负载路径，避免提权账户或 MSIX 上下文变化导致辅助进程找错目录。
    let ext_dir = match default_ext_dir() {
        Ok(dir) => dir,
        Err(error) => return ElevatedInstall::Unavailable(error),
    };
    let args = wide(&format!("browsers install --id {id} --silent --ext-dir \"{}\"", ext_dir.display()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: args.as_ptr(),
        nShow: 0, // SW_HIDE：swoop-cli 是控制台程序，不闪窗口
        ..Default::default()
    };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code = unsafe { GetLastError() };
        return if code == ERROR_CANCELLED {
            ElevatedInstall::Cancelled
        } else {
            ElevatedInstall::Unavailable(format!("{}", std::io::Error::from_raw_os_error(code as i32)))
        };
    }
    if info.hProcess.is_null() {
        return ElevatedInstall::Unavailable(swoop_i18n::tr!("无法跟踪提权进程").into());
    }
    let started = Instant::now();
    let result = loop {
        match unsafe { WaitForSingleObject(info.hProcess, 500) } {
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                let ok = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
                break if ok != 0 && code == 0 {
                    ElevatedInstall::Done
                } else {
                    ElevatedInstall::Failed(swoop_i18n::trf!("提权进程退出码 {code}"))
                };
            }
            WAIT_TIMEOUT if started.elapsed() >= WAIT => {
                break ElevatedInstall::Failed(swoop_i18n::tr!("等待提权进程超时").into());
            }
            WAIT_TIMEOUT => {}
            other => break ElevatedInstall::Failed(format!("WaitForSingleObject = {other}")),
        }
    };
    unsafe {
        CloseHandle(info.hProcess);
    }
    result
}

/// 对指定 id 的浏览器执行引导安装。
#[cfg(windows)]
pub fn guide_by_id(id: &str) -> Result<GuideInfo, String> {
    let defs = swoop_browser_bridge::browsers();
    let detected = swoop_browser_bridge::detect_installed(&defs);
    let browser = detected.iter().find(|b| b.id() == id).ok_or_else(|| swoop_i18n::trf!("未检测到浏览器：{id}"))?;
    let ext = load_ext(&None)?;
    let (_, unpacked) = guide_browser(browser, &ext)?;
    Ok(GuideInfo { id: browser.id().into(), name: browser.name().into(), unpacked })
}

#[cfg(windows)]
pub fn outcome_label(outcome: &InstallOutcome) -> &'static str {
    match outcome {
        InstallOutcome::Registered => swoop_i18n::tr!("已登记，重启浏览器后生效"),
        InstallOutcome::Guided => swoop_i18n::tr!("已打开引导，请按提示完成安装"),
        InstallOutcome::Skipped(reason) => match reason.as_str() {
            "store" => swoop_i18n::tr!("请从官方商店安装"),
            _ => swoop_i18n::tr!("已跳过"),
        },
    }
}

#[cfg(windows)]
pub fn status_label(status: ExtensionStatus) -> &'static str {
    match status {
        ExtensionStatus::Installed => swoop_i18n::tr!("已登记"),
        ExtensionStatus::NotInstalled => swoop_i18n::tr!("未安装"),
        ExtensionStatus::Unknown => swoop_i18n::tr!("未知"),
    }
}

/// CLI 入口。返回进程退出码。
pub fn run(action: Action, json: bool) -> u8 {
    match run_inner(action, json) {
        Ok(code) => code,
        Err(error) => {
            println!("{}", crate::instance::failure("browser_bridge", &error));
            1
        }
    }
}

fn run_inner(action: Action, json: bool) -> Result<u8, String> {
    #[cfg(not(windows))]
    {
        let _ = (action, json);
        Err(swoop_browser_bridge::BridgeError::Unsupported("浏览器扩展集成仅支持 Windows".into()).to_string())
    }
    #[cfg(windows)]
    {
        let defs = swoop_browser_bridge::browsers();
        let detected = swoop_browser_bridge::detect_installed(&defs);
        match action {
            Action::List => {
                let rows: Vec<_> = {
                    let ext = load_ext(&None).ok();
                    detected
                        .iter()
                        .map(|b| {
                            let status = ext
                                .as_ref()
                                .map(|e| swoop_browser_bridge::status(b, e))
                                .unwrap_or(ExtensionStatus::Unknown);
                            serde_json::json!({
                                "id": b.id(),
                                "name": b.name(),
                                "version": b.version,
                                "status": status_label(status),
                            })
                        })
                        .collect()
                };
                if json {
                    println!("{}", serde_json::json!({"ok": true, "data": {"browsers": rows}}));
                } else if rows.is_empty() {
                    println!("{}", swoop_i18n::tr!("未检测到支持的浏览器"));
                } else {
                    for row in rows {
                        println!(
                            "{}  {}  {}",
                            row["name"].as_str().unwrap_or(""),
                            row["version"].as_str().unwrap_or(""),
                            row["status"].as_str().unwrap_or(""),
                        );
                    }
                }
                Ok(0)
            }
            Action::Install { all, id, silent, ext_dir } => {
                let ext = load_ext(&ext_dir)?;
                let targets: Vec<&DetectedBrowser> = match (&id, all) {
                    (Some(id), _) => {
                        let b = detected
                            .iter()
                            .find(|b| b.id() == id)
                            .ok_or_else(|| swoop_i18n::trf!("未知的浏览器：{id}"))?;
                        vec![b]
                    }
                    (None, true) => detected.iter().collect(),
                    (None, false) => return Err(swoop_i18n::tr!("install 需要 --all 或 --id").into()),
                };
                let mut results = Vec::new();
                let mut code = 0u8;
                for browser in targets {
                    // 静默模式不弹任何窗口：引导类策略直接跳过
                    if silent
                        && matches!(
                            browser.def.install,
                            swoop_browser_bridge::InstallStrategy::StoreLink
                                | swoop_browser_bridge::InstallStrategy::GuidedDrag
                        )
                    {
                        results.push(serde_json::json!({
                            "id": browser.id(),
                            "outcome": swoop_i18n::tr!("已跳过"),
                        }));
                        continue;
                    }
                    let outcome = if browser.id() == "chrome" && !silent {
                        guide_browser(browser, &ext).map(|(outcome, _)| outcome)
                    } else {
                        swoop_browser_bridge::install_extension(browser, &ext).map_err(|e| e.to_string())
                    };
                    match outcome {
                        Ok(outcome) => {
                            // 静默模式跳过引导类策略；交互模式自动降级到引导
                            let final_outcome = if silent && outcome == InstallOutcome::Guided {
                                InstallOutcome::Skipped("guided".into())
                            } else {
                                outcome
                            };
                            results.push(serde_json::json!({
                                "id": browser.id(),
                                "outcome": outcome_label(&final_outcome),
                            }));
                        }
                        Err(e) => {
                            code = 1;
                            results.push(serde_json::json!({
                                "id": browser.id(),
                                "error": e.to_string(),
                            }));
                        }
                    }
                }
                println!("{}", serde_json::json!({"ok": code == 0, "data": {"results": results}}));
                Ok(code)
            }
            Action::Uninstall { ext_dir } => {
                let ext = load_ext(&ext_dir)?;
                for browser in &detected {
                    let _ = swoop_browser_bridge::uninstall_extension(browser, &ext);
                }
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "data": {"removed": detected.iter().map(|b| b.id()).collect::<Vec<_>>()}})
                );
                Ok(0)
            }
            Action::Guide { id, ext_dir } => {
                let ext = load_ext(&ext_dir)?;
                let browser =
                    detected.iter().find(|b| b.id() == id).ok_or_else(|| swoop_i18n::trf!("未知的浏览器：{id}"))?;
                let (outcome, unpacked) = guide_browser(browser, &ext)?;
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "data": {"id": browser.id(), "outcome": outcome_label(&outcome), "unpacked": unpacked}})
                );
                Ok(0)
            }
        }
    }
}

#[cfg(all(test, windows))]
mod payload_tests {
    use super::*;
    use std::io::Write;
    use swoop_browser_bridge::{CRX_FILE, INFO_FILE, XPI_FILE};

    #[test]
    fn repairs_missing_and_rebuilt_payload_even_with_unchanged_info() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        for name in [INFO_FILE, CRX_FILE, XPI_FILE] {
            std::fs::write(source.path().join(name), name).unwrap();
        }
        let unpacked = source.path().join("swoop-extension");
        std::fs::create_dir(&unpacked).unwrap();
        std::fs::write(unpacked.join("manifest.json"), "{}").unwrap();
        assert!(sync_payload(source.path(), target.path()).unwrap());
        assert!(!sync_payload(source.path(), target.path()).unwrap());
        std::fs::remove_file(target.path().join(CRX_FILE)).unwrap();
        assert!(sync_payload(source.path(), target.path()).unwrap());
        std::fs::write(source.path().join(XPI_FILE), "rebuilt").unwrap();
        assert!(sync_payload(source.path(), target.path()).unwrap());
        assert_eq!(std::fs::read(target.path().join(XPI_FILE)).unwrap(), b"rebuilt");
        std::fs::write(unpacked.join("manifest.json"), "{\"version\":\"2\"}").unwrap();
        assert!(sync_payload(source.path(), target.path()).unwrap());
        assert_eq!(std::fs::read(target.path().join("swoop-extension/manifest.json")).unwrap(), b"{\"version\":\"2\"}");
    }

    #[test]
    fn incomplete_source_does_not_publish_info() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join(INFO_FILE), "new version").unwrap();
        std::fs::write(source.path().join(CRX_FILE), "crx").unwrap();
        assert!(sync_payload(source.path(), target.path()).is_err());
        assert!(!target.path().join(INFO_FILE).exists());
    }

    #[test]
    fn extracts_crx3_as_loadable_chrome_folder() {
        let temp = tempfile::tempdir().unwrap();
        let crx = temp.path().join(CRX_FILE);
        let target = temp.path().join("swoop-extension");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(br#"{"manifest_version":3,"version":"0.7.0","name":"Swoop"}"#).unwrap();
        writer.start_file("background.js", options).unwrap();
        writer.write_all(b"export {};").unwrap();
        let archive = writer.finish().unwrap().into_inner();
        let mut package = b"Cr24".to_vec();
        package.extend_from_slice(&3_u32.to_le_bytes());
        package.extend_from_slice(&0_u32.to_le_bytes());
        package.extend_from_slice(&archive);
        std::fs::write(&crx, package).unwrap();

        unpack_crx(&crx, &target).unwrap();
        assert_eq!(std::fs::read(target.join("background.js")).unwrap(), b"export {};");
        assert_eq!(std::fs::read(target.join("manifest.json")).unwrap(), br#"{"manifest_version":3,"version":"0.7.0","name":"Swoop"}"#);
    }
}
