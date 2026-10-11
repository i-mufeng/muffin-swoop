//! 表格视图共用卡片的任务、选中状态和操作。
use std::collections::HashSet;

use egui::{Align, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use swoop_core::{QueueSnapshot, TaskId, TaskSnapshot};

use super::task_list::{RowAction, row_context_menu, status_view};
use crate::app::SwoopApp;
use crate::list_layout::{ColumnId, ListLayout};
use crate::task_transfer::{SortField, SortOrder};
use crate::theme::Palette;

struct TableView<'a> {
    tasks: &'a [TaskSnapshot],
    queues: &'a [QueueSnapshot],
    checked: &'a HashSet<TaskId>,
    selected: Option<TaskId>,
    busy: bool,
    compact: bool,
    layout: &'a ListLayout,
    sort: SortOrder,
    palette: Palette,
    reset: bool,
    drag_out: bool,
}
#[derive(Default)]
struct TableOutput {
    actions: Vec<RowAction>,
    sort: Option<SortOrder>,
    widths: Vec<(ColumnId, f32)>,
    #[cfg(test)]
    cells: Vec<(TaskId, ColumnId, egui::Rect)>,
    #[cfg(test)]
    checkboxes: Vec<(TaskId, egui::Rect)>,
}

impl SwoopApp {
    pub(super) fn task_table(&mut self, ui: &mut egui::Ui, tasks: &[TaskSnapshot]) -> Vec<RowAction> {
        let state = &self.task_transfer;
        let output = show_table(
            ui,
            TableView {
                tasks,
                queues: &self.queues,
                checked: &state.selected,
                selected: self.selected,
                busy: state.busy(),
                compact: state.view.compact,
                layout: &state.layout.list,
                sort: state.sort,
                palette: self.palette,
                drag_out: self.native_drag.config.allow_drag_out,
                reset: state.table_revision != Some(state.layout.revision),
            },
        );
        self.task_transfer.table_revision = Some(self.task_transfer.layout.revision);
        if let Some(sort) = output.sort
            && let Err(error) = self.task_transfer.set_sort(&self.store, sort)
        {
            self.toasts.push_error(swoop_i18n::tr!("排序未保存"), error);
        }
        // TableBody reports the latest widths on the frame after a drag; commit once the pointer is released.
        let released = ui.input(|input| input.pointer.any_released());
        if released {
            ui.ctx().request_repaint();
        }
        if !released && !ui.input(|input| input.pointer.any_down()) {
            let next = self.task_transfer.layout.list.with_widths(&output.widths);
            if next != self.task_transfer.layout.list {
                let toolbar = self.task_transfer.layout.toolbar.clone();
                if let Err(error) = self.task_transfer.layout.save(&self.store, next, toolbar) {
                    self.toasts.push_error(swoop_i18n::tr!("列宽未保存"), error);
                }
            }
        }
        output.actions
    }
}

fn sort_field(column: ColumnId) -> Option<SortField> {
    match column {
        ColumnId::Name => Some(SortField::Name),
        ColumnId::Status => Some(SortField::Status),
        ColumnId::Size => Some(SortField::Size),
        ColumnId::Created => Some(SortField::Created),
        _ => None,
    }
}

fn show_table(ui: &mut egui::Ui, view: TableView<'_>) -> TableOutput {
    let mut output = TableOutput::default();
    let columns: Vec<_> = view.layout.visible().collect();
    let p = view.palette;
    egui::ScrollArea::horizontal().id_salt("task_table_horizontal").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(
            columns.iter().map(|c| c.width).sum::<f32>() + ui.spacing().item_spacing.x * columns.len() as f32 + 20.0,
        );
        let mut table = TableBuilder::new(ui)
            .id_salt("task_table")
            .striped(true)
            .resizable(true)
            .sense(Sense::click_and_drag())
            .cell_layout(Layout::left_to_right(Align::Center));
        for column in &columns {
            table = table.column(Column::initial(column.width).range(column.id.min_width()..=800.0).clip(true));
        }
        if view.reset {
            table.reset();
        }
        table
            .header(28.0, |mut header| {
                for column in &columns {
                    header.col(|ui| {
                        if let Some(field) = sort_field(column.id) {
                            let active = field == view.sort.field;
                            let suffix = if active { if view.sort.descending { " ↓" } else { " ↑" } } else { "" };
                            if ui
                                .selectable_label(
                                    active,
                                    RichText::new(format!("{}{suffix}", column.id.label())).color(p.text_muted),
                                )
                                .clicked()
                            {
                                output.sort = Some(SortOrder { field, descending: active && !view.sort.descending });
                            }
                        } else {
                            ui.label(RichText::new(column.id.label()).color(p.text_muted));
                        }
                    });
                }
            })
            .body(|body| {
                output.widths = columns.iter().zip(body.widths()).map(|(column, width)| (column.id, *width)).collect();
                body.rows(if view.compact { 30.0 } else { 40.0 }, view.tasks.len(), |mut row| {
                    let task = &view.tasks[row.index()];
                    let mut action = None;
                    row.set_selected(view.checked.contains(&task.id) || view.selected == Some(task.id));
                    for column in &columns {
                        let (_, response) = row.col(|ui| match column.id {
                            ColumnId::Name => {
                                let mut checked = view.checked.contains(&task.id);
                                let checkbox = ui
                                    .add_enabled(!view.busy, egui::Checkbox::without_text(&mut checked))
                                    .on_hover_text(swoop_i18n::tr!("选择此任务"));
                                #[cfg(test)]
                                output.checkboxes.push((task.id, checkbox.rect));
                                if checkbox.changed() {
                                    action = Some(RowAction::Toggle(task.id, checked));
                                }
                                let name = if task.file_name.is_empty() {
                                    swoop_i18n::tr!("正在获取文件信息…")
                                } else {
                                    &task.file_name
                                };
                                ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate())
                                    .on_hover_text(name);
                            }
                            ColumnId::Status => status_view(ui, &p, task, false),
                            _ => {
                                let text = cell_text(column.id, task);
                                ui.add(egui::Label::new(RichText::new(&text).color(p.text_muted)).truncate())
                                    .on_hover_text(text);
                            }
                        });
                        #[cfg(test)]
                        output.cells.push((task.id, column.id, response.rect));
                        #[cfg(not(test))]
                        let _ = response;
                    }
                    let response = row.response();
                    response.context_menu(|ui| row_context_menu(ui, &p, task, view.queues, &mut action));
                    if response.drag_started()
                        && action.is_none()
                        && view.drag_out
                        && task.status == swoop_core::TaskStatus::Completed
                    {
                        action = Some(RowAction::Drag(task.id));
                    }
                    if response.clicked() && action.is_none() {
                        action = Some(RowAction::Select(task.id));
                    }
                    if let Some(action) = action {
                        output.actions.push(action);
                    }
                });
            });
    });
    output
}

