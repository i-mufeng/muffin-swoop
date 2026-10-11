use std::path::Path;

use egui::{Align, Align2, CornerRadius, Frame, Label, Layout, Margin, RichText, Sense, Stroke, UiBuilder, vec2};
use swoop_core::{TaskId, TaskSnapshot, TaskStatus};

use super::filekind::FileKind;
use super::{file_tile, format, icon_button, icon_button_colored, pill, primary_button};
use crate::app::SwoopApp;
use crate::fonts;
use crate::task_transfer::{BulkAction, ListPreferences, TransferEvent, urls_text};
use crate::theme::{Palette, R_CARD};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowAction {
    Toggle(TaskId, bool),
    Select(TaskId),
    Pause(TaskId),
    Resume(TaskId),
    Remove(TaskId),
    Reveal(TaskId),
    Drag(TaskId),
    Move(TaskId, swoop_core::QueueId),
    Unqueue(TaskId),
}

impl SwoopApp {
    pub fn task_list(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let mut tasks = self.visible_tasks();
        self.task_transfer.sort.sort(&mut tasks);
        self.task_transfer.retain_visible(&tasks);
        let mut actions = Vec::new();
        let mut open_add = false;

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 6, bottom: 12 }))
            .show(ui, |ui| {
                if tasks.is_empty() {
                    open_add = self.empty_state(ui);
                    return;
                }
                self.selection_toolbar(ui, &tasks);
                ui.add_space(8.0);
                if self.task_transfer.layout.list.mode == crate::list_layout::ListMode::Table {
                    actions.extend(self.task_table(ui, &tasks));
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = if self.task_transfer.view.compact { 4.0 } else { 8.0 };
                        for t in &tasks {
                            if let Some(a) = self.task_row(ui, t) {
                                actions.push(a);
                            }
                        }
                        ui.add_space(8.0);
                    });
                }
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
            RowAction::Toggle(id, selected) => {
                if selected {
                    self.task_transfer.selected.insert(id);
                } else {
                    self.task_transfer.selected.remove(&id);
                }
            }
            RowAction::Move(id, queue) => {
                if let Err(error) = self.engine.move_to_queue(id, queue) {
                    self.toasts.push_error(swoop_i18n::tr!("移动队列失败"), error.to_string());
                }
            }
            RowAction::Unqueue(id) => {
                if let Err(error) = self.engine.remove_from_queue(id) {
                    self.toasts.push_error(swoop_i18n::tr!("移出队列失败"), error.to_string());
                }
            }
            // 再点一次已选中的行收起详情面板
            RowAction::Select(id) => self.selected = if self.selected == Some(id) { None } else { Some(id) },
            RowAction::Pause(id) => match self.engine.pause(id) {
                Ok(()) => {
                    self.task_transfer.selected.remove(&id);
                }
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("暂停失败"), error.to_string()),
            },
            RowAction::Resume(id) => match self.engine.resume(id) {
                Ok(()) => {
                    self.task_transfer.selected.remove(&id);
                }
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("继续失败"), error.to_string()),
            },
            RowAction::Remove(id) => match self.engine.remove(id, false) {
                Ok(()) => {
                    self.task_transfer.selected.remove(&id);
                    if self.selected == Some(id) {
                        self.selected = None;
                    }
                }
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("移除失败"), error.to_string()),
            },
            RowAction::Drag(id) => {
                let chosen = if self.task_transfer.selected.contains(&id) {
                    self.task_transfer
                        .selected
                        .iter()
                        .filter_map(|id| self.engine.snapshot_one(*id))
                        .collect::<Vec<_>>()
                } else {
                    self.engine.snapshot_one(id).into_iter().collect()
                };
                if let Err(error) = self.native_drag.queue_files(&chosen) {
                    self.toasts.push_error(swoop_i18n::tr!("无法拖出"), error);
                }
            }
            RowAction::Reveal(id) => {
                if let Some(t) = self.engine.snapshot_one(id) {
                    reveal_in_file_manager(&t.path);
                }
            }
        }
    }

    fn selection_toolbar(&mut self, ui: &mut egui::Ui, tasks: &[TaskSnapshot]) {
        let p = self.palette;
        let busy = self.task_transfer.busy();
        let chosen = self.task_transfer.chosen(tasks);
        let mut all = chosen.len() == tasks.len();
        let mut action = None;
        let mut copy = false;
        let mut export = false;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if ui.checkbox(&mut all, swoop_i18n::tr!("全选当前结果")).changed() {
                    self.task_transfer.select_visible(tasks, all);
                }
            });
            if busy {
                ui.spinner();
            }
            if !chosen.is_empty() {
                ui.label(RichText::new(swoop_i18n::trf!("已选 {}", chosen.len())).color(p.text_muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        use egui_phosphor::regular as i;
                        if icon_button(ui, &p, i::X, swoop_i18n::tr!("取消选择")).clicked() {
                            self.task_transfer.selected.clear();
                        }
                        ui.menu_button(super::icon_text(i::DOTS_THREE, 18.0, p.text_muted), |ui| {
                            if ui.button(swoop_i18n::tr!("复制所选链接")).clicked() {
                                copy = true;
                                ui.close();
                            }
                            if ui.button(swoop_i18n::tr!("导出所选链接…")).clicked() {
                                export = true;
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .button(RichText::new(swoop_i18n::tr!("移除所选记录（保留文件）")).color(p.danger))
                                .clicked()
                            {
                                action = Some(BulkAction::Remove);
                                ui.close();
                            }
                        })
                        .response
                        .on_hover_text(swoop_i18n::tr!("所选任务操作"));
                        ui.add_enabled_ui(chosen.iter().any(|task| BulkAction::Resume.accepts(&task.status)), |ui| {
                            if icon_button(ui, &p, i::PLAY, swoop_i18n::tr!("继续所选任务")).clicked() {
                                action = Some(BulkAction::Resume);
                            }
                        });
                        ui.add_enabled_ui(chosen.iter().any(|task| BulkAction::Pause.accepts(&task.status)), |ui| {
                            if icon_button(ui, &p, i::PAUSE, swoop_i18n::tr!("暂停所选任务")).clicked() {
                                action = Some(BulkAction::Pause);
                            }
                        });
                    });
                });
            }
        });
        if copy {
            self.copy_link(ui.ctx(), urls_text(&chosen));
            for task in &chosen {
                self.task_transfer.selected.remove(&task.id);
            }
        }
        let result = if let Some(action) = action {
            self.task_transfer.start_bulk(
                ui.ctx().clone(),
                self.engine.clone(),
                action,
                chosen.iter().map(|task| task.id).collect(),
            )
        } else if export {
            self.task_transfer.start_export(
                ui.ctx().clone(),
                chosen,
                self.tasks.iter().map(|task| task.path.clone()).collect(),
            )
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.toasts.push_error(swoop_i18n::tr!("操作未启动"), error);
        }
    }

    pub fn poll_task_transfers(&mut self, _ctx: &egui::Context) {
        let Some(event) = self.task_transfer.poll() else {
            return;
        };
        match event {
            TransferEvent::Bulk(action, report) => {
                for id in &report.succeeded {
                    self.task_transfer.selected.remove(id);
                    if action == BulkAction::Remove && self.selected == Some(*id) {
                        self.selected = None;
                    }
                }
                let detail = swoop_i18n::trf!("成功 {} 项，跳过 {} 项", report.succeeded.len(), report.skipped);
                if report.errors.is_empty() {
                    self.toasts.push_info(swoop_i18n::trf!("批量{}完成", action.label()), detail);
                } else {
                    self.toasts.push_error(
                        swoop_i18n::trf!("批量{}：{} 项失败", action.label(), report.errors.len()),
                        format!("{detail}\n{}", report.errors.iter().take(3).cloned().collect::<Vec<_>>().join("\n")),
                    );
                }
            }
            TransferEvent::Exported { ids, path, count } => {
                for id in ids {
                    self.task_transfer.selected.remove(&id);
                }
                self.toasts.push_info(swoop_i18n::trf!("已导出 {count} 条链接"), path.display().to_string());
            }
            TransferEvent::Cancelled => {}
            TransferEvent::Error(error) => self.toasts.push_error(swoop_i18n::tr!("批量操作失败"), error),
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
                super::Filter::All => swoop_i18n::tr!("还没有下载任务"),
                super::Filter::Active => swoop_i18n::tr!("没有进行中的任务"),
                super::Filter::Completed => swoop_i18n::tr!("还没有完成的任务"),
                super::Filter::Failed => swoop_i18n::tr!("没有失败的任务"),
            };
            ui.label(RichText::new(title).size(16.0).strong().color(p.text));
            ui.add_space(4.0);
            ui.label(RichText::new(swoop_i18n::tr!("按 ⌘N 新建，或直接 ⌘V 粘贴链接")).size(13.0).color(p.text_muted));
            if self.filter == super::Filter::All {
                ui.add_space(16.0);
                if primary_button(ui, &p, swoop_i18n::tr!("新建任务")).clicked() {
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
        let view = self.task_transfer.view;
        let mut action = None;
        let kind = FileKind::from_name(&t.file_name);

        // UiBuilder::sense 让整行可点击，同时行内按钮优先接收点击
        let scope = ui.scope_builder(UiBuilder::new().id_salt(t.id.0).sense(Sense::click_and_drag()), |ui| {
            let resp = ui.response();
            let t_hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
            let fill = p.surface.lerp_to_gamma(p.surface_hover, if selected { 1.0 } else { t_hover });
            let stroke = Stroke::new(1.0, if selected { p.accent } else { p.border });
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(R_CARD))
                .inner_margin(Margin {
                    left: 14,
                    right: 12,
                    top: if view.compact { 6 } else { 12 },
                    bottom: if view.compact { 6 } else { 12 },
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let mut checked = self.task_transfer.selected.contains(&t.id);
                        if ui
                            .add_enabled(!self.task_transfer.busy(), egui::Checkbox::without_text(&mut checked))
                            .on_hover_text(swoop_i18n::tr!("选择此任务"))
                            .changed()
                        {
                            action = Some(RowAction::Toggle(t.id, checked));
                        }
                        file_tile(ui, &p, kind, if view.compact { 32.0 } else { 40.0 });

                        // 中间：文件名 + 元信息，两行
                        let right_w = 236.0;
                        let name_w = (ui.available_width() - right_w).max(120.0);
                        ui.allocate_ui_with_layout(vec2(name_w, 40.0), Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.add_space(1.0);
                            let name = if t.file_name.is_empty() {
                                swoop_i18n::tr!("正在获取文件信息…").to_string()
                            } else {
                                t.file_name.clone()
                            };
                            ui.add(Label::new(RichText::new(name).size(14.5).strong().color(p.text)).truncate());
                            ui.add(
                                Label::new(RichText::new(meta_text(t, view)).size(12.0).color(p.text_muted)).truncate(),
                            );
                        });

                        // 右侧：操作按钮 + 状态
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            use egui_phosphor::regular as i;
                            if icon_button_colored(
                                ui,
                                &p,
                                i::TRASH,
                                swoop_i18n::tr!("移除任务（保留文件）"),
                                p.text_muted,
                                p.danger,
                            )
                            .clicked()
                            {
                                action = Some(RowAction::Remove(t.id));
                            }
                            match &t.status {
                                s if s.is_active() => {
                                    if icon_button(ui, &p, i::PAUSE, swoop_i18n::tr!("暂停")).clicked() {
                                        action = Some(RowAction::Pause(t.id));
                                    }
                                }
                                TaskStatus::Paused | TaskStatus::Failed(_) => {
                                    if icon_button(ui, &p, i::PLAY, swoop_i18n::tr!("继续")).clicked() {
                                        action = Some(RowAction::Resume(t.id));
                                    }
                                }
                                TaskStatus::Completed => {
                                    if icon_button(ui, &p, i::FOLDER_OPEN, swoop_i18n::tr!("打开所在目录")).clicked()
                                    {
                                        action = Some(RowAction::Reveal(t.id));
                                    }
                                }
                                _ => {}
                            }
                            ui.add_space(10.0);
                            status_view(ui, &p, t, view.speed);
                        });
                    });

                    if view.progress_bar {
                        ui.add_space(if view.compact { 5.0 } else { 10.0 });
                        super::progress::bar(ui, &p, t, self.progress_style);
                    }
                });
        });

        let resp = scope.response.on_hover_cursor(egui::CursorIcon::PointingHand);
        resp.context_menu(|ui| row_context_menu(ui, &p, t, &self.queues, &mut action));
        if resp.drag_started()
            && action.is_none()
            && t.status == TaskStatus::Completed
            && self.native_drag.config.allow_drag_out
        {
            action = Some(RowAction::Drag(t.id));
        }
        if resp.clicked() && action.is_none() {
            action = Some(RowAction::Select(t.id));
        }
        action
    }
}

