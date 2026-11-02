//! 系统通知：策略与发送分离，系统调用在单独线程上执行。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

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
const MAX_WAITING: usize = 3;
const MAX_RECENT: usize = 3;

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

#[derive(Default)]
struct PendingNotifications {
    queue: VecDeque<DesktopNotification>,
    closed: bool,
}

pub struct Notifier {
    pending: Arc<(Mutex<PendingNotifications>, Condvar)>,
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
        let pending = Arc::new((Mutex::new(PendingNotifications::default()), Condvar::new()));
        let worker_pending = pending.clone();
        let (action_tx, actions) = mpsc::channel();
        let responder = Responder { tx: action_tx, wake: Arc::new(wake), waiting: Arc::default() };
        std::thread::Builder::new().name("swoop-notifications".into()).spawn(move || {
            loop {
                let (lock, ready) = &*worker_pending;
                let mut pending = lock.lock().unwrap();
                while pending.queue.is_empty() && !pending.closed {
                    pending = ready.wait(pending).unwrap();
                }
                let Some(message) = pending.queue.pop_front() else { break };
                drop(pending);
                if let Err(error) = show(message, &responder) {
                    tracing::warn!(%error, "发送系统通知失败");
                }
            }
        })?;
        Ok(Self { pending, actions })
    }

    pub fn send(&self, message: DesktopNotification) {
        // 后端调用不持锁；大量任务同时结束时只保留最近三条，淘汰旧消息。
        let (lock, ready) = &*self.pending;
        let mut pending = lock.lock().unwrap();
        if pending.queue.len() == MAX_RECENT {
            pending.queue.pop_front();
        }
        pending.queue.push_back(message);
        ready.notify_one();
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

impl Drop for Notifier {
    fn drop(&mut self) {
        let (lock, ready) = &*self.pending;
        lock.lock().unwrap().closed = true;
        ready.notify_one();
    }
}

#[cfg(windows)]
fn show_native(message: DesktopNotification, responder: &Responder) -> Result<(), String> {
    show_windows(message, responder)
}

#[cfg(not(windows))]
fn show_native(mut message: DesktopNotification, responder: &Responder) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    ensure_macos_application()?;
    #[cfg(target_os = "macos")]
    if let Some(path) = message.file.take() {
        return show_actionable(message, path, responder);
    }
    #[cfg(not(target_os = "macos"))]
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

/// 通知来源显示为 Swoop：未注册的桌面程序只能借 PowerShell 的 AppUserModelID 发通知，
/// 系统会把来源显示成 "Windows PowerShell"。在当前用户下登记自己的 ID（名称 + 图标）即可，
/// 不需要开始菜单快捷方式；卸载程序会删掉这个键。
#[cfg(windows)]
const WINDOWS_APP_ID: &str = "com.muffin.swoop";

#[cfg(windows)]
fn windows_app_id() -> &'static str {
    static ID: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    ID.get_or_init(|| match register_windows_app_id() {
        Ok(()) => WINDOWS_APP_ID,
        Err(error) => {
            tracing::warn!(%error, "登记通知标识失败，改用 PowerShell 标识");
            tauri_winrt_notification::Toast::POWERSHELL_APP_ID
        }
    })
}

#[cfg(windows)]
fn register_windows_app_id() -> Result<(), String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;
    let dir = crate::instance::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let icon = dir.join("notification-icon.png");
    if std::fs::read(&icon).ok().as_deref() != Some(crate::brand::app_png()) {
        std::fs::write(&icon, crate::brand::app_png()).map_err(|e| e.to_string())?;
    }
    let wide = |v: &std::ffi::OsStr| -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        v.encode_wide().chain(Some(0)).collect()
    };
    let key = wide(format!(r"Software\Classes\AppUserModelId\{WINDOWS_APP_ID}").as_ref());
    let mut handle = std::ptr::null_mut();
    let open = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    if open != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyExW {open}"));
    }
    let mut result = ERROR_SUCCESS;
    for (name, value) in [("DisplayName", wide("Swoop".as_ref())), ("IconUri", wide(icon.as_os_str()))] {
        let name = wide(name.as_ref());
        let status = unsafe {
            RegSetValueExW(handle, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
        };
        if status != ERROR_SUCCESS {
            result = status;
        }
    }
    unsafe {
        RegCloseKey(handle);
    }
    if result == ERROR_SUCCESS { Ok(()) } else { Err(format!("RegSetValueExW {result}")) }
}

