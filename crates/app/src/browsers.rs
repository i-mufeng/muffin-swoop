//! 浏览器扩展识别与安装：swoop-cli `browsers` 子命令与 GUI 设置页共用。

use std::path::PathBuf;

use swoop_browser_bridge::{DetectedBrowser, ExtensionInfo, ExtensionStatus, InstallOutcome};

/// GUI 列表里的一行。
#[derive(Debug, Clone)]
pub struct BrowserRow {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub status: ExtensionStatus,
    pub action: RowAction,
}

/// 这一行能给用户的动作。
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

/// 默认扩展目录：可执行文件旁边（安装包会把 crx/xpi 释放在那里）。
fn default_ext_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or_else(|| swoop_i18n::tr!("无法确定可执行文件目录").to_string())
}

fn load_ext(ext_dir: &Option<PathBuf>) -> Result<ExtensionInfo, String> {
    let dir = match ext_dir {
        Some(dir) => dir.clone(),
        None => default_ext_dir()?,
    };
    ExtensionInfo::load(&dir).map_err(|e| e.to_string())
}

/// 扫描并生成 GUI 行（非 Windows 返回空列表）。
#[cfg(windows)]
pub fn scan_rows() -> Vec<BrowserRow> {
    scan_rows_impl()
}

#[cfg(not(windows))]
pub fn scan_rows() -> Vec<BrowserRow> {
    Vec::new()
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
    match swoop_browser_bridge::guide(browser, ext) {
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
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};

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
    let args = wide(&format!("browsers install --id {id} --silent"));
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
pub fn guide_by_id(id: &str) -> Result<String, String> {
    let defs = swoop_browser_bridge::browsers();
    let detected = swoop_browser_bridge::detect_installed(&defs);
    let browser = detected.iter().find(|b| b.id() == id).ok_or_else(|| swoop_i18n::trf!("未检测到浏览器：{id}"))?;
    let ext = load_ext(&None)?;
    let outcome = swoop_browser_bridge::guide(browser, &ext).map_err(|e| e.to_string())?;
    Ok(outcome_label(&outcome).to_string())
}

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
        return Err(BridgeError::Unsupported("浏览器扩展集成仅支持 Windows".into()).to_string());
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
                    let outcome = swoop_browser_bridge::install_extension(browser, &ext);
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
                let outcome = swoop_browser_bridge::guide(browser, &ext).map_err(|e| e.to_string())?;
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "data": {"id": browser.id(), "outcome": outcome_label(&outcome)}})
                );
                Ok(0)
            }
        }
    }
}
