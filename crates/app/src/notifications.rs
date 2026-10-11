//! 系统通知：策略与发送分离，系统调用在单独线程上执行。

use std::sync::mpsc::{self, SyncSender};

use swoop_core::Event;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationTiming {
    #[default]
    Background,
    Always,
}

impl NotificationTiming {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Always => "always",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "background" => Some(Self::Background),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationSettings {
    pub on_complete: bool,
    pub on_failure: bool,
    pub timing: NotificationTiming,
    pub sound: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { on_complete: true, on_failure: true, timing: NotificationTiming::Background, sound: false }
    }
}

impl NotificationSettings {
    /// 只消费本次运行的新事件，历史任务、删除事件和暂停不触发通知。
    pub fn message(&self, event: &Event, name: Option<&str>, background: bool) -> Option<DesktopNotification> {
        if self.timing == NotificationTiming::Background && !background {
            return None;
        }
        let summary = match event {
            Event::Completed(_) if self.on_complete => swoop_i18n::tr!("Swoop · 下载完成"),
            Event::Failed(_, _) if self.on_failure => swoop_i18n::tr!("Swoop · 下载失败"),
            _ => return None,
        };
        // 不把错误详情或 URL 送到锁屏：错误里可能含 Cookie、签名 URL 等。
        // None 表示任务已被删除；空文件名表示探测前已失败。
        let name = name?;
        let mut chars = name.chars().filter(|c| !c.is_control());
        let mut body: String = chars.by_ref().take(180).collect();
        if chars.next().is_some() {
            body.push('…');
        }
        if body.trim().is_empty() {
            body = swoop_i18n::tr!("未命名任务").into();
        }
        Some(DesktopNotification { summary, body, sound: self.sound })
    }
}

#[derive(Debug)]
pub struct DesktopNotification {
    summary: &'static str,
    body: String,
    sound: bool,
}

pub struct Notifier {
    tx: SyncSender<DesktopNotification>,
}

impl Notifier {
    pub fn new() -> Option<Self> {
        match Self::with_backend(show_native) {
            Ok(notifier) => Some(notifier),
            Err(error) => {
                tracing::warn!(%error, "系统通知线程启动失败");
                None
            }
        }
    }

