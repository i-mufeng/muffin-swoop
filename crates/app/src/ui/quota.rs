use crate::app::SwoopApp;
use chrono::{Local, TimeZone};
use egui::{Frame, Margin, RichText};

impl SwoopApp {
    pub fn quota_ui(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.quota_editor else {
            return;
        };
        let p = self.palette;
        let quota = self.engine.quota();
        let mut save = false;
        let modal = egui::Modal::new(egui::Id::new("quota_settings"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new(swoop_i18n::tr!("文件下载配额")).size(18.0).strong());
                let mut enabled = draft.max_bytes > 0;
                if ui.checkbox(&mut enabled, swoop_i18n::tr!("限制每个周期的下载量")).changed() {
                    draft.max_bytes = if enabled { 1024 * 1024 * 1024 } else { 0 };
                }
                if enabled {
                    ui.horizontal(|ui| {
                        let mut mib = (draft.max_bytes / (1024 * 1024)).max(1);
                        if ui.add(egui::DragValue::new(&mut mib).range(1..=1024 * 1024 * 1024).suffix(" MiB")).changed()
                        {
                            draft.max_bytes = mib * 1024 * 1024;
                        }
                        ui.label(swoop_i18n::tr!("/ 每"));
                        let mut minutes = (draft.window_secs / 60).max(1);
                        if ui
                            .add(egui::DragValue::new(&mut minutes).range(1..=525600).suffix(swoop_i18n::tr!(" 分钟")))
                            .changed()
                        {
                            draft.window_secs = minutes * 60;
                        }
                    });
                }
                ui.label(swoop_i18n::trf!("本周期已用 {}", super::format::bytes(quota.used_bytes)));
                if quota.policy.max_bytes > 0 {
                    let next = Local
                        .timestamp_opt(quota.resets_at, 0)
                        .single()
                        .map(|t| t.format("%m-%d %H:%M:%S").to_string())
                        .unwrap_or_default();
                    ui.label(swoop_i18n::trf!("重置时间 {next}"));
                }
                ui.label(
                    RichText::new(swoop_i18n::tr!("用尽后等待下个周期；重新打开应用保留用量"))
                        .size(12.0)
                        .color(p.text_muted),
                );
                if let Some(error) = &self.quota_error {
                    ui.colored_label(p.danger, error);
                }
                if let Some(error) = &quota.error {
                    ui.colored_label(p.danger, error);
                }
                ui.add_space(12.0);
                save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
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
        } else if modal.should_close() {
            self.quota_editor = None;
            self.quota_error = None;
        }
    }
}
