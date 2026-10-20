//! 扩展包信息加载与跨平台共享的安装逻辑（Firefox policies.json 合并是纯函数，可测试）。

use std::path::{Path, PathBuf};

use crate::{BridgeError, ExtensionInfoFile};

/// 随安装包携带的 Swoop 扩展。
#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    pub chromium_id: String,
    pub version: String,
    pub crx: PathBuf,
    pub xpi: Option<PathBuf>,
}

/// 扩展在某浏览器里的登记状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionStatus {
    /// 已静默登记（注册表 / policies.json）。
    Installed,
    NotInstalled,
    /// 引导类策略无法自动判断。
    Unknown,
}

const INFO_FILE: &str = "swoop-extension-info.json";
const CRX_FILE: &str = "swoop-extension.crx";
const XPI_FILE: &str = "swoop-extension.xpi";

pub(crate) fn load_extension(dir: &Path) -> Result<ExtensionInfo, BridgeError> {
    let info_path = dir.join(INFO_FILE);
    let file: ExtensionInfoFile = serde_json::from_str(
        &std::fs::read_to_string(&info_path)
            .map_err(|_| BridgeError::MissingExtension(format!("{} 不存在（目录 {}）", INFO_FILE, dir.display())))?,
    )?;
    let crx = dir.join(CRX_FILE);
    if !crx.is_file() {
        return Err(BridgeError::MissingExtension(format!("{} 不存在（目录 {}）", CRX_FILE, dir.display())));
    }
    let xpi = dir.join(XPI_FILE);
    Ok(ExtensionInfo { chromium_id: file.chromium_id, version: file.version, crx, xpi: xpi.is_file().then_some(xpi) })
}

// ---- Firefox distribution/policies.json 合并（纯函数） ----

/// 把 xpi 路径合并进已有的 policies.json 内容（None 表示文件不存在）。
#[cfg(any(windows, test))]
pub(crate) fn merge_firefox_policy(existing: Option<serde_json::Value>, xpi: &str) -> serde_json::Value {
    let mut root = existing.unwrap_or_else(|| serde_json::json!({}));
    let root = root.as_object_mut().expect("policies.json 根必须是对象");
    let policies =
        root.entry("policies").or_insert_with(|| serde_json::json!({})).as_object_mut().expect("policies 必须是对象");
    let extensions = policies
        .entry("Extensions")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("Extensions 必须是对象");
    let list = extensions
        .entry("Install")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("Install 必须是数组");
    if !list.iter().any(|v| v.as_str() == Some(xpi)) {
        list.push(serde_json::Value::String(xpi.to_string()));
    }
    root.clone().into()
}

/// 从 policies 内容里移除指定 xpi 路径；返回清理后的内容（可能已无 Swoop 痕迹）。
#[cfg(any(windows, test))]
pub(crate) fn remove_firefox_policy(mut root: serde_json::Value, xpi: &str) -> serde_json::Value {
    let Some(policies) = root.get_mut("policies").and_then(|v| v.as_object_mut()) else {
        return root;
    };
    let Some(extensions) = policies.get_mut("Extensions").and_then(|v| v.as_object_mut()) else {
        return root;
    };
    let Some(list) = extensions.get_mut("Install").and_then(|v| v.as_array_mut()) else {
        return root;
    };
    list.retain(|v| v.as_str() != Some(xpi));
    if list.is_empty() {
        extensions.remove("Install");
    }
    if extensions.is_empty() {
        policies.remove("Extensions");
    }
    root
}

/// 读取当前 Install 列表（仅当结构完整时）。
#[cfg(any(windows, test))]
pub(crate) fn firefox_install_list(root: &serde_json::Value) -> Option<Vec<String>> {
    let list = root.get("policies")?.get("Extensions")?.get("Install")?.as_array()?;
    Some(list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}
