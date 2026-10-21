//! 手动安装浏览器扩展的独立置顶引导。

use egui::{RichText, ViewportId, vec2};

use super::{dialog, popup};
use crate::app::SwoopApp;

const SIZE: egui::Vec2 = vec2(390.0, 320.0);

impl SwoopApp {
    pub(crate) fn extension_guide_ui(&mut self, ctx: &egui::Context) {
        let Some(guide) = &self.extension_guide else { return };
        let p = self.palette;
        let position_id = egui::Id::new("swoop-extension-guide-position");
        let area = crate::screen::work_area_at_cursor().unwrap_or_else(|| {
            let size = ctx.input_for(ViewportId::ROOT, |i| i.viewport().monitor_size).unwrap_or(vec2(1440.0, 900.0));
            egui::Rect::from_min_size(egui::Pos2::ZERO, size)
        });
        let fallback_pos = egui::pos2(area.right() - SIZE.x - 18.0, area.top() + 18.0);
        let pos = ctx.data_mut(|data| {
            let pos = data.get_temp(position_id).unwrap_or(fallback_pos);
            data.insert_temp(position_id, pos);
            pos
        });
        let builder = popup::builder(swoop_i18n::tr!("浏览器扩展安装"), SIZE, Some(pos))
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_taskbar(false)
            .with_resizable(false);
        let mut close = false;
        let browser = guide.name.as_str();
        let chrome = guide.id == "chrome";
        let folder = guide.unpacked.as_ref();
        let dismissed = popup::show_guide(ctx, ViewportId::from_hash_of("swoop-extension-guide"), builder, &p, |ui| {
            dialog::heading(
                ui,
                &p,
                egui_phosphor::regular::PUZZLE_PIECE,
                swoop_i18n::tr!("浏览器扩展安装"),
                Some(browser),
            );
            ui.add_space(3.0);
            if chrome {
                step(ui, &p, "1", swoop_i18n::tr!("开启开发者模式"));
                step(ui, &p, "2", swoop_i18n::tr!("点击「加载已解压的扩展程序」"));
                step(ui, &p, "3", swoop_i18n::tr!("选择下方文件夹"));
            } else if guide.id == "edge" {
                step(ui, &p, "1", swoop_i18n::tr!("在商店页面点击「获取」"));
                step(ui, &p, "2", swoop_i18n::tr!("按浏览器提示完成安装"));
            } else {
                step(ui, &p, "1", swoop_i18n::tr!("在已打开的浏览器页面安装扩展"));
                step(ui, &p, "2", swoop_i18n::tr!("按浏览器提示完成安装"));
            }
            if let Some(folder) = folder {
                ui.add_space(14.0);
                egui::Frame::new().fill(p.inset).corner_radius(8).inner_margin(10).show(ui, |ui| {
                    ui.label(RichText::new(swoop_i18n::tr!("扩展文件夹")).size(11.0).color(p.text_muted));
                    ui.label(RichText::new(folder.display().to_string()).size(11.0).color(p.text).monospace())
                        .on_hover_text(folder.display().to_string());
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if super::primary_button(ui, &p, swoop_i18n::tr!("打开文件夹")).clicked()
                        && let Err(error) = std::process::Command::new("explorer.exe").arg(folder).spawn()
                    {
                        self.toasts.push_error(swoop_i18n::tr!("无法打开扩展文件夹"), error.to_string());
                    }
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("复制路径")).clicked() {
                        ui.ctx().copy_text(folder.display().to_string());
                    }
                });
            } else {
                ui.add_space(18.0);
                if super::ghost_button(ui, &p, swoop_i18n::tr!("完成")).clicked() {
                    close = true;
                }
            }
        });
        if dismissed || close {
            self.extension_guide = None;
            ctx.data_mut(|data| data.remove::<egui::Pos2>(position_id));
        }
    }
}

fn step(ui: &mut egui::Ui, p: &crate::theme::Palette, number: &str, label: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(number).size(12.0).strong().color(p.accent));
        ui.add_space(8.0);
        ui.label(RichText::new(label).size(13.0).color(p.text));
    });
    ui.add_space(9.0);
}
