use crate::app::SwoopApp;
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
        let mut close = false;
        let editable = matches!(task.status, TaskStatus::Paused | TaskStatus::Failed(_) | TaskStatus::Queued);
        let modal = super::dialog::modal("task_properties", &p).show(ctx, |ui| {
            use super::dialog;
            ui.set_width(560.0);
            close |= dialog::header(
                ui,
                &p,
                egui_phosphor::regular::NOTE_PENCIL,
                swoop_i18n::tr!("下载属性"),
                Some(&task.file_name),
            );
            dialog::body(ui, "task_properties_body", |ui| {
                dialog::group(ui, &p, None, |ui| {
                    dialog::row_stack(ui, &p, swoop_i18n::tr!("描述"), None, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut editor.description)
                                .desired_width(f32::INFINITY)
                                .desired_rows(3)
                                .margin(egui::Margin::symmetric(10, 8))
                                .char_limit(4096),
                        );
                    });
                    dialog::row(ui, &p, swoop_i18n::tr!("任务限速"), None, |ui| {
                        super::toggle(ui, &p, &mut editor.limited);
                        ui.add_space(4.0);
                        ui.add_enabled(
                            editor.limited,
                            egui::DragValue::new(&mut editor.kib_per_second)
                                .range(1..=10 * 1024 * 1024)
                                .suffix(" KiB/s"),
                        );
                    });
                });

                dialog::group(ui, &p, None, |ui| {
                    dialog::row_stack(
                        ui,
                        &p,
                        swoop_i18n::tr!("下载地址"),
                        Some(if editable {
                            swoop_i18n::tr!("更新后重新检查续传条件")
                        } else {
                            swoop_i18n::tr!("暂停下载后可更新地址")
                        }),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.add(super::field(&mut editor.url).desired_width(ui.available_width() - 110.0));
                                ui.add_enabled_ui(editable && editor.url.trim() != task.url, |ui| {
                                    update_url = super::ghost_button(ui, &p, swoop_i18n::tr!("更新地址")).clicked();
                                });
                            });
                        },
                    );
                });

                if let Some(ftp) = &mut editor.ftp {
                    dialog::group(ui, &p, None, |ui| {
                        dialog::row_stack(ui, &p, "FTP", None, |ui| {
                            ui.add_enabled_ui(editable, |ui| {
                                ftp.ui(ui, task.url.starts_with("ftps:"));
                                ui.add_space(4.0);
                                save_ftp =
                                    super::ghost_button(ui, &p, swoop_i18n::tr!("应用 FTP 设置与登录")).clicked();
                            });
                        });
                    });
                }
                if let Some(error) = &editor.error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                save = super::primary_button(ui, &p, swoop_i18n::tr!("保存属性")).clicked();
                close |= super::ghost_button(ui, &p, swoop_i18n::tr!("关闭")).clicked();
            });
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
        if close || modal.should_close() {
            self.task_editor = None;
        }
    }
}
