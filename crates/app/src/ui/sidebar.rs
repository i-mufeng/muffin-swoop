use egui::{Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, pos2, vec2};

use super::Filter;
use crate::app::SwoopApp;
use crate::fonts;
use crate::theme::{ICON_MD, R_CTRL};

impl SwoopApp {
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let ctx = ui.ctx().clone();
        egui::Panel::left("sidebar")
            .exact_size(208.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin { left: 12, right: 12, top: 44, bottom: 12 }))
            .show(ui, |ui| {
                // 品牌：强调色方块 + 闪电
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(8), p.accent);
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        egui_phosphor::fill::LIGHTNING,
                        fonts::icon_fill(15.0),
                        egui::Color32::WHITE,
                    );
                    ui.add_space(2.0);
                    ui.label(RichText::new("Swoop").size(17.0).strong().color(p.text));
                });
                ui.add_space(20.0);

                ui.add_space(2.0);
                ui.label(RichText::new("任务").size(11.0).color(p.text_muted));
                ui.add_space(4.0);

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
                        ui.add_space(2.0);
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
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
            let painter = ui.painter();
            if selected {
                painter.rect_filled(rect, CornerRadius::same(R_CTRL), p.accent_soft);
            } else if t > 0.01 {
                painter.rect_filled(rect, CornerRadius::same(R_CTRL), p.hover_tint.gamma_multiply(t));
            }
            let fg = if selected { p.text } else { p.text_muted.lerp_to_gamma(p.text, t) };
            let icon_color = if selected { p.accent } else { fg };
            painter.text(
                pos2(rect.left() + 20.0, rect.center().y),
                Align2::CENTER_CENTER,
                f.icon(),
                FontId::proportional(ICON_MD),
                icon_color,
            );
            painter.text(
                pos2(rect.left() + 38.0, rect.center().y),
                Align2::LEFT_CENTER,
                f.label(),
                FontId::proportional(13.5),
                fg,
            );
            if count > 0 {
                let (bg, fg) = if selected { (p.accent, egui::Color32::WHITE) } else { (p.soft(p.text_muted), p.text_muted) };
                let galley = painter.layout_no_wrap(count.to_string(), FontId::proportional(11.5), fg);
                let w = galley.size().x + 12.0;
                let pill = egui::Rect::from_min_size(pos2(rect.right() - 8.0 - w, rect.center().y - 9.0), vec2(w, 18.0));
                painter.rect_filled(pill, CornerRadius::same(9), bg);
                painter.galley(pill.center() - galley.size() / 2.0, galley, fg);
            }
        }
        ui.add_space(2.0);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}
