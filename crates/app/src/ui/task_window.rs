//! 单任务下载进度窗口（对标 IDM 的下载进度对话框），完成后原地变为完成窗口。

use egui::{Align, Label, Layout, Pos2, RichText, ViewportId, vec2};
use swoop_core::{TaskId, TaskSnapshot, TaskStatus};

use super::{format, ghost_button, popup, primary_button, progress};
use crate::app::SwoopApp;
use crate::completion::FileAction;

/// 同时打开的进度窗口上限；批量添加时不会弹出一排窗口
const MAX_WINDOWS: usize = 8;
const SIZE: egui::Vec2 = vec2(460.0, 270.0);

pub struct TaskWindow {
    id: TaskId,
    cascade: usize,
    /// 首次在 ui 帧里按屏幕居中算出；`logic` 里（主窗口隐藏时）可能拿不到屏幕尺寸
    pos: Option<Pos2>,
}

#[derive(Default)]
pub struct TaskWindows {
    windows: Vec<TaskWindow>,
    opened: usize,
}

impl TaskWindows {
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn contains(&self, id: TaskId) -> bool {
        self.windows.iter().any(|w| w.id == id)
    }

    pub fn open(&mut self, ctx: &egui::Context, id: TaskId) {
        if self.contains(id) {
            ctx.send_viewport_cmd_to(viewport_id(id), egui::ViewportCommand::Focus);
            return;
        }
        if self.windows.len() >= MAX_WINDOWS {
            return;
        }
        self.windows.push(TaskWindow { id, cascade: self.opened, pos: None });
        self.opened += 1;
    }

    pub fn close(&mut self, id: TaskId) {
        self.windows.retain(|w| w.id != id);
    }
}

fn viewport_id(id: TaskId) -> ViewportId {
    ViewportId::from_hash_of(("swoop-task-window", id))
}

enum Action {
    Pause(TaskId),
    Resume(TaskId),
    Open(TaskId, std::path::PathBuf, FileAction),
    Close(TaskId),
}

impl SwoopApp {
    /// 单个下载开始时按设置弹出进度窗口
    pub(crate) fn open_task_window(&mut self, ctx: &egui::Context, id: TaskId) {
        if self.progress_window {
            self.task_windows.open(ctx, id);
        }
    }

    pub fn task_windows_ui(&mut self, ctx: &egui::Context) {
        let p = self.palette;
        let mut actions = Vec::new();
        for window in &mut self.task_windows.windows {
            let Some(task) = self.tasks.iter().find(|t| t.id == window.id) else {
                actions.push(Action::Close(window.id));
                continue;
            };
            let title = match task.progress() {
                Some(progress) if task.status != TaskStatus::Completed => {
                    format!("{:.0}% · {}", progress * 100.0, task.file_name)
                }
                _ => task.file_name.clone(),
            };
            if window.pos.is_none() {
                window.pos = popup::centered(ctx, SIZE, window.cascade);
            }
            let builder = popup::builder(&title, SIZE, window.pos);
            let style = self.progress_style;
            let close = popup::show(ctx, viewport_id(window.id), builder, &p, |ui| {
                task_contents(ui, &p, task, style, &mut actions);
            });
            if close {
                actions.push(Action::Close(window.id));
            }
        }
        for action in actions {
            match action {
                Action::Pause(id) => {
                    if let Err(error) = self.engine.pause(id) {
                        self.toasts.push_error(swoop_i18n::tr!("暂停失败"), error.to_string());
                    }
                }
                Action::Resume(id) => {
                    if let Err(error) = self.engine.resume(id) {
                        self.toasts.push_error(swoop_i18n::tr!("继续失败"), error.to_string());
                    }
                }
                Action::Open(id, path, action) => {
                    self.task_windows.close(id);
                    if let Err(error) = self.completion.open_path(path, action) {
                        self.toasts.push_error(swoop_i18n::tr!("无法打开"), error);
                    }
                }
                Action::Close(id) => self.task_windows.close(id),
            }
        }
    }
}

fn task_contents(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    t: &TaskSnapshot,
    style: progress::ProgressStyle,
    actions: &mut Vec<Action>,
) {
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.horizontal(|ui| {
        super::file_tile(ui, p, super::filekind::FileKind::from_name(&t.file_name), 40.0);
        ui.add_space(4.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            super::task_list::status_view(ui, p, t, false);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(Label::new(RichText::new(&t.file_name).size(15.0).strong().color(p.text)).truncate());
                ui.add(Label::new(RichText::new(&t.url).size(12.0).color(p.text_muted)).truncate());
            });
        });
    });
    ui.add_space(10.0);
    progress::bar(ui, p, t, style);
    ui.add_space(8.0);

    let row = |ui: &mut egui::Ui, label: &str, value: String| {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(56.0, 18.0), egui::Sense::hover());
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.5),
                p.text_muted,
            );
            ui.add(Label::new(RichText::new(value).size(12.5).color(p.text)).truncate());
        });
    };
    let size = match t.total {
        Some(total) => format!("{} / {}", format::bytes(t.downloaded), format::bytes(total)),
        None => swoop_i18n::trf!("{} · 大小未知", format::bytes(t.downloaded)),
    };
    match &t.status {
        TaskStatus::Completed => {
            row(ui, swoop_i18n::tr!("保存到"), t.path.display().to_string());
            row(ui, swoop_i18n::tr!("大小"), format::bytes(t.total.unwrap_or(t.downloaded)));
        }
        TaskStatus::Failed(reason) => {
            row(ui, swoop_i18n::tr!("已下载"), size);
            ui.add(Label::new(RichText::new(reason).size(12.5).color(p.danger)).truncate()).on_hover_text(reason);
        }
        _ => {
            row(ui, swoop_i18n::tr!("已下载"), size);
            let eta = t.eta_secs().map(format::duration).unwrap_or_else(|| "—".into());
            row(
                ui,
                swoop_i18n::tr!("速度"),
                swoop_i18n::trf!("{} · 剩余 {} · {} 连接", format::speed(t.speed_bps), eta, t.active_connections),
            );
        }
    }

    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &t.status {
            TaskStatus::Completed => {
                if primary_button(ui, p, swoop_i18n::tr!("打开")).clicked() {
                    actions.push(Action::Open(t.id, t.path.clone(), FileAction::Open));
                }
                if ghost_button(ui, p, swoop_i18n::tr!("打开文件夹")).clicked() {
                    actions.push(Action::Open(t.id, t.path.clone(), FileAction::Reveal));
                }
                if ghost_button(ui, p, swoop_i18n::tr!("关闭")).clicked() {
                    actions.push(Action::Close(t.id));
                }
            }
            status => {
                if ghost_button(ui, p, swoop_i18n::tr!("关闭")).clicked() {
                    actions.push(Action::Close(t.id));
                }
                if matches!(status, TaskStatus::Paused | TaskStatus::Failed(_)) {
                    if primary_button(ui, p, swoop_i18n::tr!("继续")).clicked() {
                        actions.push(Action::Resume(t.id));
                    }
                } else if status.is_active() && ghost_button(ui, p, swoop_i18n::tr!("暂停")).clicked() {
                    actions.push(Action::Pause(t.id));
                }
            }
        });
    });
}
