use egui::{Align, Frame, Layout, Margin, RichText, Stroke};

use super::format;
use crate::app::SwoopApp;

impl SwoopApp {
    pub fn statusbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::bottom("statusbar")
            .exact_size(36.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin::symmetric(16, 0)))
            .show(ui, |ui| {
                let top = ui.max_rect().top();
                ui.painter().hline(ui.max_rect().x_range(), top, Stroke::new(1.0, p.border));

                ui.horizontal_centered(|ui| {
                    let speed = self.engine.global_speed();
                    let active = self.tasks.iter().filter(|t| t.status.is_active()).count();
                    ui.label(RichText::new(egui_phosphor::regular::ARROW_DOWN).size(14.0).color(p.accent));
                    ui.label(RichText::new(format::speed(speed)).size(13.0).strong().color(p.text));
                    ui.label(RichText::new(format!("·  {active} 个任务进行中")).size(12.5).color(p.text_muted));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.limit_enabled {
                            let resp = ui.add(
                                egui::DragValue::new(&mut self.limit_mbps)
                                    .range(0.1..=100_000.0)
                                    .speed(0.1)
                                    .max_decimals(1)
                                    .suffix(" MB/s"),
                            );
                            if resp.changed() {
                                self.engine.set_speed_limit(format::mbps_to_bps(self.limit_mbps));
                            }
                        }
                        let icon = egui_phosphor::regular::GAUGE;
                        if ui.checkbox(&mut self.limit_enabled, format!("{icon} 限速")).changed() {
                            let bps = if self.limit_enabled { format::mbps_to_bps(self.limit_mbps) } else { 0 };
                            self.engine.set_speed_limit(bps);
                        }
                    });
                });
            });
    }
}