fn cell_text(column: ColumnId, task: &TaskSnapshot) -> String {
    use super::format;
    match column {
        ColumnId::Size => task.total.map(format::bytes).unwrap_or_else(|| "—".into()),
        ColumnId::Progress => task.progress().map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "—".into()),
        ColumnId::Speed => {
            if task.status == swoop_core::TaskStatus::Downloading {
                format::speed(task.speed_bps)
            } else {
                "—".into()
            }
        }
        ColumnId::Remaining => task.eta_secs().map(format::duration).unwrap_or_else(|| "—".into()),
        ColumnId::Connections => task.active_connections.to_string(),
        ColumnId::Host => url::Url::parse(&task.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| "—".into()),
        ColumnId::Directory => task.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        ColumnId::Created => chrono::DateTime::<chrono::Local>::from(task.created_at).format("%m-%d %H:%M").to_string(),
        ColumnId::Name | ColumnId::Status => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoop_core::TaskStatus;
    fn task(id: u64) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId(id),
            url: "https://example.test/a?token=private".into(),
            file_name: format!("task-{id}"),
            path: "/tmp/task.bin".into(),
            total: Some(100),
            downloaded: 20,
            speed_bps: 0.0,
            status: TaskStatus::FileConflict,
            resumable: false,
            segments: vec![],
            active_connections: 0,
            created_at: std::time::SystemTime::UNIX_EPOCH,
            completed_at: None,
        }
    }
    fn frame(
        ctx: &egui::Context,
        tasks: &[TaskSnapshot],
        layout: &ListLayout,
        input: egui::RawInput,
        reset: bool,
    ) -> TableOutput {
        let mut output = None;
        let mut result = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                output = Some(show_table(
                    ui,
                    TableView {
                        tasks,
                        queues: &[],
                        checked: &HashSet::new(),
                        selected: None,
                        drag_out: true,
                        busy: false,
                        compact: false,
                        layout,
                        sort: SortOrder::default(),
                        palette: Palette::dark(),
                        reset,
                    },
                ));
            });
        });
        result.textures_delta.clear();
        output.unwrap()
    }
    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 400.0))),
            events,
            ..Default::default()
        }
    }
    fn click(ctx: &egui::Context, tasks: &[TaskSnapshot], layout: &ListLayout, pos: egui::Pos2) -> TableOutput {
        frame(ctx, tasks, layout, input(vec![egui::Event::PointerMoved(pos)]), false);
        frame(
            ctx,
            tasks,
            layout,
            input(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]),
            false,
        );
        frame(
            ctx,
            tasks,
            layout,
            input(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]),
            false,
        )
    }
    #[test]
    fn table_pointer_selects_correct_task_and_checkbox_does_not_open_details() {
        let ctx = egui::Context::default();
        let tasks = [task(9), task(4)];
        let layout = ListLayout::default();
        frame(&ctx, &tasks, &layout, input(vec![]), true);
        let first = frame(&ctx, &tasks, &layout, input(vec![]), false);
        let checkbox = first.checkboxes.iter().find(|(id, _)| *id == TaskId(4)).unwrap().1.center();
        assert!(checkbox.is_finite(), "checkbox positions: {:?}", first.checkboxes);
        assert_eq!(click(&ctx, &tasks, &layout, checkbox).actions, [RowAction::Toggle(TaskId(4), true)]);
        let cell = first.cells.iter().find(|(id, col, _)| *id == TaskId(9) && *col == ColumnId::Name).unwrap().2;
        assert!(cell.is_finite(), "cell positions: {:?}", first.cells);
        assert_eq!(
            click(&ctx, &tasks, &layout, egui::pos2(cell.right() - 12.0, cell.center().y)).actions,
            [RowAction::Select(TaskId(9))]
        );
        assert_eq!(cell_text(ColumnId::Host, &tasks[0]), "example.test");
    }
    #[test]
    fn rendered_widths_match_columns_when_same_count_reorders() {
        let ctx = egui::Context::default();
        let tasks = [task(1)];
        let mut layout = ListLayout::default();
        frame(&ctx, &tasks, &layout, input(vec![]), true);
        layout.columns[0].width = 333.0;
        layout.columns[2].width = 144.0;
        layout.columns.swap(0, 2);
        let output = frame(&ctx, &tasks, &layout, input(vec![]), true);
        assert_eq!(output.widths[0], (ColumnId::Size, 144.0));
        assert_eq!(output.widths[2], (ColumnId::Name, 333.0));
    }
}
