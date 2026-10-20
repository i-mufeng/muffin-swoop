//! browsers.toml 解析与 %ENV% 路径展开。

#[cfg(any(windows, test))]
use std::collections::HashMap;

use serde::Deserialize;

const BROWSERS_TOML: &str = include_str!("../browsers.toml");

/// 浏览器的安装策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStrategy {
    /// 写 Chromium 外部扩展注册表（HKCU + 尝试 HKLM），重启浏览器生效。可能无效，依赖浏览器内核行为。
    ChromiumExternalExt,
    /// 写 Firefox 的 distribution/policies.json（Extensions.Install），官方支持的静默安装。
    FirefoxPolicy,
    /// 打开浏览器扩展页 + 资源管理器定位 crx，引导用户拖拽安装。
    GuidedDrag,
    /// 打开官方商店页（Edge）。
    StoreLink,
    /// 写 Chromium 系企业策略 ExtensionInstallForcelist（HKCU，无需管理员），
    /// 重启浏览器后从官方商店强制静默安装。需要 policy_root + update_url。
    StoreForceList,
}

/// 一个浏览器的识别与安装策略定义。
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uninstall_names: Vec<String>,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub exe_candidates: Vec<String>,
    #[serde(default)]
    pub extensions_page: Option<String>,
    pub install: InstallStrategy,
    #[serde(default)]
    pub fallback: Option<InstallStrategy>,
    #[serde(default)]
    pub store_url: Option<String>,
    /// ExtensionInstallForcelist 的策略键根，如 `Software\Policies\Microsoft\Edge`。
    #[serde(default)]
    pub policy_root: Option<String>,
    /// forcelist 条目里的扩展更新地址（商店 crx 端点）。
    #[serde(default)]
    pub update_url: Option<String>,
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    browser: Vec<BrowserDef>,
}

pub(crate) fn load() -> Vec<BrowserDef> {
    let config: Config = toml::from_str(BROWSERS_TOML).expect("browsers.toml 必须可解析");
    config.browser
}

/// 用真实环境变量展开 %VAR% 片段（Windows 环境变量不区分大小写）。
#[cfg(windows)]
pub(crate) fn expand(text: &str) -> String {
    let vars: HashMap<String, String> = std::env::vars().collect();
    expand_with(&vars, text)
}

/// 可注入变量的展开逻辑（测试用）。
#[cfg(any(windows, test))]
pub(crate) fn expand_with(vars: &HashMap<String, String>, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                let value = vars.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
                match value {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
