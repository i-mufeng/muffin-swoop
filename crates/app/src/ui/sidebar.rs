use egui::{Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, pos2, vec2};

use super::Filter;
use crate::app::SwoopApp;
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
                // A「折光」：蓝紫双折面，与正式 SVG 共用轮廓。
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    crate::brand::show_mark(ui, 26.0);
                    ui.add_space(2.0);
                    ui.label(RichText::new("Swoop").size(17.0).strong().color(p.text));
                });
                ui.add_space(20.0);

                egui::ScrollArea::vertical()
                    .id_salt("sidebar_sections")
                    .max_height((ui.available_height() - 108.0).max(60.0))
                    .show(ui, |ui| {
                        ui.add_space(2.0);
                        ui.label(RichText::new(swoop_i18n::tr!("任务")).size(11.0).color(p.text_muted));
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
                            self.queue_filter = None;
                            self.category_filter = None;
                        }

                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(swoop_i18n::tr!("队列")).size(11.0).color(p.text_muted));
                            if super::icon_button(
                                ui,
                                &p,
                                egui_phosphor::regular::GEAR,
                                swoop_i18n::tr!("管理队列与计划"),
                            )
                            .clicked()
                            {
                                self.queue_editor.show(self.queue_filter.unwrap_or(swoop_core::MAIN_QUEUE_ID));
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("sidebar_queues").max_height(180.0).show(ui, |ui| {
                            for queue in &self.queues {
                                let prefix = if queue.running {
                                    egui_phosphor::regular::PLAY
                                } else {
                                    egui_phosphor::regular::PAUSE
                                };
                                let label = format!(
                                    "{prefix} {}  {}",
                                    super::queue_name(queue),
                                    queue.active_count + queue.waiting_count
                                );
                                let response = ui.selectable_label(self.queue_filter == Some(queue.id), label);
                                if response.clicked() {
                                    self.queue_filter = Some(queue.id);
                                    self.filter = Filter::All;
                                    self.category_filter = None;
                                }
                                if response.double_clicked() {
                                    self.queue_editor.show(queue.id);
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(swoop_i18n::tr!("分类")).size(11.0).color(p.text_muted));
                            if super::icon_button(
                                ui,
                                &p,
                                egui_phosphor::regular::GEAR,
                                swoop_i18n::tr!("分类与保存规则"),
                            )
                            .clicked()
                            {
                                self.category_editor = Some(self.categories.clone());
                                self.category_error = None;
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("sidebar_categories").max_height(120.0).show(ui, |ui| {
                            for rule in &self.categories.rules {
                                if ui
                                    .selectable_label(self.category_filter.as_ref() == Some(&rule.name), &rule.name)
                                    .clicked()
                                {
                                    self.category_filter = Some(rule.name.clone());
                                    self.queue_filter = None;
                                    self.filter = Filter::All;
                                }
                            }
                        });
                    });

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    if ui.button(swoop_i18n::trf!("{}  站点抓取", egui_phosphor::regular::GLOBE)).clicked() {
                        self.grabber.open = true;
                        if self.grabber.config.output_dir.as_os_str().is_empty() {
                            self.grabber.config.output_dir = self.default_dir.join("Site");
                        }
                    }
                    if ui.button(swoop_i18n::trf!("{}  媒体下载", egui_phosphor::regular::FILM_STRIP)).clicked() {
                        self.media_window.open = true;
                        if self.media_window.destination.is_empty() {
                            self.media_window.destination = self.default_dir.join("media.mp4").display().to_string();
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        let icon = match p.mode {
                            crate::theme::Mode::Dark => egui_phosphor::regular::SUN,
                            crate::theme::Mode::Light => egui_phosphor::regular::MOON,
                        };
                        if super::icon_button(ui, &p, icon, swoop_i18n::tr!("切换主题")).clicked() {
                            self.toggle_theme(&ctx);
                        }
                        if super::icon_button(ui, &p, egui_phosphor::regular::GEAR, swoop_i18n::tr!("设置")).clicked()
                        {
                            self.show_settings = !self.show_settings;
                        }
                    });
                });
            });
    }

    fn sidebar_item(&self, ui: &mut egui::Ui, f: Filter, count: usize) -> bool {
        let p = self.palette;
        let selected = self.queue_filter.is_none() && self.category_filter.is_none() && self.filter == f;
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
                let (bg, fg) =
                    if selected { (p.accent, egui::Color32::WHITE) } else { (p.soft(p.text_muted), p.text_muted) };
                let galley = painter.layout_no_wrap(count.to_string(), FontId::proportional(11.5), fg);
                let w = galley.size().x + 12.0;
                let pill =
                    egui::Rect::from_min_size(pos2(rect.right() - 8.0 - w, rect.center().y - 9.0), vec2(w, 18.0));
                painter.rect_filled(pill, CornerRadius::same(9), bg);
                painter.galley(pill.center() - galley.size() / 2.0, galley, fg);
            }
        }
        ui.add_space(2.0);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}
