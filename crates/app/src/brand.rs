//! 正式品牌 A「折光」。SVG 母版及生成脚本位于 assets/brand/swoop。

const MARK: &[u8] = include_bytes!("../../../assets/brand/swoop/png/swoop-256.png");
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
    let id = egui::Id::new("swoop-brand-mark");
    let cached = ui
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let icon =
            eframe::icon_data::from_png_bytes(MARK).expect("embedded Swoop mark must be valid PNG");
        let texture = ui.ctx().load_texture(
            "swoop-brand-mark",
            egui::ColorImage::from(icon),
            egui::TextureOptions::LINEAR,
        );
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    ui.add(egui::Image::new((texture.id(), egui::vec2(size, size))).alt_text("Swoop"));
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub fn tray_icon(color: [u8; 3]) -> egui::IconData {
    let mut icon = eframe::icon_data::from_png_bytes(TRAY)
        .expect("embedded Swoop tray icon must be valid PNG");
    for pixel in icon.rgba.as_chunks_mut::<4>().0 {
        pixel[..3].copy_from_slice(&color);
    }
    icon
}
