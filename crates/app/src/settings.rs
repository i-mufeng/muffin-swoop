//! 应用设置：存在 SQLite 的 settings 键值表里，启动时读、变更时写。

use std::path::PathBuf;

use swoop_core::Store;

use crate::bridge::{BridgeMode, DEFAULT_PORT};
use crate::notifications::{NotificationSettings, NotificationTiming};
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
    /// 关闭窗口后驻留菜单栏继续下载（无托盘的平台上忽略）
    pub close_to_tray: bool,
    /// 菜单栏图标旁显示总速度（仅 macOS）
    pub tray_speed: bool,
    pub notifications: NotificationSettings,
    /// 新建或继续任务时采用的最大连接数。
    pub connections: usize,
    pub batch_limit: usize,
    /// 单个下载开始时弹出独立的进度窗口
    pub progress_window: bool,
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads"))).unwrap_or_else(|| PathBuf::from("."))
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
            close_to_tray: flag("close_to_tray", true),
            tray_speed: flag("tray_speed", true),
            notifications: NotificationSettings {
                on_complete: flag("notify_complete", true),
                on_failure: flag("notify_failure", true),
                sound: flag("notification_sound", false),
                timing: get("notification_timing").and_then(|v| NotificationTiming::parse(&v)).unwrap_or_default(),
            },
            connections: get("connections")
                .and_then(|v| v.parse().ok())
                .filter(|n| (1..=swoop_core::MAX_CONNECTIONS).contains(n))
                .unwrap_or(swoop_core::Config::default().connections),
            batch_limit: get("batch_limit")
                .and_then(|v| v.parse().ok())
                .filter(|n| (1..=crate::batch::MAX_BATCH_LIMIT).contains(n))
                .unwrap_or(crate::batch::DEFAULT_BATCH_LIMIT),
            progress_window: flag("progress_window", true),
        }
    }

    pub fn save(&self, store: &Store) -> swoop_core::Result<()> {
        let pairs = [
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
            ("close_to_tray", flag_str(self.close_to_tray)),
            ("tray_speed", flag_str(self.tray_speed)),
            ("notify_complete", flag_str(self.notifications.on_complete)),
            ("notify_failure", flag_str(self.notifications.on_failure)),
            ("notification_timing", self.notifications.timing.as_str().into()),
            ("notification_sound", flag_str(self.notifications.sound)),
            ("connections", self.connections.to_string()),
            ("batch_limit", self.batch_limit.to_string()),
            ("progress_window", flag_str(self.progress_window)),
        ];
        store.set_settings(&pairs)
    }
}

fn flag_str(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::{NotificationSettings, NotificationTiming};

    #[test]
    fn old_settings_receive_notification_and_connection_defaults() {
        let store = Store::in_memory().unwrap();
        let settings = AppSettings::load(&store, PathBuf::from("downloads"));
        assert_eq!(settings.notifications, NotificationSettings::default());
        assert_eq!(settings.connections, 8);
    }

    #[test]
    fn notification_preferences_and_connections_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let store = Store::open(&path).unwrap();
        let mut settings = AppSettings::load(&store, dir.path().into());
        settings.notifications.on_complete = false;
        settings.notifications.on_failure = true;
        settings.notifications.timing = NotificationTiming::Always;
        settings.notifications.sound = true;
        settings.connections = 3;
        settings.save(&store).unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let restored = AppSettings::load(&reopened, PathBuf::from("unused"));
        assert_eq!(restored.notifications, settings.notifications);
        assert_eq!(restored.connections, 3);
    }

    #[test]
    fn invalid_connection_counts_and_notification_timing_have_safe_defaults() {
        let store = Store::in_memory().unwrap();
        store.set_setting("notification_timing", "unknown").unwrap();
        for value in ["0", "33", "-1", "broken", "999999999999999999999999"] {
            store.set_setting("connections", value).unwrap();
            let settings = AppSettings::load(&store, PathBuf::from("downloads"));
            assert_eq!(settings.connections, 8);
            assert_eq!(settings.notifications.timing, NotificationTiming::Background);
        }
    }
}
