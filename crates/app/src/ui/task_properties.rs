use crate::app::SwoopApp;
use egui::{Frame, Margin, RichText};
use swoop_core::{TaskId, TaskProperties, TaskStatus};

pub struct TaskEditor {
    pub id: TaskId,
    url: String,
    description: String,
    limited: bool,
    kib_per_second: u64,
    error: Option<String>,
    ftp: Option<crate::ftp_settings::FtpEditor>,
}

impl SwoopApp {
    pub fn edit_task(&mut self, id: TaskId) {
        let Some(task) = self.engine.snapshot_one(id) else {
            return;
        };
        match self.engine.task_properties(id) {
            Ok(properties) => {
                self.task_editor = Some(TaskEditor {
                    id,
                    url: task.url,
                    description: properties.description,
                    limited: properties.speed_limit > 0,
                    kib_per_second: (properties.speed_limit / 1024).max(1),
                    error: None,
                    ftp: self
                        .engine
                        .ftp_task_options(id)
                        .ok()
                        .flatten()
                        .map(crate::ftp_settings::FtpEditor::from_options),
                })
            }
            Err(error) => self.toasts.push_error(swoop_i18n::tr!("读取属性失败"), error.to_string()),
        }
    }

    pub fn task_properties_ui(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.task_editor else {
            return;
        };
        let Some(task) = self.engine.snapshot_one(editor.id) else {
            self.task_editor = None;
            return;
        };
        let p = self.palette;
        let mut save = false;
        let mut update_url = false;
        let mut save_ftp = false;
        let modal = egui::Modal::new(egui::Id::new("task_properties"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(530.0);
                ui.label(RichText::new(swoop_i18n::tr!("下载属性")).size(18.0).strong());
                ui.add(egui::Label::new(&task.file_name).truncate());
                ui.add_space(12.0);
                ui.label(swoop_i18n::tr!("描述"));
                ui.add(
                    egui::TextEdit::multiline(&mut editor.description)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .char_limit(4096),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut editor.limited, swoop_i18n::tr!("任务限速"));
                    ui.add_enabled(
                        editor.limited,
                        egui::DragValue::new(&mut editor.kib_per_second).range(1..=10 * 1024 * 1024).suffix(" KiB/s"),
                    );
                });
                save = super::primary_button(ui, &p, swoop_i18n::tr!("保存属性")).clicked();
                if let Some(ftp) = &mut editor.ftp {
                    ui.separator();
                    let editable =
                        matches!(task.status, TaskStatus::Paused | TaskStatus::Failed(_) | TaskStatus::Queued);
                    ui.add_enabled_ui(editable, |ui| {
                        egui::ScrollArea::vertical().id_salt("task_ftp_settings").max_height(220.0).show(ui, |ui| {
                            ftp.ui(ui, task.url.starts_with("ftps:"));
                            save_ftp = ui.button(swoop_i18n::tr!("应用 FTP 设置与登录")).clicked();
                        });
                    });
                }
                ui.add_space(12.0);
                ui.separator();
                ui.label(swoop_i18n::tr!("下载地址"));
                ui.add(egui::TextEdit::singleline(&mut editor.url).desired_width(f32::INFINITY));
                let editable = matches!(task.status, TaskStatus::Paused | TaskStatus::Failed(_) | TaskStatus::Queued);
                update_url = ui
                    .add_enabled(
                        editable && editor.url.trim() != task.url,
                        egui::Button::new(swoop_i18n::tr!("更新地址")),
                    )
                    .clicked();
                ui.label(
                    RichText::new(if editable {
                        swoop_i18n::tr!("更新后重新检查续传条件")
                    } else {
                        swoop_i18n::tr!("暂停下载后可更新地址")
                    })
                    .size(12.0)
                    .color(p.text_muted),
                );
                if let Some(error) = &editor.error {
                    ui.colored_label(p.danger, error);
                }
            });
        if save {
            let properties = TaskProperties {
                description: editor.description.clone(),
                speed_limit: if editor.limited { editor.kib_per_second.saturating_mul(1024) } else { 0 },
            };
            match self.engine.set_task_properties(editor.id, properties) {
                Ok(()) => {
                    editor.error = None;
                    self.toasts.push_info(swoop_i18n::tr!("属性已保存"), &task.file_name);
                }
                Err(error) => editor.error = Some(error.to_string()),
            }
        }
        if update_url {
            match self.engine.update_task_url(editor.id, editor.url.trim(), None) {
                Ok(()) => {
                    editor.error = None;
                    editor.ftp = self
                        .engine
                        .ftp_task_options(editor.id)
                        .ok()
                        .flatten()
                        .map(crate::ftp_settings::FtpEditor::from_options);
                    self.toasts.push_info(swoop_i18n::tr!("地址已更新"), swoop_i18n::tr!("点击继续下载时检查文件"));
                }
                Err(error) => editor.error = Some(error.to_string()),
            }
        }
        if save_ftp && let Some(ftp) = &mut editor.ftp {
            let result =
                ftp.prepare(&task.url).and_then(|(options, mut credentials)| {
                    // An unchanged, blank login field preserves the current session when only transport settings change.
                    if ftp.password.is_empty()
                        && !self.engine.ftp_auth_required(editor.id).map_err(|e| e.to_string())?
                        && self.engine.ftp_task_options(editor.id).map_err(|e| e.to_string())?.is_some_and(|previous| {
                            previous.username == options.username && previous.tls == options.tls
                        })
                    {
                        credentials = None;
                    }
                    self.engine.configure_ftp_task(editor.id, options, credentials).map_err(|e| e.to_string())
                });
            match result {
                Ok(()) => {
                    ftp.password.clear();
                    editor.error = None;
                    self.toasts.push_info(swoop_i18n::tr!("FTP 设置已应用"), swoop_i18n::tr!("点击继续下载"));
                }
                Err(error) => editor.error = Some(error),
            }
        }
        if modal.should_close() {
            self.task_editor = None;
        }
    }
}