pub(super) fn row_context_menu(
    ui: &mut egui::Ui,
    p: &Palette,
    t: &TaskSnapshot,
    queues: &[swoop_core::QueueSnapshot],
    action: &mut Option<RowAction>,
) {
    if t.status.is_active() && ui.button(swoop_i18n::tr!("暂停")).clicked() {
        *action = Some(RowAction::Pause(t.id));
        ui.close();
    }
    if matches!(t.status, TaskStatus::Paused | TaskStatus::Failed(_)) && ui.button(swoop_i18n::tr!("继续")).clicked()
    {
        *action = Some(RowAction::Resume(t.id));
        ui.close();
    }
    if t.status == TaskStatus::Completed && ui.button(swoop_i18n::tr!("打开所在目录")).clicked() {
        *action = Some(RowAction::Reveal(t.id));
        ui.close();
    }
    ui.menu_button(swoop_i18n::tr!("移动到队列"), |ui| {
        for queue in queues {
            if ui.button(super::queue_name(queue)).clicked() {
                *action = Some(RowAction::Move(t.id, queue.id));
                ui.close();
            }
        }
    });
    if ui.button(swoop_i18n::tr!("移出队列并暂停")).clicked() {
        *action = Some(RowAction::Unqueue(t.id));
        ui.close();
    }
    ui.separator();
    if ui.button(RichText::new(swoop_i18n::tr!("移除任务（保留文件）")).color(p.danger)).clicked() {
        *action = Some(RowAction::Remove(t.id));
        ui.close();
    }
}

