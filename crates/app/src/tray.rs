//! 菜单栏（托盘）图标：总速度、全部开始/暂停、显示窗口、退出。
//!
//! 托盘回调只把命令转发进通道并唤醒 egui；真正的处理在 `SwoopApp::logic` 里——
//! 窗口隐藏时 eframe 不跑 ui，但只要有人 `request_repaint`，`logic` 照样会被调用。
//! Linux 的托盘依赖 GTK + libappindicator，暂不启用：`Tray::new` 返回 None，关窗即退出。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    NewTask,
    ResumeAll,
    PauseAll,
    Quit,
}

/// 菜单栏的状态快照，`Tray::update` 据此刷新文案
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrayStatus {
    /// 总速度（字节/秒）
    pub speed: f64,
    /// 下载中/排队中的任务数
    pub active: usize,
    /// 暂停或失败、可以继续的任务数
    pub resumable: usize,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use imp::Tray;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{TrayCommand, TrayStatus};
    use crate::ui::format;

    pub struct Tray {
        icon: TrayIcon,
        status: MenuItem,
        resume_all: MenuItem,
        pause_all: MenuItem,
        rx: mpsc::Receiver<TrayCommand>,
        shown: Option<(String, Option<String>)>,
        last_update: Option<Instant>,
    }

    impl Tray {
        /// 必须在主线程、事件循环启动后调用（eframe 的创建回调里即可）
        pub fn new(ctx: &egui::Context) -> Option<Self> {
            let status = MenuItem::new(swoop_i18n::tr!("没有进行中的任务"), false, None);
            let show = MenuItem::with_id("show", swoop_i18n::tr!("显示 Swoop"), true, None);
            let new_task = MenuItem::with_id("new", swoop_i18n::tr!("新建任务…"), true, None);
            let resume_all = MenuItem::with_id("resume_all", swoop_i18n::tr!("全部开始"), true, None);
            let pause_all = MenuItem::with_id("pause_all", swoop_i18n::tr!("全部暂停"), true, None);
            let quit = MenuItem::with_id("quit", swoop_i18n::tr!("退出 Swoop"), true, None);
            let menu = Menu::new();
            let sep = PredefinedMenuItem::separator;
            if let Err(e) =
                menu.append_items(&[&status, &sep(), &show, &new_task, &sep(), &resume_all, &pause_all, &sep(), &quit])
            {
                tracing::warn!("创建托盘菜单失败: {e}");
                return None;
            }

            // macOS 用模板图（只看透明度，系统按菜单栏深浅着色）；Windows 托盘底色不定，用品牌色
            let color = if cfg!(target_os = "macos") { [0, 0, 0] } else { [0x6d, 0x8c, 0xff] };
            let pixels = crate::brand::tray_icon(color);
            let icon = match Icon::from_rgba(pixels.rgba, pixels.width, pixels.height) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!("生成托盘图标失败: {e}");
                    return None;
                }
            };
            let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Swoop");
            // Windows 习惯右键弹菜单、双击打开主窗口；左键单击也弹菜单的话双击会先闪出菜单
            #[cfg(target_os = "windows")]
            let builder = builder.with_menu_on_left_click(false);
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_templated(icon);
            #[cfg(target_os = "windows")]
            let builder = builder.with_icon(icon);
            let icon = match builder.build() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("创建托盘图标失败: {e}");
                    return None;
                }
            };

            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let cmd = match e.id.0.as_str() {
                    "show" => TrayCommand::Show,
                    "new" => TrayCommand::NewTask,
                    "resume_all" => TrayCommand::ResumeAll,
                    "pause_all" => TrayCommand::PauseAll,
                    "quit" => TrayCommand::Quit,
                    _ => return,
                };
                if matches!(cmd, TrayCommand::Show | TrayCommand::NewTask) {
                    super::raise_window();
                }
                let _ = menu_tx.send(cmd);
                menu_ctx.request_repaint();
            }));
            // macOS 点图标直接弹菜单；Windows 双击图标打开主窗口。其余图标事件丢弃，免得堆在默认通道里
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::DoubleClick { button: tray_icon::MouseButton::Left, .. } = e {
                    super::raise_window();
                    let _ = tx.send(TrayCommand::Show);
                    ctx.request_repaint();
                }
            }));

            Some(Self { icon, status, resume_all, pause_all, rx, shown: None, last_update: None })
        }

        pub fn try_recv(&self) -> Option<TrayCommand> {
            self.rx.try_recv().ok()
        }

        /// 每秒最多刷新一次，文案没变就不碰系统控件
        pub fn update(&mut self, s: TrayStatus, show_speed: bool) {
            if self.last_update.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return;
            }
            self.last_update = Some(Instant::now());

            let status = if s.active > 0 {
                swoop_i18n::trf!("↓ {}  ·  {} 个任务进行中", format::speed(s.speed), s.active)
            } else {
                swoop_i18n::tr!("没有进行中的任务").to_string()
            };
            let title = (show_speed && s.active > 0).then(|| format::speed(s.speed));
            let next = (status, title);
            if self.shown.as_ref() == Some(&next) {
                return;
            }
            self.status.set_text(&next.0);
            self.pause_all.set_enabled(s.active > 0);
            self.resume_all.set_enabled(s.resumable > 0);
            // 图标旁的文字只有 macOS 菜单栏支持，Windows 上是空操作
            self.icon.set_title(next.1.as_deref());
            self.shown = Some(next);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct Tray;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Tray {
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }
    pub fn try_recv(&self) -> Option<TrayCommand> {
        None
    }
    pub fn update(&mut self, _s: TrayStatus, _show_speed: bool) {}
}

