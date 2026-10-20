//! Windows 实现：注册表识别 + 按策略安装扩展。

use std::path::{Path, PathBuf};
use std::process::Command;

use winreg::RegKey;
use winreg::enums::*;

use crate::config::{InstallStrategy, expand};
use crate::install::{self, ExtensionInfo, ExtensionStatus};
use crate::{BridgeError, BrowserDef, DetectedBrowser, InstallOutcome};

// ---- 识别 ----

/// 从卸载登记、协议注册和路径探测三路取并集。
pub(crate) fn detect_installed(defs: &[BrowserDef]) -> Vec<DetectedBrowser> {
    let uninstall = collect_uninstall_entries();
    defs.iter()
        .filter_map(|def| {
            let mut version = None;
            for (name, ver) in &uninstall {
                if def.uninstall_names.iter().any(|p| name.contains(p.as_str())) {
                    version = ver.clone();
                    break;
                }
            }
            let protocol_hit = def.protocols.iter().any(|p| protocol_registered(p));
            let exe = def.exe_candidates.iter().map(|c| PathBuf::from(expand(c))).find(|p| p.is_file());
            if version.is_some() || protocol_hit || exe.is_some() {
                Some(DetectedBrowser { def: def.clone(), version, exe })
            } else {
                None
            }
        })
        .collect()
}

/// 读取 HKLM（64/32 视图）与 HKCU 的卸载登记：(DisplayName, DisplayVersion)。
fn collect_uninstall_entries() -> Vec<(String, Option<String>)> {
    let roots = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_READ),
    ];
    let mut out = Vec::new();
    for (root, flags) in roots {
        let Ok(key) =
            RegKey::predef(root).open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Uninstall", flags)
        else {
            continue;
        };
        for name in key.enum_keys().map_while(Result::ok) {
            let Ok(sub) = key.open_subkey(&name) else { continue };
            let Ok(display) = sub.get_value::<String, _>("DisplayName") else { continue };
            let version = sub.get_value::<String, _>("DisplayVersion").ok();
            out.push((display, version));
        }
    }
    out
}

fn protocol_registered(protocol: &str) -> bool {
    RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey_with_flags(format!("{protocol}\\shell\\open\\command"), KEY_READ)
        .is_ok()
}

// ---- Chromium 外部扩展注册表 ----

/// Chromium 系读取的外部扩展键（Google 品牌 + Chromium 无品牌内核各写一份）。
fn external_ext_roots() -> &'static [&'static str] {
    &[r"Software\Google\Chrome\Extensions", r"Software\Chromium\Extensions"]
}

fn external_ext_key(id: &str) -> String {
    // 调用方拼根；这里只保证 id 合法（32 个小写字母）。
    id.to_string()
}

fn write_external_ext(ext: &ExtensionInfo, hive: &str, root: &str) -> std::io::Result<()> {
    let (predef, path) = match hive {
        "HKCU" => (HKEY_CURRENT_USER, root),
        _ => (HKEY_LOCAL_MACHINE, root),
    };
    let (key, _) = RegKey::predef(predef).create_subkey(format!("{path}\\{}", external_ext_key(&ext.chromium_id)))?;
    key.set_value("path", &ext.crx.to_string_lossy().to_string())?;
    key.set_value("version", &ext.version)?;
    Ok(())
}

fn delete_external_ext(id: &str, hive: &str, root: &str) {
    let predef = if hive == "HKCU" { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
    let _ = RegKey::predef(predef).delete_subkey_all(format!("{root}\\{id}"));
}

fn external_ext_registered(ext: &ExtensionInfo, root: &str) -> bool {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!("{root}\\{}", ext.chromium_id)).is_ok()
}

// ---- Chromium ExtensionInstallForcelist 策略 ----

/// 策略键根下的 forcelist 子键。
fn forcelist_key(root: &str) -> String {
    format!("{root}\\ExtensionInstallForcelist")
}

/// 从商店页 URL 提取扩展 id（路径最后一段，32 位字母数字）。
fn store_id(url: &str) -> Option<String> {
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    if last.len() == 32 && last.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(last.to_string())
    } else {
        None
    }
}

