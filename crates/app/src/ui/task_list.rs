use std::path::Path;

use egui::{Align, Color32, CornerRadius, Frame, Label, Layout, Margin, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2};
use swoop_core::{TaskId, TaskSnapshot, TaskStatus};

use super::{format, icon_button};
use crate::app::SwoopApp;
use crate::theme::Palette;

enum RowAction {
    Select(TaskId),
    Pause(TaskId),
    Resume(TaskId),
    Remove(TaskId),
    Reveal(TaskId),
}

impl SwoopApp {
    pub fn task_list(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let tasks = self.visible_tasks();
        let mut actions = Vec::new();

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 8, bottom: 12 }))
            .show(ui, |ui| {
                if tasks.is_empty() {
                    self.empty_state(ui);
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    for t in &tasks {
                        if let Some(a) = self.task_row(ui, t) {
                            actions.push(a);
                        }
                    }
                    ui.add_space(8.0);
                });
            });

        for a in actions {
            self.apply(a);
        }
    }

    fn apply(&mut self, action: RowAction) {
        match action {
            RowAction::Select(id) => self.selected = Some(id),
            RowAction::Pause(id) => {
                let _ = self.engine.pause(id);
            }
            RowAction::Resume(id) => {
                let _ = self.engine.resume(id);
            }
            RowAction::Remove(id) => {
                let _ = self.engine.remove(id, false);
                if self.selected == Some(id) {
                    self.selected = None;
                }
            }
            RowAction::Reveal(id) => {
                if let Some(t) = self.engine.snapshot_one(id) {
                    reveal_in_file_manager(&t.path);
                }
            }
        }
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.28);
            ui.label(RichText::new(egui_phosphor::regular::CLOUD_ARROW_DOWN).size(60.0).color(p.text_muted));
            ui.add_space(8.0);
            let title = match self.filter {
                super::Filter::All => "还没有下载任务",
                super::Filter::Active => "没有进行中的任务",
                super::Filter::Completed => "还没有完成的任务",
                super::Filter::Failed => "没有失败的任务",
            };
            ui.label(RichText::new(title).size(16.0).strong().color(p.text));
            ui.add_space(4.0);
            ui.label(RichText::new("按 ⌘N 新建，或直接 ⌘V 粘贴链接").size(13.0).color(p.text_muted));
        });
    }

    #[allow(clippy::collapsible_match)] // 按状态分支写法更直观，不合并到 match guard
    fn task_row(&self, ui: &mut egui::Ui, t: &TaskSnapshot) -> Option<RowAction> {
        let p = self.palette;
        let selected = self.selected == Some(t.id);
        let mut action = None;

        // UiBuilder::sense 让整行可点击，同时行内按钮优先接收点击
        let scope = ui.scope_builder(UiBuilder::new().id_salt(t.id.0).sense(Sense::click()), |ui| {
            let hovered = ui.response().hovered();
            let fill = if hovered || selected { p.surface_hover } else { p.surface };
            let stroke = Stroke::new(1.0, if selected { p.accent } else { p.border });
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin { left: 16, right: 12, top: 12, bottom: 12 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    // 第一行：状态图标 + 文件名 ……… 速度/状态 + 操作按钮
                    ui.horizontal(|ui| {
                        let (icon, color) = status_icon(&t.status, &p);
                        ui.label(RichText::new(icon).size(18.0).color(color));
                        ui.add_space(2.0);

                        let name_w = (ui.available_width() - 270.0).max(80.0);
                        ui.allocate_ui_with_layout(vec2(name_w, 22.0), Layout::left_to_right(Align::Center), |ui| {
                            let name = if t.file_name.is_empty() { "正在获取文件信息…".to_string() } else { t.file_name.clone() };
                            ui.add(Label::new(RichText::new(name).size(14.5).strong().color(p.text)).truncate());
                        });

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            use egui_phosphor::regular as i;
                            if icon_button(ui, &p, i::TRASH, "移除任务（保留文件）").clicked() {
                                action = Some(RowAction::Remove(t.id));
                            }
                            match &t.status {
                                s if s.is_active() => {
                                    if icon_button(ui, &p, i::PAUSE, "暂停").clicked() {
                                        action = Some(RowAction::Pause(t.id));
                                    }
                                }
                                TaskStatus::Paused | TaskStatus::Failed(_) => {
                                    if icon_button(ui, &p, i::PLAY, "继续").clicked() {
                                        action = Some(RowAction::Resume(t.id));
                                    }
                                }
                                TaskStatus::Completed => {
                                    if icon_button(ui, &p, i::FOLDER_OPEN, "在访达中显示").clicked() {
                                        action = Some(RowAction::Reveal(t.id));
                                    }
                                }
                                _ => {}
                            }
                            ui.add_space(6.0);
                            let (text, color) = status_text(t, &p);
                            let label = ui.label(RichText::new(text).size(13.0).strong().color(color));
                            if let TaskStatus::Failed(reason) = &t.status {
                                label.on_hover_text(reason);
                            }
                        });
                    });

                    ui.add_space(8.0);
                    progress_bar(ui, &p, t);
                    ui.add_space(6.0);

                    // 第三行：大小 / 已下载 ……… 剩余时间 · 连接数
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(meta_text(t)).size(12.0).color(p.text_muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if let Some(eta) = t.eta_secs() {
                                ui.label(RichText::new(format!("剩余 {}", format::duration(eta))).size(12.0).color(p.text_muted));
                            }
                            if t.status == TaskStatus::Downloading && t.active_connections > 0 {
                                ui.label(RichText::new(format!("{} 连接 ·", t.active_connections)).size(12.0).color(p.text_muted));
                            }
                        });
                    });
                });
        });

        if scope.response.clicked() && action.is_none() {
            action = Some(RowAction::Select(t.id));
        }
        action
    }
}