/// 右侧状态：下载中显示速度，其他状态显示胶囊标签
pub(super) fn status_view(ui: &mut egui::Ui, p: &Palette, t: &TaskSnapshot, show_speed: bool) {
    match &t.status {
        TaskStatus::Downloading => {
            if show_speed {
                ui.label(RichText::new(format::speed(t.speed_bps)).size(14.0).strong().color(p.accent));
            } else {
                pill(ui, swoop_i18n::tr!("下载中"), p.accent_soft, p.accent);
            }
        }
        TaskStatus::Queued => {
            pill(ui, swoop_i18n::tr!("排队中"), p.soft(p.text_muted), p.text_muted);
        }
        TaskStatus::Probing => {
            pill(ui, swoop_i18n::tr!("连接中"), p.accent_soft, p.accent);
        }
        TaskStatus::FileConflict => {
            pill(ui, swoop_i18n::tr!("等待确认"), p.soft(p.tint_amber), p.tint_amber);
        }
        TaskStatus::Paused => {
            pill(ui, swoop_i18n::tr!("已暂停"), p.soft(p.text_muted), p.text_muted);
        }
        TaskStatus::Completed => {
            pill(ui, swoop_i18n::tr!("已完成"), p.success_soft, p.success);
        }
        TaskStatus::Failed(reason) => {
            pill(ui, swoop_i18n::tr!("失败"), p.danger_soft, p.danger).on_hover_text(reason);
        }
    }
}