/// 查询四个视图：HKCU/HKLM × 64/32 位。
fn forcelist_registered(root: &str, id: &str) -> bool {
    let key_path = forcelist_key(root);
    let prefix = format!("{id};");
    [
        (HKEY_CURRENT_USER, KEY_READ),
        (HKEY_CURRENT_USER, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
    ]
    .iter()
    .any(|(predef, flags)| {
        RegKey::predef(*predef)
            .open_subkey_with_flags(&key_path, *flags)
            .map(|key| {
                key.enum_values()
                    .map_while(Result::ok)
                    .any(|(name, _)| key.get_value::<String, _>(name).is_ok_and(|v| v.starts_with(&prefix)))
            })
            .unwrap_or(false)
    })
}

/// HKCU 写入保证成功（无需管理员）；HKLM 需要管理员，失败忽略（按用户安装场景）。
fn write_forcelist(root: &str, id: &str, update_url: &str) -> std::io::Result<()> {
    let key_path = forcelist_key(root);
    let entry = format!("{id};{update_url}");
    let mut last_err = None;
    for (predef, flags) in
        [(HKEY_CURRENT_USER, KEY_SET_VALUE), (HKEY_CURRENT_USER, KEY_SET_VALUE | KEY_WOW64_32KEY)]
    {
        match RegKey::predef(predef).create_subkey_with_flags(&key_path, flags) {
            Ok((key, _)) => {
                if forcelist_key_has(&key, id) {
                    continue;
                }
                // 值名取第一个空闲的数字名（"1".."99"），与常见企业管理工具习惯一致。
                let name = (1..100).map(|n| n.to_string()).find(|n| key.get_value::<String, _>(n).is_err());
                match name {
                    Some(name) => {
                        if let Err(e) = key.set_value(&name, &entry) {
                            last_err = Some(e);
                        }
                    }
                    None => last_err = Some(std::io::Error::other("forcelist 条目已满")),
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    for flags in [KEY_SET_VALUE | KEY_WOW64_64KEY, KEY_SET_VALUE | KEY_WOW64_32KEY] {
        if let Ok((key, _)) = RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey_with_flags(&key_path, flags)
            && !forcelist_key_has(&key, id)
            && let Some(name) = (1..100).map(|n| n.to_string()).find(|n| key.get_value::<String, _>(n).is_err())
        {
            let _ = key.set_value(&name, &entry);
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn forcelist_key_has(key: &RegKey, id: &str) -> bool {
    let prefix = format!("{id};");
    key.enum_values()
        .map_while(Result::ok)
        .any(|(name, _)| key.get_value::<String, _>(name).is_ok_and(|v| v.starts_with(&prefix)))
}

fn delete_forcelist(root: &str, id: &str) {
    let key_path = forcelist_key(root);
    let prefix = format!("{id};");
    for (predef, flags) in [
        (HKEY_CURRENT_USER, KEY_READ | KEY_SET_VALUE),
        (HKEY_CURRENT_USER, KEY_READ | KEY_SET_VALUE | KEY_WOW64_32KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_SET_VALUE | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_SET_VALUE | KEY_WOW64_32KEY),
    ] {
        let Ok(key) = RegKey::predef(predef).open_subkey_with_flags(&key_path, flags) else { continue };
        let names: Vec<String> = key
            .enum_values()
            .map_while(Result::ok)
            .filter(|(name, _)| key.get_value::<String, _>(name).is_ok_and(|v| v.starts_with(&prefix)))
            .map(|(name, _)| name)
            .collect();
        for name in names {
            let _ = key.delete_value(&name);
        }
    }
}

// ---- Firefox policies.json ----

fn firefox_policies_path(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or(exe).join("distribution").join("policies.json")
}

fn write_firefox_policy(exe: &Path, xpi: &Path) -> std::io::Result<()> {
    let path = firefox_policies_path(exe);
    let existing = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok());
    let merged = install::merge_firefox_policy(existing, &xpi.to_string_lossy());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&merged)?)
}

fn remove_firefox_policy(exe: &Path, xpi: &Path) -> std::io::Result<()> {
    let path = firefox_policies_path(exe);
    let Some(existing) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()) else {
        return Ok(());
    };
    let cleaned = install::remove_firefox_policy(existing, &xpi.to_string_lossy());
    std::fs::write(&path, serde_json::to_string_pretty(&cleaned)?)
}

fn firefox_policy_registered(exe: &Path, xpi: &Path) -> bool {
    let path = firefox_policies_path(exe);
    let Some(existing) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()) else {
        return false;
    };
    install::firefox_install_list(&existing).is_some_and(|l| l.iter().any(|e| e == &xpi.to_string_lossy().to_string()))
}

// ---- 打开外部程序 ----

/// `cmd /c start "" <target>`：target 可为 URL 或路径。
fn open_in_default(target: &str) -> std::io::Result<()> {
    Command::new("cmd").args(["/c", "start", "", target]).spawn().map(|_| ())
}

/// 资源管理器窗口定位并选中文件。
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn().map(|_| ())
}

// ---- 安装 / 卸载 / 引导 / 状态 ----

pub(crate) fn install_extension(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    run_strategy(browser, ext, browser.def.install, false)
}

pub(crate) fn guide(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    // 引导安装直接用 fallback（或主策略若是引导类/商店类则用之）。
    // store-force-list 的引导降级是打开商店页（策略被组织接管时兜底）。
    let strategy = match browser.def.install {
        InstallStrategy::GuidedDrag | InstallStrategy::StoreLink => browser.def.install,
        InstallStrategy::StoreForceList => InstallStrategy::StoreLink,
        _ => browser.def.fallback.unwrap_or(InstallStrategy::GuidedDrag),
    };
    run_strategy(browser, ext, strategy, true)
}

fn run_strategy(
    browser: &DetectedBrowser,
    ext: &ExtensionInfo,
    strategy: InstallStrategy,
    guided: bool,
) -> Result<InstallOutcome, BridgeError> {
    match strategy {
        InstallStrategy::ChromiumExternalExt => {
            let mut last_err = None;
            for root in external_ext_roots() {
                if let Err(e) = write_external_ext(ext, "HKCU", root) {
                    last_err = Some(e);
                }
                // HKLM 需要管理员权限；per-user 安装失败属预期，忽略错误。
                let _ = write_external_ext(ext, "HKLM", root);
            }
            match last_err {
                Some(e) => Err(e.into()),
                None => Ok(InstallOutcome::Registered),
            }
        }
        InstallStrategy::FirefoxPolicy => {
            let Some(exe) = &browser.exe else {
                return Err(BridgeError::Other(format!("找不到 {} 的安装目录", browser.name())));
            };
            let Some(xpi) = &ext.xpi else {
                return Err(BridgeError::MissingExtension("Firefox 需要 swoop-extension.xpi".into()));
            };
            write_firefox_policy(exe, xpi).map_err(|e| {
                // Firefox 装在 Program Files 时，distribution\policies.json 需要管理员权限
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    BridgeError::AccessDenied(e.to_string())
                } else {
                    BridgeError::Io(e)
                }
            })?;
            Ok(InstallOutcome::Registered)
        }
        InstallStrategy::GuidedDrag => {
            let page = browser.def.extensions_page.as_deref().unwrap_or("chrome://extensions/");
            // 优先用浏览器自己的 exe 打开扩展页：about:addons / se:// 这类页面用默认浏览器打开可能是错的
            match &browser.exe {
                Some(exe) => {
                    Command::new(exe).arg(page).spawn().map(|_| ())?;
                }
                None => open_in_default(page)?,
            }
            // Firefox 装 xpi，Chromium 系装 crx
            let package =
                if browser.def.install == InstallStrategy::FirefoxPolicy { ext.xpi.as_ref().unwrap_or(&ext.crx) } else { &ext.crx };
            reveal_in_explorer(package)?;
            Ok(InstallOutcome::Guided)
        }
        InstallStrategy::StoreLink => {
            let Some(url) = browser.def.store_url.as_deref() else {
                return Err(BridgeError::Other(format!("{} 未配置商店地址", browser.name())));
            };
            open_in_default(url)?;
            Ok(if guided { InstallOutcome::Guided } else { InstallOutcome::Skipped("store".into()) })
        }
        InstallStrategy::StoreForceList => {
            let missing = |what: &str| BridgeError::Other(format!("{} 未配置{what}", browser.name()));
            let root = browser.def.policy_root.as_deref().ok_or_else(|| missing("策略键"))?;
            let update_url = browser.def.update_url.as_deref().ok_or_else(|| missing("扩展更新地址"))?;
            let id = store_id(browser.def.store_url.as_deref().ok_or_else(|| missing("商店地址"))?)
                .ok_or_else(|| missing("商店扩展 ID"))?;
            write_forcelist(root, &id, update_url).map_err(|e| {
                // HKCU\Software\Policies 默认 ACL 只给普通用户读取权限，未提权写入会得到“拒绝访问”
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    BridgeError::AccessDenied(e.to_string())
                } else {
                    BridgeError::Io(e)
                }
            })?;
            Ok(InstallOutcome::Registered)
        }
    }
}