fn status_icon(status: &TaskStatus, p: &Palette) -> (&'static str, Color32) {
    use egui_phosphor::regular as i;
    match status {
        TaskStatus::Queued | TaskStatus::Probing => (i::CLOCK, p.text_muted),
        TaskStatus::Downloading => (i::ARROW_CIRCLE_DOWN, p.accent),
        TaskStatus::Paused => (i::PAUSE_CIRCLE, p.text_muted),
        TaskStatus::Completed => (i::CHECK_CIRCLE, p.success),
        TaskStatus::Failed(_) => (i::WARNING_CIRCLE, p.danger),
    }
}

fn status_text(t: &TaskSnapshot, p: &Palette) -> (String, Color32) {
    match &t.status {
        TaskStatus::Queued => ("排队中".into(), p.text_muted),
        TaskStatus::Probing => ("连接中…".into(), p.text_muted),
        TaskStatus::Downloading => (format::speed(t.speed_bps), p.accent),
        TaskStatus::Paused => ("已暂停".into(), p.text_muted),
        TaskStatus::Completed => ("已完成".into(), p.success),
        TaskStatus::Failed(_) => ("失败".into(), p.danger),
    }
}

fn meta_text(t: &TaskSnapshot) -> String {
    match (&t.status, t.total) {
        (TaskStatus::Completed, Some(total)) => {
            format!("{} · {}", format::bytes(total), t.path.parent().map(|d| d.display().to_string()).unwrap_or_default())
        }
        (_, Some(total)) => format!("{} / {}", format::bytes(t.downloaded), format::bytes(total)),
        (_, None) if t.downloaded > 0 => format!("{} / 未知大小", format::bytes(t.downloaded)),
        _ => t.url.clone(),
    }
}

/// 进度条 + 分段图：每个区段按各自进度填充，区段边界用底色细线分隔。
fn progress_bar(ui: &mut egui::Ui, p: &Palette, t: &TaskSnapshot) {
    let h = 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let r = CornerRadius::same(3);
    painter.rect_filled(rect, r, p.track);

    let color = match &t.status {
        TaskStatus::Queued | TaskStatus::Probing | TaskStatus::Downloading => p.accent,
        TaskStatus::Paused => p.text_muted,
        TaskStatus::Completed => p.success,
        TaskStatus::Failed(_) => p.danger,
    };

    if t.status == TaskStatus::Completed {
        painter.rect_filled(rect, r, color);
        return;
    }

    match t.total {
        Some(total) if total > 0 => {
            let scale = rect.width() / total as f32;
            for seg in &t.segments {
                if seg.pos == 0 {
                    continue;
                }
                let x0 = rect.left() + seg.start as f32 * scale;
                let x1 = (rect.left() + (seg.start + seg.pos) as f32 * scale).min(rect.right());
                if x1 - x0 < 0.5 {
                    continue;
                }
                painter.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())), r, color);
            }
            if t.segments.len() > 1 && t.status.is_active() {
                for seg in &t.segments {
                    let x = rect.left() + seg.start as f32 * scale;
                    if x > rect.left() + 1.0 && x < rect.right() - 1.0 {
                        painter.vline(x, rect.y_range(), Stroke::new(1.0, p.bg));
                    }
                }
            }
        }
        _ if t.status.is_active() => {
            // 大小未知：来回滑动的指示块
            let time = ui.input(|i| i.time) as f32;
            let frac = (time * 0.5) % 1.0;
            let bw = rect.width() * 0.22;
            let x0 = rect.left() + (rect.width() + bw) * frac - bw;
            let seg = Rect::from_min_max(pos2(x0.max(rect.left()), rect.top()), pos2((x0 + bw).min(rect.right()), rect.bottom()));
            if seg.width() > 0.0 {
                painter.rect_filled(seg, r, color);
            }
            ui.ctx().request_repaint();
        }
        _ => {}
    }
}

fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}