    fn with_backend(
        mut show: impl FnMut(DesktopNotification) -> Result<(), String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::Builder::new().name("swoop-notifications".into()).spawn(move || {
            while let Ok(message) = rx.recv() {
                if let Err(error) = show(message) {
                    tracing::warn!(%error, "发送系统通知失败");
                }
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, message: DesktopNotification) {
        // 通知服务卡住或大量任务同时结束，都不能卡住主界面或无限占用内存。
        if self.tx.try_send(message).is_err() {
            tracing::warn!("系统通知队列不可用，已跳过提醒");
        }
    }

    pub fn test(&self, sound: bool) {
        self.send(DesktopNotification {
            summary: swoop_i18n::tr!("Swoop · 通知测试"),
            body: swoop_i18n::tr!("下载完成或失败时将在这里提醒").into(),
            sound,
        });
    }
}

fn show_native(message: DesktopNotification) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // macOS 需要已注册的应用标识。打包运行使用自己的标识；cargo run
        // 没有 .app 容器，开发态借用终端标识（系统通知权限也归终端）。
        static APPLICATION: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        APPLICATION
            .get_or_init(|| {
                let id = objc2_foundation::NSBundle::mainBundle()
                    .bundleIdentifier()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "com.apple.Terminal".into());
                notify_rust::set_application(&id).map_err(|error| error.to_string())
            })
            .clone()?;
    }
    // Linux 通知正文允许标记，文件名必须作为普通文本显示。
    #[cfg(all(unix, not(target_os = "macos")))]
    let body = message.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let body = message.body;
    let mut notification = notify_rust::Notification::new();
    notification.appname("Swoop").summary(message.summary).body(&body);
    if message.sound {
        #[cfg(target_os = "macos")]
        notification.sound_name("Glass");
        #[cfg(target_os = "windows")]
        notification.sound_name("Default");
        #[cfg(all(unix, not(target_os = "macos")))]
        notification.sound_name("complete");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if !message.sound {
        notification.hint(notify_rust::Hint::SuppressSound(true));
    }
    notification.show().map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoop_core::{Event, TaskId};

    #[test]
    fn foreground_background_and_event_switches() {
        let complete = Event::Completed(TaskId(1));
        let failed = Event::Failed(TaskId(1), "https://private.example/?token=secret".into());
        let mut settings = NotificationSettings::default();
        assert!(settings.message(&complete, Some("文件.zip"), false).is_none());
        assert!(settings.message(&complete, Some("文件.zip"), true).is_some());
        assert!(settings.message(&failed, Some("文件.zip"), true).is_some());
        settings.sound = true;
        assert!(settings.message(&complete, Some("文件.zip"), true).unwrap().sound);
        settings.on_complete = false;
        assert!(settings.message(&complete, Some("文件.zip"), true).is_none());
        assert!(settings.message(&failed, Some("文件.zip"), true).is_some());
        settings.on_failure = false;
        assert!(settings.message(&failed, Some("文件.zip"), true).is_none());
        settings.on_complete = true;
        settings.timing = NotificationTiming::Always;
        assert!(settings.message(&complete, Some("文件.zip"), false).is_some());
        assert!(settings.message(&complete, None, true).is_none());
        assert!(settings.message(&Event::Changed(TaskId(1)), Some("a"), true).is_none());
        assert!(settings.message(&Event::Removed(TaskId(1)), Some("a"), true).is_none());
    }

    #[test]
    fn failure_notification_does_not_expose_request_errors() {
        let event = Event::Failed(TaskId(1), "https://private.example/?token=secret".into());
        let message = NotificationSettings::default().message(&event, Some("文件.zip"), true).unwrap();
        assert_eq!(message.summary, "Swoop · 下载失败");
        assert_eq!(message.body, "文件.zip");
    }

    #[test]
    fn names_are_bounded_and_control_characters_removed() {
        let settings = NotificationSettings::default();
        let event = Event::Completed(TaskId(1));
        assert_eq!(settings.message(&event, Some("\n\t"), true).unwrap().body, "未命名任务");
        let name = format!("\n{}\r", "文".repeat(500));
        let body = settings.message(&event, Some(&name), true).unwrap().body;
        assert!(body.chars().count() <= 181);
        assert!(!body.contains(['\n', '\r']));
        assert!(body.ends_with('…'));
    }

    #[test]
    fn a_backend_failure_does_not_stop_later_notifications() {
        let (sent, received) = std::sync::mpsc::channel();
        let notifier = Notifier::with_backend(move |message| {
            sent.send(message.body.clone()).unwrap();
            Err("模拟系统拒绝通知".into())
        })
        .unwrap();
        for name in ["one.zip", "two.zip"] {
            notifier.send(DesktopNotification { summary: "Swoop · 下载完成", body: name.into(), sound: false });
        }
        for name in ["one.zip", "two.zip"] {
            assert_eq!(received.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), name);
        }
    }

    #[test]
    fn a_busy_backend_does_not_block_the_caller() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (started, ready) = std::sync::mpsc::channel();
        let notifier = Notifier::with_backend(move |_| {
            let _ = started.send(());
            let _ = wait.recv();
            Ok(())
        })
        .unwrap();
        notifier.send(DesktopNotification { summary: "test", body: "one".into(), sound: false });
        ready.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        // 后端仍在等待：填满队列后也必须立即返回。
        for _ in 0..100 {
            notifier.send(DesktopNotification { summary: "test", body: "queued".into(), sound: false });
        }
        drop(release);
    }

    #[test]
    #[ignore = "会实际发送系统通知；仅用于本机人工验收"]
    fn native_notification_smoke() {
        show_native(DesktopNotification {
            summary: "Swoop · 通知测试", body: "系统通知测试".into(), sound: false
        })
        .unwrap();
    }
}
