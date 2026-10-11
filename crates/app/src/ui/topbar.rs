use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use super::{icon_button, icon_text, pill, primary_button};
use crate::app::SwoopApp;
use crate::list_layout::{ColumnId, ListMode, ToolbarAction};
use crate::task_transfer::SortField;
use crate::theme::{ICON_SM, R_CTRL};

impl SwoopApp {
    pub fn topbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        // 固定高度：egui 会优先采用持久化的旧面板尺寸，内容变矮时会留出未绘制的空带
        egui::Panel::top("topbar")
            .exact_size(62.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 10 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let count = self.visible_tasks().len();
                    let title = self
                        .queue_filter
                        .and_then(|id| self.queues.iter().find(|q| q.id == id))
                        .map(super::queue_name)
                        .unwrap_or_else(|| self.category_filter.as_deref().unwrap_or(self.filter.label()));
                    ui.add(egui::Label::new(RichText::new(title).size(22.0).strong().color(p.text)).truncate());
                    ui.add_space(2.0);
                    pill(ui, &format!("{count}"), p.soft(p.text_muted), p.text_muted);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let toolbar = self.task_transfer.layout.toolbar.clone();
                        for action in toolbar.visible().rev() {
                            let clicked = if action == ToolbarAction::New {
                                primary_button(ui, &p, &format!("{}  {}", action.icon(), action.label())).clicked()
                            } else {
                                icon_button(ui, &p, action.icon(), action.label()).clicked()
                            };
                            if clicked {
                                self.run_toolbar_action(action, ui.ctx());
                            }
                        }
                        ui.menu_button(icon_text(egui_phosphor::regular::DOTS_THREE, ICON_SM, p.text_muted), |ui| {
                            for action in ToolbarAction::ALL {
                                if ui.button(format!("{}  {}", action.icon(), action.label())).clicked() {
                                    self.run_toolbar_action(action, ui.ctx());
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text(swoop_i18n::tr!("更多操作"));
                        self.list_layout_menu(ui);
                        ui.add_space(6.0);

                        let mut order = self.task_transfer.sort;
                        ui.menu_button(
                            icon_text(egui_phosphor::regular::SORT_ASCENDING, ICON_SM, p.text_muted),
                            |ui| {
                                for field in SortField::ALL {
                                    ui.selectable_value(&mut order.field, field, field.label());
                                }
                                ui.separator();
                                ui.checkbox(&mut order.descending, swoop_i18n::tr!("降序"));
                            },
                        )
                        .response
                        .on_hover_text(swoop_i18n::trf!("排序：{}", order.field.label()));
                        if order != self.task_transfer.sort
                            && let Err(error) = self.task_transfer.set_sort(&self.store, order)
                        {
                            self.toasts.push_error(swoop_i18n::tr!("保存排序失败"), error);
                        }
                        ui.add_space(4.0);

                        // 搜索框：自绘外框 + 无边框 TextEdit；详情面板打开、窗口又窄时让出空间给标题
                        let search_w = (ui.available_width() - 56.0).clamp(72.0, 150.0);
                        Frame::new()
                            .fill(p.surface)
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(R_CTRL))
                            .inner_margin(Margin { left: 10, right: 8, top: 6, bottom: 6 })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.label(icon_text(
                                        egui_phosphor::regular::MAGNIFYING_GLASS,
                                        ICON_SM,
                                        p.text_muted,
                                    ));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.search)
                                            .frame(Frame::NONE)
                                            .hint_text(swoop_i18n::tr!("搜索任务"))
                                            .desired_width(search_w)
                                            .margin(Margin::ZERO),
                                    );
                                });
                            });
                    });
                });
            });
    }
    fn run_toolbar_action(&mut self, action: ToolbarAction, ctx: &egui::Context) {
        match action {
            ToolbarAction::New => {
                self.add_dialog.queue = self.queue_filter.unwrap_or(swoop_core::MAIN_QUEUE_ID);
                self.add_dialog.open_with(None);
            }
            ToolbarAction::ResumeAll => self.engine.resume_all(),
            ToolbarAction::PauseAll => self.engine.pause_all(),
            ToolbarAction::Import => {
                self.add_dialog.queue = self.queue_filter.unwrap_or(swoop_core::MAIN_QUEUE_ID);
                self.start_import(ctx, None);
            }
            ToolbarAction::Media => {
                self.media_window.open = true;
                if self.media_window.destination.is_empty() {
                    self.media_window.destination = self.default_dir.join("media.mp4").display().to_string();
                }
            }
            ToolbarAction::Grabber => {
                self.grabber.open = true;
                if self.grabber.config.output_dir.as_os_str().is_empty() {
                    self.grabber.config.output_dir = self.default_dir.join("Site");
                }
            }
        }
    }

    fn list_layout_menu(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let mut view = self.task_transfer.view;
        let mut list = self.task_transfer.layout.list.clone();
        let mut toolbar = self.task_transfer.layout.toolbar.clone();
        ui.menu_button(icon_text(egui_phosphor::regular::SLIDERS_HORIZONTAL, ICON_SM, p.text_muted), |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut list.mode, ListMode::Cards, swoop_i18n::tr!("卡片"));
                ui.selectable_value(&mut list.mode, ListMode::Table, swoop_i18n::tr!("表格"));
            });
            ui.checkbox(&mut view.compact, swoop_i18n::tr!("紧凑显示"));
            ui.menu_button(swoop_i18n::tr!("表格列"), |ui| {
                ui.set_min_width(320.0);
                let mut reorder = None;
                let count = list.columns.len();
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    for (index, column) in list.columns.iter_mut().enumerate() {
                        ui.push_id(column.id, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled(
                                    column.id != ColumnId::Name,
                                    egui::Checkbox::new(&mut column.visible, column.id.label()),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(
                                            index + 1 < count,
                                            egui::Button::new(icon_text(
                                                egui_phosphor::regular::ARROW_DOWN,
                                                ICON_SM,
                                                p.text_muted,
                                            )),
                                        )
                                        .on_hover_text(swoop_i18n::tr!("后移"))
                                        .clicked()
                                    {
                                        reorder = Some((index, index + 1));
                                    }
                                    if ui
                                        .add_enabled(
                                            index > 0,
                                            egui::Button::new(icon_text(
                                                egui_phosphor::regular::ARROW_UP,
                                                ICON_SM,
                                                p.text_muted,
                                            )),
                                        )
                                        .on_hover_text(swoop_i18n::tr!("前移"))
                                        .clicked()
                                    {
                                        reorder = Some((index, index - 1));
                                    }
                                    ui.add(
                                        egui::DragValue::new(&mut column.width)
                                            .range(column.id.min_width()..=800.0)
                                            .max_decimals(0)
                                            .suffix(" px"),
                                    )
                                    .on_hover_text(swoop_i18n::tr!("列宽"));
                                });
                            });
                        });
                    }
                });
                if let Some((from, to)) = reorder {
                    list.columns.swap(from, to);
                }
                if ui.button(swoop_i18n::tr!("恢复默认列")).clicked() {
                    let mode = list.mode;
                    list = Default::default();
                    list.mode = mode;
                }
            });
            ui.menu_button(swoop_i18n::tr!("工具栏"), |ui| {
                ui.set_min_width(240.0);
                let mut reorder = None;
                let count = toolbar.order.len();
                for (index, action) in toolbar.order.iter().copied().enumerate() {
                    ui.push_id(action, |ui| {
                        ui.horizontal(|ui| {
                            let mut visible = !toolbar.hidden.contains(&action);
                            if ui.checkbox(&mut visible, action.label()).changed() {
                                if visible {
                                    toolbar.hidden.retain(|item| *item != action);
                                } else {
                                    toolbar.hidden.push(action);
                                }
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(
                                        index + 1 < count,
                                        egui::Button::new(icon_text(
                                            egui_phosphor::regular::ARROW_DOWN,
                                            ICON_SM,
                                            p.text_muted,
                                        )),
                                    )
                                    .on_hover_text(swoop_i18n::tr!("后移"))
                                    .clicked()
                                {
                                    reorder = Some((index, index + 1));
                                }
                                if ui
                                    .add_enabled(
                                        index > 0,
                                        egui::Button::new(icon_text(
                                            egui_phosphor::regular::ARROW_UP,
                                            ICON_SM,
                                            p.text_muted,
                                        )),
                                    )
                                    .on_hover_text(swoop_i18n::tr!("前移"))
                                    .clicked()
                                {
                                    reorder = Some((index, index - 1));
                                }
                            });
                        });
                    });
                }
                if let Some((from, to)) = reorder {
                    toolbar.order.swap(from, to);
                }
                if ui.button(swoop_i18n::tr!("恢复默认工具栏")).clicked() {
                    toolbar = Default::default();
                }
            });
            ui.menu_button(swoop_i18n::tr!("卡片信息"), |ui| {
                ui.checkbox(&mut view.progress_bar, swoop_i18n::tr!("分段进度条"));
                ui.checkbox(&mut view.size, swoop_i18n::tr!("文件大小"));
                ui.checkbox(&mut view.percent, swoop_i18n::tr!("进度百分比"));
                ui.checkbox(&mut view.connections, swoop_i18n::tr!("连接数"));
                ui.checkbox(&mut view.remaining_time, swoop_i18n::tr!("剩余时间"));
                ui.checkbox(&mut view.directory, swoop_i18n::tr!("保存目录"));
                ui.checkbox(&mut view.host, swoop_i18n::tr!("来源站点"));
                ui.checkbox(&mut view.speed, swoop_i18n::tr!("下载速度"));
                if ui.button(swoop_i18n::tr!("恢复默认卡片")).clicked() {
                    view = Default::default();
                }
            });
        })
        .response
        .on_hover_text(swoop_i18n::tr!("列表与工具栏"));
        if (list != self.task_transfer.layout.list || toolbar != self.task_transfer.layout.toolbar)
            && let Err(error) = self.task_transfer.layout.save(&self.store, list, toolbar)
        {
            self.toasts.push_error(swoop_i18n::tr!("布局未保存"), error);
        }
        if view != self.task_transfer.view
            && let Err(error) = self.task_transfer.set_view(&self.store, view)
        {
            self.toasts.push_error(swoop_i18n::tr!("显示设置未保存"), error);
        }
    }
}
