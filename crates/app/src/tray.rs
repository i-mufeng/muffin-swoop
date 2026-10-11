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

    use super::{TrayCommand, TrayStatus, icon_rgba};
    use crate::ui::format;

    const ICON_PX: u32 = 44;

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
            let status = MenuItem::new("没有进行中的任务", false, None);
            let show = MenuItem::with_id("show", "显示 Swoop", true, None);
            let new_task = MenuItem::with_id("new", "新建任务…", true, None);
            let resume_all = MenuItem::with_id("resume_all", "全部开始", true, None);
            let pause_all = MenuItem::with_id("pause_all", "全部暂停", true, None);
            let quit = MenuItem::with_id("quit", "退出 Swoop", true, None);
            let menu = Menu::new();
            let sep = PredefinedMenuItem::separator;
            if let Err(e) = menu.append_items(&[
                &status,
                &sep(),
                &show,
                &new_task,
                &sep(),
                &resume_all,
                &pause_all,
                &sep(),
                &quit,
            ]) {
                tracing::warn!("创建托盘菜单失败: {e}");
                return None;
            }

            // macOS 用模板图（只看透明度，系统按菜单栏深浅着色）；Windows 托盘底色不定，用品牌色
            let color = if cfg!(target_os = "macos") { [0, 0, 0] } else { [0x6d, 0x8c, 0xff] };
            let icon = match Icon::from_rgba(icon_rgba(ICON_PX, color), ICON_PX, ICON_PX) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!("生成托盘图标失败: {e}");
                    return None;
                }
            };
            let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Swoop");
            let builder =
                if cfg!(target_os = "macos") { builder.with_icon_templated(icon) } else { builder.with_icon(icon) };
            let icon = match builder.build() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("创建托盘图标失败: {e}");
                    return None;
                }
            };

            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let cmd = match e.id.0.as_str() {
                    "show" => TrayCommand::Show,
                    "new" => TrayCommand::NewTask,
                    "resume_all" => TrayCommand::ResumeAll,
                    "pause_all" => TrayCommand::PauseAll,
                    "quit" => TrayCommand::Quit,
                    _ => return,
                };
                let _ = tx.send(cmd);
                ctx.request_repaint();
            }));
            // 点图标直接弹菜单，图标事件用不上；装个空处理器，免得事件堆在默认通道里
            TrayIconEvent::set_event_handler(Some(|_| {}));

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
                format!("↓ {}  ·  {} 个任务进行中", format::speed(s.speed), s.active)
            } else {
                "没有进行中的任务".to_string()
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

/// Phosphor「lightning」(fill) 的轮廓，256 单位坐标；圆角处用直线近似，菜单栏尺寸下看不出差别
const BOLT: [(f32, f32); 10] = [
    (215.8, 118.2),
    (103.8, 238.2),
    (90.1, 230.6),
    (104.8, 157.3),
    (47.2, 135.7),
    (44.2, 122.7),
    (156.2, 2.7),
    (169.9, 9.7),
    (155.2, 83.1),
    (212.8, 104.7),
];

/// 把闪电画成 `px`×`px` 的 RGBA（4×4 超采样抗锯齿），上下各留 1/12 边距
pub fn icon_rgba(px: u32, color: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 4;
    let (min_y, max_y) = (2.7f32, 238.2f32);
    let pad = px as f32 / 12.0;
    let scale = (px as f32 - 2.0 * pad) / (max_y - min_y);
    // 水平居中
    let (min_x, max_x) = (44.2f32, 215.8f32);
    let off_x = (px as f32 - (max_x - min_x) * scale) / 2.0;
    let to_unit = |x: f32, y: f32| ((x - off_x) / scale + min_x, (y - pad) / scale + min_y);

    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for y in 0..px {
        for x in 0..px {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let fy = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    let (ux, uy) = to_unit(fx, fy);
                    if inside(&BOLT, ux, uy) {
                        hits += 1;
                    }
                }
            }
            let a = (hits * 255 / (SS * SS)) as u8;
            out.extend_from_slice(&[color[0], color[1], color[2], a]);
        }
    }
    out
}

/// 射线法判断点是否在多边形内
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(img: &[u8], px: u32, x: u32, y: u32) -> u8 {
        img[((y * px + x) * 4 + 3) as usize]
    }

    #[test]
    fn bolt_icon_shape() {
        let px = 44;
        let img = icon_rgba(px, [0, 0, 0]);
        assert_eq!(img.len(), (px * px * 4) as usize);
        // 中心在闪电腰部，实心；四角透明
        assert_eq!(alpha(&img, px, px / 2, px / 2), 255);
        for (x, y) in [(0, 0), (px - 1, 0), (0, px - 1), (px - 1, px - 1)] {
            assert_eq!(alpha(&img, px, x, y), 0);
        }
        // 上下边距内没有像素
        assert!((0..px).all(|x| alpha(&img, px, x, 0) == 0 && alpha(&img, px, x, px - 1) == 0));
        // 有半透明的抗锯齿边
        assert!(img.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
    }
}