pub(crate) fn uninstall_extension(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<(), BridgeError> {
    match browser.def.install {
        InstallStrategy::ChromiumExternalExt => {
            for root in external_ext_roots() {
                delete_external_ext(&ext.chromium_id, "HKCU", root);
                delete_external_ext(&ext.chromium_id, "HKLM", root);
            }
            Ok(())
        }
        InstallStrategy::FirefoxPolicy => {
            if let (Some(exe), Some(xpi)) = (&browser.exe, &ext.xpi) {
                remove_firefox_policy(exe, xpi)?;
            }
            Ok(())
        }
        InstallStrategy::StoreForceList => {
            if let (Some(root), Some(id)) = (&browser.def.policy_root, browser.def.store_url.as_deref().and_then(store_id))
            {
                delete_forcelist(root, &id);
            }
            Ok(())
        }
        InstallStrategy::GuidedDrag | InstallStrategy::StoreLink => Ok(()),
    }
}

pub(crate) fn status(browser: &DetectedBrowser, ext: &ExtensionInfo) -> ExtensionStatus {
    match browser.def.install {
        InstallStrategy::ChromiumExternalExt => {
            if external_ext_roots().iter().any(|root| external_ext_registered(ext, root)) {
                ExtensionStatus::Installed
            } else {
                ExtensionStatus::NotInstalled
            }
        }
        InstallStrategy::FirefoxPolicy => match (&browser.exe, &ext.xpi) {
            (Some(exe), Some(xpi)) if firefox_policy_registered(exe, xpi) => ExtensionStatus::Installed,
            _ => ExtensionStatus::NotInstalled,
        },
        InstallStrategy::StoreForceList => {
            match (&browser.def.policy_root, browser.def.store_url.as_deref().and_then(store_id)) {
                (Some(root), Some(id)) if forcelist_registered(root, &id) => ExtensionStatus::Installed,
                _ => ExtensionStatus::NotInstalled,
            }
        }
        InstallStrategy::GuidedDrag | InstallStrategy::StoreLink => ExtensionStatus::Unknown,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn store_id_picks_last_path_segment() {
        assert_eq!(
            store_id("https://microsoftedge.microsoft.com/addons/detail/swoop/ecoapkmgaijhekboiplkojnpefddffkh"),
            Some("ecoapkmgaijhekboiplkojnpefddffkh".to_string())
        );
        assert_eq!(store_id("https://example.com/addons/detail/swoop/ecoapkmgaijhekboiplkojnpefddffkh/"), store_id("https://example.com/addons/detail/swoop/ecoapkmgaijhekboiplkojnpefddffkh"));
        assert_eq!(store_id("https://example.com/"), None);
        assert_eq!(store_id("https://example.com/short"), None);
    }

    #[test]
    fn forcelist_key_appends_subkey() {
        assert_eq!(
            forcelist_key(r"Software\Policies\Microsoft\Edge"),
            r"Software\Policies\Microsoft\Edge\ExtensionInstallForcelist"
        );
    }
}
