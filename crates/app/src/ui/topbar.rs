use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use super::{icon_button, icon_text, pill, primary_button};
use crate::app::SwoopApp;
use crate::theme::{ICON_SM, R_CTRL};

impl SwoopApp {
    pub fn topbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        // 固定高度：egui 会优先采用持久化的旧面板尺寸，内容变矮时会留出未绘制的空带
        egui::Panel::top("topbar")
            .exact_size(62.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 10 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let count = self.tasks.iter().filter(|t| self.filter.matches(&t.status)).count();
                    ui.label(RichText::new(self.filter.label()).size(22.0).strong().color(p.text));
                    ui.add_space(2.0);
                    pill(ui, &format!("{count}"), p.soft(p.text_muted), p.text_muted);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &p, &format!("{}  新建任务", egui_phosphor::regular::PLUS)).clicked() {
                            self.add_dialog.open_with(None);
                        }
                        ui.add_space(6.0);
                        if icon_button(ui, &p, egui_phosphor::regular::PAUSE, "全部暂停").clicked() {
                            self.engine.pause_all();
                        }
                        if icon_button(ui, &p, egui_phosphor::regular::PLAY, "全部开始").clicked() {
                            self.engine.resume_all();
                        }
                        ui.add_space(6.0);

                        // 搜索框：自绘外框 + 无边框 TextEdit
                        Frame::new()
                            .fill(p.surface)
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(R_CTRL))
                            .inner_margin(Margin { left: 10, right: 8, top: 6, bottom: 6 })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.label(icon_text(egui_phosphor::regular::MAGNIFYING_GLASS, ICON_SM, p.text_muted));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.search)
                                            .frame(Frame::NONE)
                                            .hint_text("搜索任务")
                                            .desired_width(150.0)
                                            .margin(Margin::ZERO),
                                    );
                                });
                            });
                    });
                });
            });
    }
}
