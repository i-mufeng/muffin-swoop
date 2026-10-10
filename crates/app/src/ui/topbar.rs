use egui::{Align, Frame, Layout, Margin, RichText};

use super::{icon_button, primary_button};
use crate::app::SwoopApp;

impl SwoopApp {
    pub fn topbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::top("topbar")
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 8 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let count = self.tasks.iter().filter(|t| self.filter.matches(&t.status)).count();
                    ui.label(RichText::new(self.filter.label()).size(22.0).strong().color(p.text));
                    ui.add_space(2.0);
                    ui.label(RichText::new(format!("{count} 项")).size(13.0).color(p.text_muted));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &p, &format!("{}  新建任务", egui_phosphor::regular::PLUS)).clicked() {
                            self.add_dialog.open_with(None);
                        }
                        ui.add_space(4.0);
                        if icon_button(ui, &p, egui_phosphor::regular::PAUSE, "全部暂停").clicked() {
                            self.engine.pause_all();
                        }
                        if icon_button(ui, &p, egui_phosphor::regular::PLAY, "全部开始").clicked() {
                            self.engine.resume_all();
                        }
                        ui.add_space(8.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text(format!("{}  搜索任务", egui_phosphor::regular::MAGNIFYING_GLASS))
                                .desired_width(200.0)
                                .margin(Margin::symmetric(10, 6)),
                        );
                    });
                });
            });
    }
}
