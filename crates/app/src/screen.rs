//! 屏幕可用区域：去掉菜单栏、Dock、任务栏后的矩形，右下角的下载卡片贴着它摆。
//! egui / winit 只给整块屏幕的尺寸，这里分平台问系统。

use egui::Rect;

/// 鼠标所在屏幕的可用区域，单位是逻辑点，原点在主屏左上角（与 egui 的窗口坐标一致）。
/// 拿不到时返回 None，由调用方按主窗口所在屏幕估算。
pub fn work_area_at_cursor() -> Option<Rect> {
    platform::work_area_at_cursor()
}

#[cfg(target_os = "macos")]
mod platform {
    use egui::{Rect, pos2, vec2};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};

    pub fn work_area_at_cursor() -> Option<Rect> {
        let mtm = MainThreadMarker::new()?;
        let screens = NSScreen::screens(mtm);
        // 第一块是主屏（带菜单栏），AppKit 的全局坐标以它的左下角为原点、y 向上
        let primary = screens.firstObject()?;
        let primary_height = primary.frame().size.height;
        let mouse = NSEvent::mouseLocation();
        let screen = (0..screens.count())
            .map(|i| screens.objectAtIndex(i))
            .find(|screen| {
                let f = screen.frame();
                (f.origin.x..f.origin.x + f.size.width).contains(&mouse.x)
                    && (f.origin.y..f.origin.y + f.size.height).contains(&mouse.y)
            })
            .unwrap_or(primary);
        let v = screen.visibleFrame();
        let top = primary_height - (v.origin.y + v.size.height);
        Some(Rect::from_min_size(pos2(v.origin.x as f32, top as f32), vec2(v.size.width as f32, v.size.height as f32)))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use egui::{Rect, pos2};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    pub fn work_area_at_cursor() -> Option<Rect> {
        // winit 让进程按显示器感知 DPI，这里拿到的都是物理像素；按该显示器的缩放换回逻辑点
        unsafe {
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor).ok()?;
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return None;
            }
            let (mut dpi_x, mut dpi_y) = (96, 96);
            GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).ok()?;
            let scale = dpi_x.max(1) as f32 / 96.0;
            let r = info.rcWork;
            Some(Rect::from_min_max(
                pos2(r.left as f32 / scale, r.top as f32 / scale),
                pos2(r.right as f32 / scale, r.bottom as f32 / scale),
            ))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn work_area_at_cursor() -> Option<egui::Rect> {
        None
    }
}