/// 固定三个 tag 槽位：替换旧通知，并隐藏仍在屏幕上的旧横幅。
#[cfg(windows)]
fn show_windows(message: DesktopNotification, responder: &Responder) -> Result<(), String> {
    use std::cell::RefCell;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
    use windows::core::{HSTRING, IInspectable, Interface};

    #[derive(Default)]
    struct Recent {
        next: usize,
        initialized: bool,
        visible: [Option<ToastNotification>; MAX_RECENT],
    }
    thread_local! { static RECENT: RefCell<Recent> = RefCell::default(); }
    let result = RECENT.with(|recent| -> windows::core::Result<()> {
        let mut recent = recent.borrow_mut();
        let app_id = HSTRING::from(windows_app_id());
        let group = HSTRING::from("swoop-downloads");
        let history = ToastNotificationManager::History()?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id)?;
        if !recent.initialized {
            // 清理本应用旧版无 tag 通知；借用的 PowerShell 标识只清理自己的分组。
            if windows_app_id() == WINDOWS_APP_ID {
                history.ClearWithId(&app_id)?;
            } else {
                history.RemoveGroupWithId(&group, &app_id)?;
            }
            recent.initialized = true;
        }
        let slot = recent.next;
        let tag = HSTRING::from(format!("recent-{slot}"));
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(windows_toast_xml(&message)))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&tag)?;
        toast.SetGroup(&group)?;
        let responder = responder.clone();
        let path = message.file;
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            let argument = args
                .as_ref()
                .and_then(|args| args.cast::<ToastActivatedEventArgs>().ok())
                .and_then(|args| args.Arguments().ok())
                .map(|args| args.to_string());
            responder.send(match (argument.as_deref(), &path) {
                (Some("open"), Some(path)) => NotificationAction::Open(path.clone()),
                (Some("reveal"), Some(path)) => NotificationAction::Reveal(path.clone()),
                _ => NotificationAction::Show,
            });
            Ok(())
        }))?;
        if let Some(old) = &recent.visible[slot] {
            notifier.Hide(old)?;
            history.RemoveGroupedTagWithId(&tag, &group, &app_id)?;
        }
        notifier.Show(&toast)?;
        recent.visible[slot] = Some(toast);
        recent.next = (slot + 1) % MAX_RECENT;
        Ok(())
    });
    result.map_err(|error| error.to_string())
}

#[cfg(any(windows, test))]
fn windows_toast_xml(message: &DesktopNotification) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let actions = if message.file.is_some() {
        format!(
            r#"<actions><action content="{}" arguments="open" activationType="foreground"/><action content="{}" arguments="reveal" activationType="foreground"/></actions>"#,
            escape(swoop_i18n::tr!("打开")),
            escape(swoop_i18n::tr!("打开文件夹"))
        )
    } else {
        String::new()
    };
    let audio = if message.sound {
        r#"<audio src="ms-winsoundevent:Notification.Default"/>"#
    } else {
        r#"<audio silent="true"/>"#
    };
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>{actions}{audio}</toast>"#,
        escape(message.summary),
        escape(&message.body)
    )
}

#[cfg(not(windows))]
fn show_plain(message: DesktopNotification) -> Result<(), String> {
    // Linux 通知正文允许标记，文件名必须作为普通文本显示。
    #[cfg(all(unix, not(target_os = "macos")))]
    let body = message.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let body = message.body;
    let mut notification = notify_rust::Notification::new();
    notification.appname("Swoop").summary(message.summary).body(&body);
    #[cfg(windows)]
    notification.app_id(windows_app_id());
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

    #[test]
    fn slow_backend_discards_old_pending_messages_instead_of_newest() {
        let (release, wait) = mpsc::channel::<()>();
        let (sent, received) = mpsc::channel();
        let notifier = Notifier::with_backend(
            || {},
            move |message, _| {
                let first = message.body == "0";
                sent.send(message.body).unwrap();
                if first {
                    wait.recv().unwrap();
                }
                Ok(())
            },
        )
        .unwrap();
        let send = |number: usize| {
            notifier.send(DesktopNotification { summary: "test", body: number.to_string(), sound: false, file: None })
        };
        send(0);
        assert_eq!(received.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "0");
        for number in 1..=100 {
            send(number);
        }
        assert_eq!(notifier.pending.0.lock().unwrap().queue.len(), MAX_RECENT);
        release.send(()).unwrap();
        for number in 98..=100 {
            assert_eq!(received.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), number.to_string());
        }
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn windows_toast_escapes_text_and_keeps_file_actions_and_sound() {
        let mut message =
            DesktopNotification { summary: "下载完成", body: "文件<&\"'".into(), sound: false, file: None };
        let plain = windows_toast_xml(&message);
        assert!(plain.contains("文件&lt;&amp;&quot;&apos;"));
        assert!(plain.contains(r#"<audio silent="true"/>"#));
        assert!(!plain.contains("<actions>"));
        message.file = Some(PathBuf::from("file.zip"));
        message.sound = true;
        let actionable = windows_toast_xml(&message);
        assert!(actionable.contains(r#"arguments="open""#));
        assert!(actionable.contains(r#"arguments="reveal""#));
        assert!(actionable.contains("Notification.Default"));
    }
}
