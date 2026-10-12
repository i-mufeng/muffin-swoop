use crate::app::SwoopApp;
use chrono::{Local, TimeZone};
use egui::{Frame, Margin, RichText};

use super::dialog;

impl SwoopApp {
    pub fn quota_ui(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.quota_editor else {
            return;
        };
        let p = self.palette;
        let quota = self.engine.quota();
        let mut save = false;
        let mut close = false;
        let modal = dialog::modal("quota_settings", &p).show(ctx, |ui| {
            ui.set_width(480.0);
            close |= dialog::header(
                ui,
                &p,
                egui_phosphor::regular::GAUGE,
                swoop_i18n::tr!("文件下载配额"),
                Some(swoop_i18n::tr!("用尽后等待下个周期；重新打开应用保留用量")),
            );
            dialog::body(ui, "quota_body", |ui| {
                // 用量概览
                Frame::new()
                    .fill(p.group)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(swoop_i18n::trf!("本周期已用 {}", super::format::bytes(quota.used_bytes)))
                                .size(15.0)
                                .strong()
                                .color(p.text),
                        );
                        if quota.policy.max_bytes > 0 {
                            ui.add_space(8.0);
                            let ratio = (quota.used_bytes as f32 / quota.policy.max_bytes as f32).clamp(0.0, 1.0);
                            let (bar, _) =
                                ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
                            ui.painter().rect_filled(bar, egui::CornerRadius::same(3), p.track);
                            let fill = if ratio >= 1.0 { p.danger } else { p.accent };
                            ui.painter().rect_filled(
                                bar.with_max_x(bar.left() + bar.width() * ratio),
                                egui::CornerRadius::same(3),
                                fill,
                            );
                            ui.add_space(6.0);
                            let next = Local
                                .timestamp_opt(quota.resets_at, 0)
                                .single()
                                .map(|t| t.format("%m-%d %H:%M:%S").to_string())
                                .unwrap_or_default();
                            dialog::note(ui, &p, &swoop_i18n::trf!("重置时间 {next}"));
                        }
                    });
                ui.add_space(12.0);
                dialog::group(ui, &p, None, |ui| {
                    let mut enabled = draft.max_bytes > 0;
                    dialog::row(ui, &p, swoop_i18n::tr!("限制每个周期的下载量"), None, |ui| {
                        if super::toggle(ui, &p, &mut enabled).changed() {
                            draft.max_bytes = if enabled { 1024 * 1024 * 1024 } else { 0 };
                        }
                    });
                    if enabled {
                        dialog::row(ui, &p, swoop_i18n::tr!("下载上限"), None, |ui| {
                            let mut mib = (draft.max_bytes / (1024 * 1024)).max(1);
                            if ui
                                .add(egui::DragValue::new(&mut mib).range(1..=1024 * 1024 * 1024).suffix(" MiB"))
                                .changed()
                            {
                                draft.max_bytes = mib * 1024 * 1024;
                            }
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("周期"), None, |ui| {
                            let mut minutes = (draft.window_secs / 60).max(1);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut minutes)
                                        .range(1..=525600)
                                        .suffix(swoop_i18n::tr!(" 分钟")),
                                )
                                .changed()
                            {
                                draft.window_secs = minutes * 60;
                            }
                        });
                    }
                });
                if let Some(error) = &self.quota_error {
                    dialog::error(ui, &p, error);
                }
                if let Some(error) = &quota.error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                close |= super::ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked();
            });
        });
        if save {
            match self.engine.set_quota_policy(draft.clone()) {
                Ok(()) => {
                    self.quota_editor = None;
                    self.quota_error = None;
                    self.toasts.push_info(swoop_i18n::tr!("配额已保存"), "");
                }
                Err(error) => self.quota_error = Some(error.to_string()),
            }
        } else if close || modal.should_close() {
            self.quota_editor = None;
            self.quota_error = None;
        }
    }
}
