use egui::{Align, Frame, Layout, Margin, RichText, Stroke};

use super::{format, icon_text, toggle};
use crate::app::SwoopApp;
use crate::theme::ICON_SM;

impl SwoopApp {
    pub fn statusbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::bottom("statusbar")
            .exact_size(36.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin::symmetric(16, 0)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, p.border));

                ui.horizontal_centered(|ui| {
                    let speed = self.engine.global_speed();
                    let active = self.tasks.iter().filter(|t| t.status.is_active()).count();
                    ui.label(icon_text(egui_phosphor::regular::ARROW_DOWN, ICON_SM, p.accent));
                    ui.label(RichText::new(format::speed(speed)).size(13.0).strong().color(p.text));
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{active} 个任务进行中")).size(12.5).color(p.text_muted));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if toggle(ui, &p, &mut self.limit_enabled).changed() {
                            let bps = if self.limit_enabled { format::mbps_to_bps(self.limit_mbps) } else { 0 };
                            self.engine.set_speed_limit(bps);
                            self.save_settings();
                        }
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
                            if resp.drag_stopped() || resp.lost_focus() {
                                self.save_settings();
                            }
                        }
                        ui.label(RichText::new("限速").size(12.5).color(p.text_muted));
                        ui.label(icon_text(egui_phosphor::regular::GAUGE, ICON_SM, p.text_muted));
                    });
                });
            });
    }
}
