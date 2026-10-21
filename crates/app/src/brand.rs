//! 正式品牌 A「折光」。SVG 母版及生成脚本位于 assets/brand/swoop。

const APP: &[u8] = include_bytes!("../../../assets/brand/swoop/png/swoop-app-256.png");
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const TRAY: &[u8] = include_bytes!("../../../assets/brand/swoop/png/swoop-tray-44.png");

/// Windows 通知的 IconUri 需要磁盘上的图片文件
#[cfg(windows)]
pub fn app_png() -> &'static [u8] {
    APP
}

pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP).expect("embedded Swoop app icon must be valid PNG")
}

pub fn show_mark(ui: &mut egui::Ui, size: f32) {
    // SVG loader 按控件尺寸和屏幕 DPI 栅格化并缓存，避免缩放固定尺寸 PNG。
    ui.add(
        egui::Image::new(egui::include_image!("../../../assets/brand/swoop/swoop.svg"))
            .fit_to_exact_size(egui::vec2(size, size))
            .alt_text("Swoop"),
    );
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub fn tray_icon(color: [u8; 3]) -> egui::IconData {
    let mut icon = eframe::icon_data::from_png_bytes(TRAY).expect("embedded Swoop tray icon must be valid PNG");
    for pixel in icon.rgba.as_chunks_mut::<4>().0 {
        pixel[..3].copy_from_slice(&color);
    }
    icon
}
