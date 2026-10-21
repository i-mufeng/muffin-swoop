//! 识别系统中已安装的浏览器，并按策略表把 Swoop 扩展装进去。
//!
//! 平台现实约束见 browsers.toml 头部注释。核心入口：
//! - [`browsers`]：内嵌的策略表（`browsers.toml`）。
//! - [`detect_installed`]：扫描注册表 / 协议 / 已知路径，返回识别到的浏览器。
//! - [`install_extension`] / [`uninstall_extension`] / [`guide`]：按策略执行。
//! - [`load_extension`]：读取随安装包携带的 crx / xpi / 信息文件。

use std::path::{Path, PathBuf};

use serde::Deserialize;

mod config;
mod install;
mod sys;

pub use config::{BrowserDef, InstallStrategy};
pub use install::{CRX_FILE, ExtensionInfo, ExtensionStatus, INFO_FILE, XPI_FILE};

pub(crate) use install::load_extension;

/// 识别到的浏览器实例。
#[derive(Debug, Clone)]
pub struct DetectedBrowser {
    pub def: BrowserDef,
    /// 卸载登记里读到的版本号（不一定有）。
    pub version: Option<String>,
    /// 探测到的浏览器可执行文件。
    pub exe: Option<PathBuf>,
}

impl DetectedBrowser {
    pub fn id(&self) -> &str {
        &self.def.id
    }
    pub fn name(&self) -> &str {
        &self.def.name
    }
}

/// 安装动作的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// 静默登记完成（注册表 / policies.json），重启浏览器后生效。
    Registered,
    /// 需要用户配合（已打开商店页或扩展页 + 定位到安装包）。
    Guided,
    /// 该浏览器跳过（引导类策略在静默模式下，或策略不适用）。
    Skipped(String),
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("未找到扩展安装包：{0}")]
    MissingExtension(String),
    #[error("扩展信息文件无效：{0}")]
    BadInfo(#[from] serde_json::Error),
    #[error("读写失败：{0}")]
    Io(#[from] std::io::Error),
    /// 写浏览器策略被拒绝：HKCU\Software\Policies 默认只允许管理员写入，需要 UAC 提权。
    #[error("写入浏览器策略被拒绝（需要管理员权限）：{0}")]
    AccessDenied(String),
    #[error("当前平台不支持：{0}")]
    Unsupported(String),
    #[error("{0}")]
    Other(String),
}

impl ExtensionInfo {
    /// 从目录加载随包携带的扩展（swoop-extension.crx[+.xpi] + swoop-extension-info.json）。
    pub fn load(dir: &Path) -> Result<Self, BridgeError> {
        load_extension(dir)
    }
}

/// 内嵌的浏览器策略表。
pub fn browsers() -> Vec<BrowserDef> {
    config::load()
}

/// 扫描系统中已安装的浏览器（来自策略表的并集探测）。
pub fn detect_installed(defs: &[BrowserDef]) -> Vec<DetectedBrowser> {
    sys::detect_installed(defs)
}

/// 安装扩展到指定浏览器。
pub fn install_extension(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    sys::install_extension(browser, ext)
}

/// 移除之前登记的扩展（注册表项 / policies.json 条目）。
pub fn uninstall_extension(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<(), BridgeError> {
    sys::uninstall_extension(browser, ext)
}

/// 打开引导式安装（扩展页 + 定位安装包，或商店页）。
pub fn guide(browser: &DetectedBrowser, ext: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    sys::guide(browser, ext)
}

/// 查询扩展登记状态。
pub fn status(browser: &DetectedBrowser, ext: &ExtensionInfo) -> ExtensionStatus {
    sys::status(browser, ext)
}

/// 扩展包信息文件（由 extension/scripts/package-crx.mjs 生成）。
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionInfoFile {
    pub chromium_id: String,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_config_parses_with_unique_ids() {
        let defs = browsers();
        assert!(defs.len() >= 5, "策略表至少覆盖 Edge/Chrome/360x2/QQ/Firefox");
        let mut ids = std::collections::HashSet::new();
        for def in &defs {
            assert!(ids.insert(def.id.clone()), "重复的浏览器 id：{}", def.id);
            assert!(!def.name.is_empty());
        }
    }

    #[test]
    fn edge_uses_store_force_list_with_full_config() {
        let defs = browsers();
        let edge = defs.iter().find(|d| d.id == "edge").expect("策略表必须包含 Edge");
        assert_eq!(edge.install, InstallStrategy::StoreForceList);
        let store_url = edge.store_url.as_deref().expect("Edge 必须配置商店地址");
        let root = edge.policy_root.as_deref().expect("Edge 必须配置策略键");
        assert!(root.contains("Policies"), "策略键必须位于 Policies 下：{root}");
        assert!(edge.update_url.as_deref().is_some_and(|u| u.starts_with("https://")));
        // 商店地址最后一段必须能解析出 32 位扩展 ID
        let last = store_url.trim_end_matches('/').rsplit('/').next().unwrap();
        assert_eq!(last.len(), 32);
        assert!(last.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn env_expansion_is_pure_and_case_insensitive() {
        let vars = [("programfiles".to_string(), "C:\\PF".to_string())].into_iter().collect();
        assert_eq!(config::expand_with(&vars, "%ProgramFiles%\\X\\y.exe"), "C:\\PF\\X\\y.exe");
        assert_eq!(config::expand_with(&vars, "%UNKNOWN%\\x"), "%UNKNOWN%\\x");
        assert_eq!(config::expand_with(&vars, "plain"), "plain");
    }

    #[test]
    fn firefox_policy_merge_adds_and_dedupes() {
        let merged = install::merge_firefox_policy(None, "C:\\x\\swoop-extension.xpi");
        let list = install::firefox_install_list(&merged).expect("有新条目");
        assert_eq!(list, vec!["C:\\x\\swoop-extension.xpi"]);

        let again = install::merge_firefox_policy(Some(merged.clone()), "C:\\x\\swoop-extension.xpi");
        assert_eq!(install::firefox_install_list(&again).unwrap(), list);

        let other = install::merge_firefox_policy(Some(merged), "D:\\other.xpi");
        let list = install::firefox_install_list(&other).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"D:\\other.xpi".to_string()));
    }

    #[test]
    fn firefox_policy_remove_cleans_empty_containers() {
        let merged = install::merge_firefox_policy(None, "C:\\x\\swoop-extension.xpi");
        let cleaned = install::remove_firefox_policy(merged, "C:\\x\\swoop-extension.xpi");
        assert!(install::firefox_install_list(&cleaned).is_none());
    }

    #[test]
    fn load_extension_reads_info_and_finds_crx() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("swoop-extension-info.json"),
            r#"{"chromium_id":"eljhiocjhkdacpekjnhloomccicdedng","version":"0.7.0"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("swoop-extension.crx"), b"Cr24").unwrap();
        let info = ExtensionInfo::load(dir.path()).unwrap();
        assert_eq!(info.chromium_id, "eljhiocjhkdacpekjnhloomccicdedng");
        assert_eq!(info.version, "0.7.0");
        assert!(info.xpi.is_none());
    }

    #[test]
    fn load_extension_fails_without_crx() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("swoop-extension-info.json"), r#"{"chromium_id":"x","version":"0.7.0"}"#)
            .unwrap();
        assert!(ExtensionInfo::load(dir.path()).is_err());
    }
}