fn meta_text(t: &TaskSnapshot, view: ListPreferences) -> String {
    let mut parts = Vec::new();
    if view.size {
        match (t.total, &t.status) {
            (Some(total), TaskStatus::Completed | TaskStatus::Queued) => parts.push(format::bytes(total)),
            (Some(total), _) => parts.push(format!("{} / {}", format::bytes(t.downloaded), format::bytes(total))),
            (None, _) => parts.push(swoop_i18n::trf!("{} · 大小未知", format::bytes(t.downloaded))),
        }
    }
    match &t.status {
        TaskStatus::Completed if view.directory => {
            if let Some(dir) = t.path.parent() {
                parts.push(dir.display().to_string());
            }
        }
        TaskStatus::Probing => parts.push(swoop_i18n::tr!("正在获取文件信息").into()),
        TaskStatus::Queued => parts.push(swoop_i18n::tr!("等待开始").into()),
        TaskStatus::FileConflict => parts.push(swoop_i18n::tr!("同名文件等待处理").into()),
        TaskStatus::Failed(reason) => parts.push(reason.clone()),
        _ => {
            if view.percent
                && t.status != TaskStatus::Completed
                && let Some(progress) = t.progress()
            {
                parts.push(format!("{:.0}%", progress * 100.0));
            }
            if view.connections && t.status == TaskStatus::Downloading && t.active_connections > 0 {
                parts.push(swoop_i18n::trf!("{} 连接", t.active_connections));
            }
            if view.remaining_time
                && let Some(eta) = t.eta_secs()
            {
                parts.push(swoop_i18n::trf!("剩余 {}", format::duration(eta)));
            }
        }
    }
    if view.host
        && let Ok(url) = url::Url::parse(&t.url)
        && let Some(host) = url.host_str()
    {
        parts.push(host.to_string());
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