#[cfg(target_os = "windows")]
static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 记下主窗口句柄，供 `raise_window` 在 egui 之外直接显示窗口
pub fn register_window(handle: &impl raw_window_handle::HasWindowHandle) {
    #[cfg(target_os = "windows")]
    if let Ok(raw_window_handle::RawWindowHandle::Win32(h)) = handle.window_handle().map(|h| h.as_raw()) {
        WINDOW.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = handle;
}

/// Windows 上窗口 `Visible(false)` 后，靠 eframe 帧循环再把它显示出来并不可靠（托盘菜单点了没反应），
/// 这里直接调 Win32 显示并前置；窗口可见后正常帧会跑 `logic`，再由 `show_window` 同步 `hidden` 等状态。
/// 其他平台什么也不做，照常走 `logic`。
pub fn raise_window() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow};
        let raw = WINDOW.load(std::sync::atomic::Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        let hwnd = HWND(raw as *mut _);
        // SAFETY: 句柄来自 eframe 主窗口，进程存活期间一直有效；托盘回调在主线程上执行
        unsafe {
            let cmd = if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW };
            let _ = ShowWindow(hwnd, cmd);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// 关窗驻留时隐藏 Dock 图标（只留菜单栏），显示窗口时恢复。其他平台隐藏窗口即可，无需处理。
pub fn set_dock_visible(visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("不在主线程，无法切换 Dock 图标");
            return;
        };
        let policy =
            if visible { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
        NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = visible;
}

#[cfg(test)]
mod tests {
    fn alpha(img: &[u8], px: u32, x: u32, y: u32) -> u8 {
        img[((y * px + x) * 4 + 3) as usize]
    }

    #[test]
    fn fold_template_has_padding_antialiasing_and_preserves_tint_alpha() {
        let icon = crate::brand::tray_icon([0, 0, 0]);
        let px = icon.width;
        assert_eq!((px, icon.height), (44, 44));
        let img = icon.rgba;
        assert_eq!(img.len(), (px * px * 4) as usize);
        // 有实心图形，四角透明；两片之间的负形保留。
        assert!(img.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        for (x, y) in [(0, 0), (px - 1, 0), (0, px - 1), (px - 1, px - 1)] {
            assert_eq!(alpha(&img, px, x, y), 0);
        }
        // 上下边距内没有像素
        assert!((0..px).all(|x| alpha(&img, px, x, 0) == 0 && alpha(&img, px, x, px - 1) == 0));
        // 有半透明的抗锯齿边
        assert!(img.as_chunks::<4>().0.iter().any(|p| p[3] > 0 && p[3] < 255));
        let tinted = crate::brand::tray_icon([109, 140, 255]);
        for (a, b) in img.as_chunks::<4>().0.iter().zip(tinted.rgba.as_chunks::<4>().0) {
            assert_eq!(a[3], b[3]);
            assert_eq!(&b[..3], &[109, 140, 255]);
        }
    }
}
