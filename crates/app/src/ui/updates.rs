use crate::app::SwoopApp;
use swoop_updater::Channel;

impl SwoopApp {
    pub fn updates_ui(&mut self, ctx: &egui::Context) {
        if !self.updates.open {
            return;
        }
        let mut open = true;
        let idle = self.update_install_idle();
        egui::Window::new(swoop_i18n::tr!("软件更新")).open(&mut open).resizable(false).default_width(490.0).show(
            ctx,
            |ui| {
                ui.label(format!("Swoop {}", env!("CARGO_PKG_VERSION")));
                let busy = self.updates.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.updates.draft.automatic_check, swoop_i18n::tr!("自动检查"));
                        ui.add(
                            egui::DragValue::new(&mut self.updates.draft.interval_hours)
                                .range(1..=8760)
                                .suffix(swoop_i18n::tr!(" 小时")),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.updates.draft.channel,
                            Channel::Stable,
                            swoop_i18n::tr!("稳定版"),
                        );
                        ui.selectable_value(
                            &mut self.updates.draft.channel,
                            Channel::Preview,
                            swoop_i18n::tr!("预览版"),
                        );
                    });
                    ui.collapsing(swoop_i18n::tr!("更新源"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.updates.draft.feed_url)
                                .desired_width(460.0)
                                .hint_text(swoop_i18n::tr!("HTTPS 更新清单地址")),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.updates.draft.public_key)
                                .desired_width(460.0)
                                .hint_text(swoop_i18n::tr!("签名公钥（64 位十六进制）")),
                        );
                    });
                    ui.horizontal(|ui| {
                        if ui.button(swoop_i18n::tr!("保存设置")).clicked()
                            && let Err(error) = self.updates.save()
                        {
                            self.updates.error = Some(error);
                        }
                        if ui.button(swoop_i18n::tr!("检查更新")).clicked() {
                            self.updates.check(ctx);
                        }
                    });
                });
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        if ui.button(swoop_i18n::tr!("取消")).clicked() {
                            self.updates.cancel();
                        }
                    });
                }
                if !self.updates.message.is_empty() {
                    ui.label(&self.updates.message);
                }
                if let Some(candidate) = &self.updates.candidate {
                    ui.separator();
                    ui.heading(&candidate.release.version);
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        ui.label(&candidate.release.notes);
                    });
                    if self.updates.archive.is_none() {
                        if ui.add_enabled(!busy, egui::Button::new(swoop_i18n::tr!("下载更新"))).clicked() {
                            self.updates.download(ctx);
                        }
                    } else {
                        if ui.add_enabled(!busy && idle, egui::Button::new(swoop_i18n::tr!("安装并重启"))).clicked()
                        {
                            self.begin_update_install(ctx);
                        }
                        if !idle {
                            ui.label(swoop_i18n::tr!("请先暂停或结束当前任务"));
                        }
                    }
                }
                if let Some(error) = &self.updates.error {
                    ui.colored_label(self.palette.danger, error);
                }
            },
        );
        self.updates.open = open;
    }
}
