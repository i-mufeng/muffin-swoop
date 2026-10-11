use std::path::Path;

use egui::{Align, Align2, CornerRadius, Frame, Label, Layout, Margin, RichText, Sense, Stroke, UiBuilder, vec2};
use swoop_core::{TaskId, TaskSnapshot, TaskStatus};

use super::filekind::FileKind;
use super::{file_tile, format, icon_button, icon_button_colored, pill, primary_button};
use crate::app::SwoopApp;
use crate::fonts;
use crate::theme::{Palette, R_CARD};

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
        let mut open_add = false;

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 6, bottom: 12 }))
            .show(ui, |ui| {
                if tasks.is_empty() {
                    open_add = self.empty_state(ui);
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    for t in &tasks {
                        if let Some(a) = self.task_row(ui, t) {
                            actions.push(a);
                        }
                    }
                    ui.add_space(8.0);
                });
            });

        if open_add {
            self.add_dialog.open_with(None);
        }
        for a in actions {
            self.apply(a);
        }
    }

    fn apply(&mut self, action: RowAction) {
        match action {
            // 再点一次已选中的行收起详情面板
            RowAction::Select(id) => self.selected = if self.selected == Some(id) { None } else { Some(id) },
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

    /// 空状态；返回 true 表示用户点了"新建任务"
    fn empty_state(&self, ui: &mut egui::Ui) -> bool {
        let p = self.palette;
        let mut clicked = false;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.26);
            let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            let painter = ui.painter();
            painter.circle_filled(rect.center(), 36.0, p.surface_hover);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                egui_phosphor::fill::CLOUD_ARROW_DOWN,
                fonts::icon_fill(34.0),
                p.text_muted,
            );
            ui.add_space(14.0);
            let title = match self.filter {
                super::Filter::All => "还没有下载任务",
                super::Filter::Active => "没有进行中的任务",
                super::Filter::Completed => "还没有完成的任务",
                super::Filter::Failed => "没有失败的任务",
            };
            ui.label(RichText::new(title).size(16.0).strong().color(p.text));
            ui.add_space(4.0);
            ui.label(RichText::new("按 ⌘N 新建，或直接 ⌘V 粘贴链接").size(13.0).color(p.text_muted));
            if self.filter == super::Filter::All {
                ui.add_space(16.0);
                if primary_button(ui, &p, "新建任务").clicked() {
                    clicked = true;
                }
            }
        });
        clicked
    }

    #[allow(clippy::collapsible_match)] // 按状态分支写法更直观，不合并到 match guard
    fn task_row(&self, ui: &mut egui::Ui, t: &TaskSnapshot) -> Option<RowAction> {
        let p = self.palette;
        let selected = self.selected == Some(t.id);
        let mut action = None;
        let kind = FileKind::from_name(&t.file_name);

        // UiBuilder::sense 让整行可点击，同时行内按钮优先接收点击
        let scope = ui.scope_builder(UiBuilder::new().id_salt(t.id.0).sense(Sense::click()), |ui| {
            let resp = ui.response();
            let t_hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
            let fill = p.surface.lerp_to_gamma(p.surface_hover, if selected { 1.0 } else { t_hover });
            let stroke = Stroke::new(1.0, if selected { p.accent } else { p.border });
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(R_CARD))
                .inner_margin(Margin { left: 14, right: 12, top: 12, bottom: 12 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        file_tile(ui, &p, kind, 40.0);

                        // 中间：文件名 + 元信息，两行
                        let right_w = 236.0;
                        let name_w = (ui.available_width() - right_w).max(120.0);
                        ui.allocate_ui_with_layout(vec2(name_w, 40.0), Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.add_space(1.0);
                            let name = if t.file_name.is_empty() { "正在获取文件信息…".to_string() } else { t.file_name.clone() };
                            ui.add(Label::new(RichText::new(name).size(14.5).strong().color(p.text)).truncate());
                            ui.add(Label::new(RichText::new(meta_text(t)).size(12.0).color(p.text_muted)).truncate());
                        });

                        // 右侧：操作按钮 + 状态
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            use egui_phosphor::regular as i;
                            if icon_button_colored(ui, &p, i::TRASH, "移除任务（保留文件）", p.text_muted, p.danger).clicked() {
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
                            ui.add_space(10.0);
                            status_view(ui, &p, t);
                        });
                    });

                    ui.add_space(10.0);
                    super::progress::bar(ui, &p, t, self.progress_style);
                });
        });

        let resp = scope.response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() && action.is_none() {
            action = Some(RowAction::Select(t.id));
        }
        action
    }
}

/// 右侧状态：下载中显示速度，其他状态显示胶囊标签
fn status_view(ui: &mut egui::Ui, p: &Palette, t: &TaskSnapshot) {
    match &t.status {
        TaskStatus::Downloading => {
            ui.label(RichText::new(format::speed(t.speed_bps)).size(14.0).strong().color(p.accent));
        }
        TaskStatus::Queued => {
            pill(ui, "排队中", p.soft(p.text_muted), p.text_muted);
        }
        TaskStatus::Probing => {
            pill(ui, "连接中", p.accent_soft, p.accent);
        }
        TaskStatus::Paused => {
            pill(ui, "已暂停", p.soft(p.text_muted), p.text_muted);
        }
        TaskStatus::Completed => {
            pill(ui, "已完成", p.success_soft, p.success);
        }
        TaskStatus::Failed(reason) => {
            pill(ui, "失败", p.danger_soft, p.danger).on_hover_text(reason);
        }
    }
}

fn meta_text(t: &TaskSnapshot) -> String {
    let mut parts: Vec<String> = Vec::new();
    match (&t.status, t.total) {
        (TaskStatus::Completed, Some(total)) => {
            parts.push(format::bytes(total));
            if let Some(dir) = t.path.parent() {
                parts.push(dir.display().to_string());
            }
        }
        (TaskStatus::Probing, _) => parts.push("正在获取文件信息".into()),
        (TaskStatus::Queued, Some(total)) => {
            parts.push(format::bytes(total));
            parts.push("等待开始".into());
        }
        (TaskStatus::Failed(reason), total) => {
            if let Some(total) = total {
                parts.push(format!("{} / {}", format::bytes(t.downloaded), format::bytes(total)));
            }
            parts.push(reason.clone());
        }
        (_, Some(total)) => {
            parts.push(format!("{} / {}", format::bytes(t.downloaded), format::bytes(total)));
            if let Some(pr) = t.progress() {
                parts.push(format!("{:.0}%", pr * 100.0));
            }
            if t.status == TaskStatus::Downloading && t.active_connections > 0 {
                parts.push(format!("{} 连接", t.active_connections));
            }
            if let Some(eta) = t.eta_secs() {
                parts.push(format!("剩余 {}", format::duration(eta)));
            }
        }
        (_, None) => {
            parts.push(format::bytes(t.downloaded));
            parts.push("大小未知".into());
        }
    }
    if parts.is_empty() {
        return t.url.clone();
    }
    parts.join("  ·  ")
}

pub fn reveal_in_file_manager(path: &Path) {
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
