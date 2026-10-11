//! 系统通知：策略与发送分离，系统调用在单独线程上执行。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};

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
        Some(DesktopNotification { summary, body, sound: self.sound, file: None })
    }
}

#[derive(Debug)]
pub struct DesktopNotification {
    summary: &'static str,
    body: String,
    sound: bool,
    /// 下载完成的文件；有值时通知带「打开 / 打开文件夹」按钮（macOS、Windows）
    file: Option<PathBuf>,
}

impl DesktopNotification {
    pub fn with_file(mut self, path: PathBuf) -> Self {
        self.file = Some(path);
        self
    }
}

/// 用户在系统通知上的操作，回到界面线程执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationAction {
    Open(PathBuf),
    Reveal(PathBuf),
    /// 点了通知正文：只调出主窗口，不运行文件，防止误触
    Show,
}

/// 带按钮的 macOS 通知会占住一个线程直到用户处理或清除它；超过上限的退回普通通知。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAX_WAITING: usize = 4;

#[derive(Clone)]
pub struct Responder {
    tx: Sender<NotificationAction>,
    wake: Arc<dyn Fn() + Send + Sync>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    waiting: Arc<AtomicUsize>,
}

impl Responder {
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    fn send(&self, action: NotificationAction) {
        if self.tx.send(action).is_ok() {
            (self.wake)();
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn acquire(&self) -> Option<WaitSlot> {
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(WaitSlot(self.waiting.clone()))
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct WaitSlot(Arc<AtomicUsize>);

impl Drop for WaitSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Notifier {
    tx: SyncSender<DesktopNotification>,
    actions: Receiver<NotificationAction>,
}

impl Notifier {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        match Self::with_backend(wake, show_native) {
            Ok(notifier) => Some(notifier),
            Err(error) => {
                tracing::warn!(%error, "系统通知线程启动失败");
                None
            }
        }
    }

    fn with_backend(
        wake: impl Fn() + Send + Sync + 'static,
        mut show: impl FnMut(DesktopNotification, &Responder) -> Result<(), String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(32);
        let (action_tx, actions) = mpsc::channel();
        let responder = Responder { tx: action_tx, wake: Arc::new(wake), waiting: Arc::default() };
        std::thread::Builder::new().name("swoop-notifications".into()).spawn(move || {
            while let Ok(message) = rx.recv() {
                if let Err(error) = show(message, &responder) {
                    tracing::warn!(%error, "发送系统通知失败");
                }
            }
        })?;
        Ok(Self { tx, actions })
    }

    pub fn send(&self, message: DesktopNotification) {
        // 通知服务卡住或大量任务同时结束，都不能卡住主界面或无限占用内存。
        if self.tx.try_send(message).is_err() {
            tracing::warn!("系统通知队列不可用，已跳过提醒");
        }
    }

    /// 取出用户在通知上的操作
    pub fn poll(&self) -> Vec<NotificationAction> {
        self.actions.try_iter().collect()
    }

    pub fn test(&self, sound: bool) {
        self.send(DesktopNotification {
            summary: swoop_i18n::tr!("Swoop · 通知测试"),
            body: swoop_i18n::tr!("下载完成或失败时将在这里提醒").into(),
            sound,
            file: None,
        });
    }
}

fn show_native(mut message: DesktopNotification, responder: &Responder) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    ensure_macos_application()?;
    #[cfg(any(target_os = "macos", windows))]
    if let Some(path) = message.file.take() {
        return show_actionable(message, path, responder);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (message.file.take(), responder);
    show_plain(message)
}

#[cfg(target_os = "macos")]
fn ensure_macos_application() -> Result<(), String> {
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
        .clone()
}

#[cfg(target_os = "macos")]
fn show_actionable(message: DesktopNotification, path: PathBuf, responder: &Responder) -> Result<(), String> {
    use mac_notification_sys::{MainButton, Notification, NotificationResponse};
    let Some(slot) = responder.acquire() else {
        return show_plain(message);
    };
    let responder = responder.clone();
    std::thread::Builder::new()
        .name("swoop-notification-wait".into())
        .spawn(move || {
            let _slot = slot;
            let open = swoop_i18n::tr!("打开");
            let reveal = swoop_i18n::tr!("打开文件夹");
            let actions = [open, reveal];
            let body = message.body.clone();
            let mut notification = Notification::new();
            notification
                .title(message.summary)
                .message(&body)
                .main_button(MainButton::DropdownActions(open, &actions))
                .close_button(swoop_i18n::tr!("关闭"));
            if message.sound {
                notification.sound("Glass");
            }
            match notification.send() {
                Ok(NotificationResponse::ActionButton(action)) if action == reveal => {
                    responder.send(NotificationAction::Reveal(path))
                }
                Ok(NotificationResponse::ActionButton(_)) => responder.send(NotificationAction::Open(path)),
                Ok(NotificationResponse::Click) => responder.send(NotificationAction::Show),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "发送系统通知失败"),
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
fn show_actionable(message: DesktopNotification, path: PathBuf, responder: &Responder) -> Result<(), String> {
    use tauri_winrt_notification::{Sound, Toast};
    let responder = responder.clone();
    // 与 notify-rust 一致：未注册 AppUserModelID 时借用 PowerShell 的标识。
    Toast::new(Toast::POWERSHELL_APP_ID)
        .title(message.summary)
        .text1(&message.body)
        .sound(message.sound.then_some(Sound::Default))
        .add_button(swoop_i18n::tr!("打开"), "open")
        .add_button(swoop_i18n::tr!("打开文件夹"), "reveal")
        .on_activated(move |action| {
            responder.send(match action.as_deref() {
                Some("open") => NotificationAction::Open(path.clone()),
                Some("reveal") => NotificationAction::Reveal(path.clone()),
                _ => NotificationAction::Show,
            });
            Ok(())
        })
        .show()
        .map_err(|error| format!("{error:?}"))
}

fn show_plain(message: DesktopNotification) -> Result<(), String> {
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
        let notifier = Notifier::with_backend(
            || {},
            move |message, _| {
                sent.send(message.body.clone()).unwrap();
                Err("模拟系统拒绝通知".into())
            },
        )
        .unwrap();
        for name in ["one.zip", "two.zip"] {
            notifier.send(DesktopNotification {
                summary: "Swoop · 下载完成",
                body: name.into(),
                sound: false,
                file: None,
            });
        }
        for name in ["one.zip", "two.zip"] {
            assert_eq!(received.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), name);
        }
    }

    #[test]
    fn a_busy_backend_does_not_block_the_caller() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (started, ready) = std::sync::mpsc::channel();
        let notifier = Notifier::with_backend(
            || {},
            move |_, _| {
                let _ = started.send(());
                let _ = wait.recv();
                Ok(())
            },
        )
        .unwrap();
        notifier.send(DesktopNotification { summary: "test", body: "one".into(), sound: false, file: None });
        ready.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        // 后端仍在等待：填满队列后也必须立即返回。
        for _ in 0..100 {
            notifier.send(DesktopNotification { summary: "test", body: "queued".into(), sound: false, file: None });
        }
        drop(release);
    }

    #[test]
    #[ignore = "会实际发送系统通知；仅用于本机人工验收"]
    fn native_notification_smoke() {
        let notifier = Notifier::new(|| {}).unwrap();
        notifier.send(
            DesktopNotification {
                summary: "Swoop · 通知测试",
                body: "点按钮后看日志".into(),
                sound: false,
                file: None,
            }
            .with_file(std::env::current_exe().unwrap()),
        );
        std::thread::sleep(std::time::Duration::from_secs(20));
        eprintln!("{:?}", notifier.poll());
    }

    #[test]
    fn user_actions_reach_the_ui_and_wake_it() {
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let notifier = Notifier::with_backend(
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            |message, responder| {
                responder.send(NotificationAction::Reveal(message.file.unwrap()));
                Ok(())
            },
        )
        .unwrap();
        let message = NotificationSettings::default()
            .message(&Event::Completed(TaskId(1)), Some("a.zip"), true)
            .unwrap()
            .with_file(PathBuf::from("/downloads/a.zip"));
        notifier.send(message);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut actions = Vec::new();
        while actions.is_empty() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
            actions = notifier.poll();
        }
        assert_eq!(actions, [NotificationAction::Reveal(PathBuf::from("/downloads/a.zip"))]);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn waiting_notifications_are_bounded() {
        let (tx, _rx) = mpsc::channel();
        let responder = Responder { tx, wake: Arc::new(|| {}), waiting: Arc::default() };
        let slots: Vec<_> = (0..MAX_WAITING).map(|_| responder.acquire().unwrap()).collect();
        assert!(responder.acquire().is_none());
        drop(slots);
        assert!(responder.acquire().is_some());
    }
}
