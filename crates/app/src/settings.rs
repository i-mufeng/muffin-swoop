//! 应用设置：存在 SQLite 的 settings 键值表里，启动时读、变更时写。

use std::path::PathBuf;

use swoop_core::Store;

use crate::bridge::{BridgeMode, DEFAULT_PORT};
use crate::theme::Mode;
use crate::ui::progress::ProgressStyle;

#[derive(Clone, Debug)]
pub struct AppSettings {
    pub theme: Mode,
    pub default_dir: PathBuf,
    pub progress_style: ProgressStyle,
    /// 监听剪贴板里的新链接并弹吐司
    pub clipboard_watch: bool,
    /// 启动时自动继续上次未完成的任务
    pub auto_resume: bool,
    pub speed_limit_on: bool,
    pub speed_limit_mbps: f64,
    /// 浏览器集成：本地 HTTP 服务开关、端口、接收方式
    pub bridge_enabled: bool,
    pub bridge_port: u16,
    pub bridge_mode: BridgeMode,
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

impl AppSettings {
    pub fn load(store: &Store, fallback_dir: PathBuf) -> Self {
        let get = |k: &str| store.get_setting(k).ok().flatten();
        let flag = |k: &str, default: bool| get(k).map(|v| v == "1").unwrap_or(default);
        Self {
            theme: get("theme").and_then(|v| Mode::parse(&v)).unwrap_or(Mode::Dark),
            default_dir: get("default_dir").map(PathBuf::from).unwrap_or(fallback_dir),
            progress_style: get("progress_style").and_then(|v| ProgressStyle::parse(&v)).unwrap_or_default(),
            clipboard_watch: flag("clipboard_watch", true),
            auto_resume: flag("auto_resume", true),
            speed_limit_on: flag("speed_limit_on", false),
            speed_limit_mbps: get("speed_limit_mbps").and_then(|v| v.parse().ok()).unwrap_or(5.0),
            bridge_enabled: flag("bridge_enabled", true),
            bridge_port: get("bridge_port").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT),
            bridge_mode: get("bridge_mode").and_then(|v| BridgeMode::parse(&v)).unwrap_or_default(),
        }
    }

    pub fn save(&self, store: &Store) {
        let pairs: [(&str, String); 10] = [
            ("theme", self.theme.as_str().into()),
            ("default_dir", self.default_dir.display().to_string()),
            ("progress_style", self.progress_style.as_str().into()),
            ("clipboard_watch", flag_str(self.clipboard_watch)),
            ("auto_resume", flag_str(self.auto_resume)),
            ("speed_limit_on", flag_str(self.speed_limit_on)),
            ("speed_limit_mbps", format!("{:.2}", self.speed_limit_mbps)),
            ("bridge_enabled", flag_str(self.bridge_enabled)),
            ("bridge_port", self.bridge_port.to_string()),
            ("bridge_mode", self.bridge_mode.as_str().into()),
        ];
        for (k, v) in pairs {
            if let Err(e) = store.set_setting(k, &v) {
                tracing::warn!(key = k, "保存设置失败: {e}");
            }
        }
    }
}

fn flag_str(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}
