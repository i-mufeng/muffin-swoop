use egui::{Frame, Margin, RichText};
use swoop_updater::Channel;

use super::dialog;
use crate::app::SwoopApp;

impl SwoopApp {
    pub fn updates_ui(&mut self, ctx: &egui::Context) {
        if !self.updates.open {
            return;
        }
        let mut open = true;
        let idle = self.update_install_idle();
        let p = self.palette;
        let modal = dialog::modal("software_updates", &p).show(ctx, |ui| {
            ui.set_width(540.0);
            if dialog::header(
                ui,
                &p,
                egui_phosphor::regular::ARROWS_CLOCKWISE,
                swoop_i18n::tr!("软件更新"),
                Some(&format!("Swoop {}", env!("CARGO_PKG_VERSION"))),
            ) {
                open = false;
            }
            let busy = self.updates.busy();
            dialog::body(ui, "updates_body", |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    dialog::group(ui, &p, None, |ui| {
                        dialog::row(ui, &p, swoop_i18n::tr!("自动检查"), None, |ui| {
                            super::toggle(ui, &p, &mut self.updates.draft.automatic_check);
                            ui.add_space(4.0);
                            ui.add_enabled(
                                self.updates.draft.automatic_check,
                                egui::DragValue::new(&mut self.updates.draft.interval_hours)
                                    .range(1..=8760)
                                    .suffix(swoop_i18n::tr!(" 小时")),
                            );
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("更新通道"), None, |ui| {
                            super::segmented(
                                ui,
                                &p,
                                &mut self.updates.draft.channel,
                                &[
                                    (Channel::Stable, swoop_i18n::tr!("稳定版")),
                                    (Channel::Preview, swoop_i18n::tr!("预览版")),
                                ],
                            );
                        });
                    });
                    egui::CollapsingHeader::new(RichText::new(swoop_i18n::tr!("更新源")).color(p.text_muted))
                        .id_salt("update_source")
                        .show(ui, |ui| {
                            ui.add(
                                super::field(&mut self.updates.draft.feed_url)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(swoop_i18n::tr!("HTTPS 更新清单地址")),
                            );
                            ui.add(
                                super::field(&mut self.updates.draft.public_key)
                                    .desired_width(f32::INFINITY)
                                    .hint_text(swoop_i18n::tr!("签名公钥（64 位十六进制）")),
                            );
                        });
                });
                ui.add_space(6.0);
                if busy || !self.updates.message.is_empty() {
                    ui.horizontal(|ui| {
                        if busy {
                            ui.spinner();
                        }
                        if !self.updates.message.is_empty() {
                            ui.label(RichText::new(&self.updates.message).color(p.text_muted));
                        }
                    });
                }
                if let Some(candidate) = &self.updates.candidate {
                    ui.add_space(8.0);
                    Frame::new()
                        .fill(p.group)
                        .stroke(egui::Stroke::new(1.0, p.border))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&candidate.release.version).size(16.0).strong().color(p.text));
                                super::pill(ui, swoop_i18n::tr!("新版本"), p.accent_soft, p.accent);
                            });
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                ui.label(RichText::new(&candidate.release.notes).color(p.text_muted));
                            });
                            if self.updates.archive.is_some() && !idle {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("请先暂停或结束当前任务"))
                                        .size(12.0)
                                        .color(p.tint_amber),
                                );
                            }
                        });
                }
                if let Some(error) = &self.updates.error {
                    ui.add_space(8.0);
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                if let Some(_candidate) = &self.updates.candidate {
                    if self.updates.archive.is_none() {
                        ui.add_enabled_ui(!busy, |ui| {
                            if super::primary_button(ui, &p, swoop_i18n::tr!("下载更新")).clicked() {
                                self.updates.download(ctx);
                            }
                        });
                    } else {
                        ui.add_enabled_ui(!busy && idle, |ui| {
                            if super::primary_button(ui, &p, swoop_i18n::tr!("安装并重启")).clicked() {
                                self.begin_update_install(ctx);
                            }
                        });
                    }
                }
                if busy {
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked() {
                        self.updates.cancel();
                    }
                } else {
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("检查更新")).clicked() {
                        self.updates.check(ctx);
                    }
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("保存设置")).clicked()
                        && let Err(error) = self.updates.save()
                    {
                        self.updates.error = Some(error);
                    }
                }
            });
        });
        if modal.should_close() && !self.updates.busy() {
            open = false;
        }
        self.updates.open = open;
    }
}
