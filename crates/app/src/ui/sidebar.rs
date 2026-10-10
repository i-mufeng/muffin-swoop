use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke, vec2};

use super::Filter;
use crate::app::SwoopApp;

impl SwoopApp {
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let ctx = ui.ctx().clone();
        egui::Panel::left("sidebar")
            .exact_size(200.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin { left: 12, right: 12, top: 44, bottom: 12 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(egui_phosphor::regular::LIGHTNING).size(20.0).color(p.accent));
                    ui.label(RichText::new("Swoop").size(18.0).strong().color(p.text));
                });
                ui.add_space(18.0);

                let mut clicked = None;
                for f in Filter::ALL {
                    let count = self.tasks.iter().filter(|t| f.matches(&t.status)).count();
                    if self.sidebar_item(ui, f, count) {
                        clicked = Some(f);
                    }
                }
                if let Some(f) = clicked {
                    self.filter = f;
                }

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        let icon = match p.mode {
                            crate::theme::Mode::Dark => egui_phosphor::regular::SUN,
                            crate::theme::Mode::Light => egui_phosphor::regular::MOON,
                        };
                        if super::icon_button(ui, &p, icon, "切换主题").clicked() {
                            self.toggle_theme(&ctx);
                        }
                        if super::icon_button(ui, &p, egui_phosphor::regular::GEAR, "设置").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                    });
                });
            });
    }

    fn sidebar_item(&self, ui: &mut egui::Ui, f: Filter, count: usize) -> bool {
        let p = self.palette;
        let selected = self.filter == f;
        let height = 36.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if selected {
                painter.rect(rect, CornerRadius::same(9), p.accent_soft, Stroke::NONE, egui::StrokeKind::Inside);
            } else if resp.hovered() {
                painter.rect_filled(rect, CornerRadius::same(9), p.surface_hover);
            }
            let fg = if selected { p.text } else { p.text_muted };
            painter.text(
                egui::pos2(rect.left() + 20.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                f.icon(),
                egui::FontId::proportional(17.0),
                if selected { p.accent } else { fg },
            );
            painter.text(
                egui::pos2(rect.left() + 38.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                f.label(),
                egui::FontId::proportional(14.0),
                fg,
            );
            if count > 0 {
                painter.text(
                    egui::pos2(rect.right() - 12.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    count.to_string(),
                    egui::FontId::proportional(12.0),
                    p.text_muted,
                );
            }
        }
        ui.add_space(2.0);
        resp.clicked()
    }
}
