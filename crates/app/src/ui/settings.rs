use std::path::PathBuf;

use egui::{Align, Frame, Layout, Margin, RichText};

use super::primary_button;
use crate::app::SwoopApp;
use crate::theme::Mode;

impl SwoopApp {
    pub fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let p = self.palette;
        let mut close = false;

        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("设置").size(18.0).strong().color(p.text));
                ui.add_space(14.0);

                egui::Grid::new("settings_grid").num_columns(2).spacing([18.0, 14.0]).show(ui, |ui| {
                    ui.label(RichText::new("默认保存目录").color(p.text_muted));
                    let mut dir = self.default_dir.display().to_string();
                    if ui
                        .add(egui::TextEdit::singleline(&mut dir).desired_width(300.0).margin(Margin::symmetric(10, 6)))
                        .changed()
                    {
                        self.default_dir = PathBuf::from(&dir);
                        self.add_dialog.dir = dir;
                    }
                    ui.end_row();

                    ui.label(RichText::new("外观").color(p.text_muted));
                    ui.horizontal(|ui| {
                        if ui.selectable_label(p.mode == Mode::Dark, "深色").clicked() {
                            self.set_theme(ctx, Mode::Dark);
                        }
                        if ui.selectable_label(p.mode == Mode::Light, "浅色").clicked() {
                            self.set_theme(ctx, Mode::Light);
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("每任务连接数").color(p.text_muted));
                    ui.label(format!("{}（v0.2 支持修改与持久化）", self.engine.config().connections));
                    ui.end_row();

                    ui.label(RichText::new("版本").color(p.text_muted));
                    ui.label(format!("Swoop {}", env!("CARGO_PKG_VERSION")));
                    ui.end_row();
                });

                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, &p, "完成").clicked() {
                        close = true;
                    }
                });
            });

        if close || modal.should_close() {
            self.show_settings = false;
        }
    }
}
