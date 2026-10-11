use crate::app::SwoopApp;
use crate::completion::{CompletionSettings, FileAction, IdleAction, Scanner};
use egui::{Frame, Margin, RichText};
use std::time::Instant;

pub struct CompletionEditor {
    settings: CompletionSettings,
    arguments: String,
    inactive_scanner: Scanner,
    error: Option<String>,
}
impl CompletionEditor {
    pub fn new(settings: &CompletionSettings) -> Self {
        Self {
            settings: settings.clone(),
            inactive_scanner: settings.scanner.clone().unwrap_or_default(),
            arguments: settings.scanner.as_ref().map(|s| s.arguments.join("\n")).unwrap_or_else(|| "{file}".into()),
            error: None,
        }
    }
    fn toggle_scanner(&mut self, enabled: bool) {
        if enabled {
            self.settings.scanner = Some(self.inactive_scanner.clone());
            self.sync_arguments();
        } else if let Some(scanner) = self.settings.scanner.take() {
            self.inactive_scanner = scanner;
        }
    }
    fn sync_arguments(&mut self) {
        if let Some(scanner) = &mut self.settings.scanner {
            scanner.arguments = self.arguments.lines().map(str::to_string).collect();
        }
    }
}

impl SwoopApp {
    pub(crate) fn poll_completion(&mut self, ctx: &egui::Context) {
        for outcome in self.completion.poll() {
            if let Err(error) = outcome.result {
                self.toasts.push_error(
                    swoop_i18n::tr!("完成动作未成功"),
                    format!("{}：{error}", outcome.path.file_name().unwrap_or_default().to_string_lossy()),
                );
            }
        }
        if let Some(result) = self.completion.poll_shutdown() {
            match result {
                Ok(()) => self.toasts.push_info(swoop_i18n::tr!("已向系统请求关机"), ""),
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("系统未接受关机请求"), error),
            }
        }
        if self.is_quitting() {
            self.completion.cancel_all();
            return;
        }
        let now = Instant::now();
        let was_counting = self.completion.countdown.remaining(now).is_some();
        // 暂停、失败与等待队列都属于未完成；不因某个队列暂时空闲就关闭整个应用。
        let ready = self.file_conflict.is_none()
            && self.completion_editor.is_none()
            && !self.show_settings
            && !self.add_dialog.open
            && !ctx.input(|i| i.viewport().close_requested())
            && !self.startup.busy()
            && !self.completion.busy()
            && !self.completion.failed
            && !self.grabber.busy()
            && self.grabber.error.is_none()
            && self
                .grabber
                .snapshot
                .as_ref()
                .is_none_or(|s| s.status == swoop_grabber::ProjectStatus::Complete && s.failed() == 0)
            && self.engine.snapshot().iter().all(|t| t.status == swoop_core::TaskStatus::Completed)
            && self.media_tasks.jobs.iter().all(|j| j.status == crate::media_tasks::MediaStatus::Completed);
        let action = self.completion.countdown.tick(now, ready, &self.completion.settings);
        if !was_counting && self.completion.countdown.remaining(now).is_some() {
            self.show_window(ctx);
        }
        match action {
            Some(IdleAction::Quit) => self.request_quit(ctx),
            Some(IdleAction::Shutdown) => {
                if let Err(error) = self.completion.request_shutdown() {
                    self.toasts.push_error(swoop_i18n::tr!("关机请求未执行"), error);
                }
            }
            _ => {}
        }
    }

    pub fn completion_ui(&mut self, ctx: &egui::Context) {
        let p = self.palette;
        if let Some(editor) = &mut self.completion_editor {
            let mut save = false;
            let modal = egui::Modal::new(egui::Id::new("completion_settings"))
                .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
                .show(ctx, |ui| {
                    ui.set_width(500.0);
                    ui.label(RichText::new(swoop_i18n::tr!("下载完成后")).size(18.0).strong());
                    ui.add_space(12.0);
                    egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 200.0).max(160.0)).show(
                        ui,
                        |ui| {
                            ui.label(swoop_i18n::tr!("每个文件完成"));
                            ui.horizontal(|ui| {
                                for (action, label) in [
                                    (FileAction::None, swoop_i18n::tr!("不打开")),
                                    (FileAction::Reveal, swoop_i18n::tr!("打开所在目录")),
                                    (FileAction::Open, swoop_i18n::tr!("打开文件")),
                                ] {
                                    ui.selectable_value(&mut editor.settings.file_action, action, label);
                                }
                            });
                            if editor.settings.file_action == FileAction::Open {
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("可执行文件也会交给系统打开"))
                                        .size(12.0)
                                        .color(p.tint_amber),
                                );
                            }
                            ui.checkbox(&mut editor.settings.show_dialog, swoop_i18n::tr!("显示完成窗口"));
                            let mut scan = editor.settings.scanner.is_some();
                            if ui.checkbox(&mut scan, swoop_i18n::tr!("先扫描文件，成功后再打开")).changed()
                            {
                                editor.toggle_scanner(scan);
                            }
                            if let Some(scanner) = &mut editor.settings.scanner {
                                let mut program = scanner.program.to_string_lossy().into_owned();
                                ui.label(swoop_i18n::tr!("扫描程序"));
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut program)
                                            .desired_width(470.0)
                                            .hint_text(swoop_i18n::tr!("程序的完整路径")),
                                    )
                                    .changed()
                                {
                                    scanner.program = program.into();
                                }
                                ui.label(swoop_i18n::tr!("参数 · 每行一个，{file} 表示下载文件"));
                                if ui
                                    .add(
                                        egui::TextEdit::multiline(&mut editor.arguments)
                                            .desired_width(470.0)
                                            .desired_rows(3),
                                    )
                                    .changed()
                                {
                                    scanner.arguments = editor.arguments.lines().map(str::to_string).collect();
                                }
                                ui.horizontal(|ui| {
                                    ui.label(swoop_i18n::tr!("扫描超时"));
                                    ui.add(
                                        egui::DragValue::new(&mut scanner.timeout_secs)
                                            .range(1..=86400)
                                            .suffix(swoop_i18n::tr!(" 秒")),
                                    );
                                });
                            }
                            ui.add_space(16.0);
                            ui.label(swoop_i18n::tr!("全部下载成功后"));
                            ui.horizontal(|ui| {
                                for action in [IdleAction::None, IdleAction::Quit, IdleAction::Shutdown] {
                                    ui.selectable_value(&mut editor.settings.idle_action, action, action.label());
                                }
                            });
                            if editor.settings.idle_action != IdleAction::None {
                                ui.horizontal(|ui| {
                                    ui.label(swoop_i18n::tr!("可取消的倒计时"));
                                    ui.add(
                                        egui::DragValue::new(&mut editor.settings.countdown_secs)
                                            .range(10..=3600)
                                            .suffix(swoop_i18n::tr!(" 秒")),
                                    );
                                });
                            }
                            if let Some(error) = &editor.error {
                                ui.colored_label(p.danger, error);
                            }
                        },
                    );
                    if self.completion.busy() && ui.button(swoop_i18n::tr!("停止当前扫描与打开动作")).clicked()
                    {
                        self.completion.cancel_all();
                    }
                    ui.add_space(14.0);
                    save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                });
            if save {
                editor.sync_arguments();
                match editor.settings.save(&self.store) {
                    Ok(()) => {
                        self.completion.settings = editor.settings.clone();
                        self.completion.countdown.cancel();
                        self.completion.failed = false;
                        self.completion_editor = None;
                        self.toasts.push_info(swoop_i18n::tr!("完成动作已保存"), "");
                    }
                    Err(error) => editor.error = Some(error),
                }
            } else if modal.should_close() {
                self.completion_editor = None;
            }
            return;
        }
        if let Some(seconds) = self.completion.countdown.remaining(Instant::now()) {
            let modal = egui::Modal::new(egui::Id::new("completion_countdown"))
                .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
                .show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.label(
                        RichText::new(swoop_i18n::trf!(
                            "{seconds} 秒后{}",
                            self.completion.settings.idle_action.label()
                        ))
                        .size(18.0)
                        .strong(),
                    );
                    ui.add_space(14.0);
                    if super::primary_button(ui, &p, swoop_i18n::tr!("取消本次动作")).clicked() {
                        self.completion.countdown.cancel();
                    }
                });
            if modal.should_close() {
                self.completion.countdown.cancel();
            }
        }
    }

    /// 完成窗口（独立窗口，对标 IDM 的下载完成对话框）；多个文件依次显示
    pub fn completed_window_ui(&mut self, ctx: &egui::Context) {
        let Some(path) = self.completion.dialogs.front().cloned() else { return };
        let p = self.palette;
        let size = egui::vec2(440.0, 170.0);
        let builder = super::popup::builder(swoop_i18n::tr!("下载完成"), size, super::popup::centered(ctx, size, 0));
        let mut action = None;
        let mut close_clicked = false;
        let mut close = super::popup::show(ctx, egui::ViewportId::from_hash_of("swoop-completed"), builder, &p, |ui| {
            ui.label(RichText::new(swoop_i18n::tr!("下载完成")).size(18.0).strong().color(p.success));
            ui.add(
                egui::Label::new(RichText::new(path.file_name().unwrap_or_default().to_string_lossy()).color(p.text))
                    .truncate(),
            );
            if let Some(dir) = path.parent() {
                ui.add(
                    egui::Label::new(RichText::new(dir.display().to_string()).size(12.0).color(p.text_muted))
                        .truncate(),
                );
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if super::primary_button(ui, &p, swoop_i18n::tr!("打开")).clicked() {
                        action = Some(FileAction::Open);
                    }
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("打开文件夹")).clicked() {
                        action = Some(FileAction::Reveal);
                    }
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("关闭")).clicked() {
                        close_clicked = true;
                    }
                });
            });
        });
        close |= close_clicked;
        if let Some(action) = action {
            match self.completion.open_path(path, action) {
                Ok(()) => close = true,
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("无法打开"), error),
            }
        }
        if close {
            self.completion.dialogs.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toggling_scanning_retains_visible_program_arguments_and_timeout() {
        let settings = CompletionSettings {
            scanner: Some(Scanner {
                program: std::path::PathBuf::from("scanner"),
                arguments: vec!["--scan".into(), "{file}".into()],
                timeout_secs: 42,
            }),
            ..Default::default()
        };
        let mut editor = CompletionEditor::new(&settings);
        editor.toggle_scanner(false);
        editor.toggle_scanner(true);
        assert_eq!(editor.settings.scanner, settings.scanner);
        editor.arguments = "--new
{file}"
            .into();
        editor.sync_arguments();
        assert_eq!(editor.settings.scanner.unwrap().arguments, ["--new", "{file}"]);
    }
}
