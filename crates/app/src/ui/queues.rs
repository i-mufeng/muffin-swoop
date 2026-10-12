use chrono::{Local, NaiveDateTime, TimeZone};
use egui::{Align, Layout};
use swoop_core::{MAIN_QUEUE_ID, MAX_QUEUE_CONCURRENCY, QueueId, QueueSchedule, QueueSnapshot};

use super::{danger_button, dialog, ghost_button, icon_button, primary_button, toggle};
use crate::app::SwoopApp;

pub struct QueueEditor {
    pub open: bool,
    pub selected: QueueId,
    loaded: Option<QueueId>,
    name: String,
    concurrency: usize,
    sync_enabled: bool,
    start_enabled: bool,
    stop_enabled: bool,
    start: String,
    stop: String,
    repeat: bool,
    repeat_minutes: u64,
    error: Option<String>,
    confirm_delete: bool,
}

impl Default for QueueEditor {
    fn default() -> Self {
        Self {
            open: false,
            selected: MAIN_QUEUE_ID,
            loaded: None,
            name: String::new(),
            concurrency: 3,
            sync_enabled: false,
            start_enabled: false,
            stop_enabled: false,
            start: String::new(),
            stop: String::new(),
            repeat: false,
            repeat_minutes: 60,
            error: None,
            confirm_delete: false,
        }
    }
}

impl QueueEditor {
    pub fn show(&mut self, id: QueueId) {
        self.selected = id;
        self.loaded = None;
        self.open = true;
        self.error = None;
        self.confirm_delete = false;
    }

    fn load(&mut self, queue: &QueueSnapshot) {
        if self.loaded == Some(queue.id) {
            return;
        }
        self.loaded = Some(queue.id);
        self.name = queue.name.clone();
        self.concurrency = queue.max_concurrent;
        self.sync_enabled = queue.sync_enabled;
        let now = Local::now().timestamp();
        self.start_enabled = queue.schedule.as_ref().is_some_and(|s| s.start_at.is_some());
        self.stop_enabled = queue.schedule.as_ref().is_some_and(|s| s.stop_at.is_some());
        self.start = format_time(queue.schedule.as_ref().and_then(|s| s.start_at).unwrap_or(now + 3600));
        self.stop = format_time(queue.schedule.as_ref().and_then(|s| s.stop_at).unwrap_or(now + 7200));
        let repeat = queue.schedule.as_ref().and_then(|s| s.repeat_every_secs);
        self.repeat = repeat.is_some();
        self.repeat_minutes = repeat.unwrap_or(3600) / 60;
        self.error = None;
        self.confirm_delete = false;
    }

    fn schedule(&self) -> Result<Option<QueueSchedule>, String> {
        if !self.start_enabled && !self.stop_enabled {
            if self.repeat {
                return Err(swoop_i18n::tr!("请先设置开始或停止时间").into());
            }
            return Ok(None);
        }
        let schedule = QueueSchedule {
            start_at: if self.start_enabled { Some(parse_time(&self.start)?) } else { None },
            stop_at: if self.stop_enabled { Some(parse_time(&self.stop)?) } else { None },
            repeat_every_secs: if self.repeat {
                Some(self.repeat_minutes.checked_mul(60).ok_or(swoop_i18n::tr!("重复间隔过大"))?)
            } else {
                None
            },
        };
        schedule.validate().map_err(|e| e.to_string())?;
        Ok(Some(schedule))
    }
}

fn format_time(timestamp: i64) -> String {
    Local.timestamp_opt(timestamp, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

fn parse_time(value: &str) -> Result<i64, String> {
    let time = NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M")
        .map_err(|_| swoop_i18n::tr!("请输入有效时间，例如 2026-10-11 22:30"))?;
    time.and_local_timezone(Local)
        .single()
        .map(|t| t.timestamp())
        .ok_or_else(|| swoop_i18n::tr!("该本地时间不存在或存在夏令时歧义，请选择其它时间").into())
}

impl SwoopApp {
    pub fn queues_ui(&mut self, ctx: &egui::Context) {
        if !self.queue_editor.open {
            return;
        }
        let p = self.palette;
        let queues = self.engine.queues();
        if !queues.iter().any(|q| q.id == self.queue_editor.selected) {
            self.queue_editor.selected = MAIN_QUEUE_ID;
        }
        let Some(queue) = queues.iter().find(|q| q.id == self.queue_editor.selected) else { return };
        self.queue_editor.load(queue);
        let mut save = false;
        let mut close = false;
        let mut create = false;
        let mut delete = false;
        let mut toggle_running = false;
        let mut sync_now = false;
        let mut order = None;
        let modal = dialog::modal("queue_editor", &p).show(ctx, |ui| {
            ui.set_width(600.0);
            close |= dialog::header(
                ui,
                &p,
                egui_phosphor::regular::STACK,
                swoop_i18n::tr!("队列与计划"),
                Some(&swoop_i18n::trf!("{} 下载中 · {} 等待", queue.active_count, queue.waiting_count)),
            );
            dialog::body_fixed(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("queue_select")
                        .selected_text(super::queue_name(queue))
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for item in &queues {
                                if ui.selectable_label(item.id == queue.id, super::queue_name(item)).clicked() {
                                    self.queue_editor.selected = item.id;
                                    self.queue_editor.loaded = None;
                                }
                            }
                        });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ghost_button(
                            ui,
                            &p,
                            &format!("{}  {}", egui_phosphor::regular::PLUS, swoop_i18n::tr!("新建队列")),
                        )
                        .clicked()
                        {
                            create = true;
                        }
                        let (icon, label) = if queue.running {
                            (egui_phosphor::regular::PAUSE, swoop_i18n::tr!("停止队列"))
                        } else {
                            (egui_phosphor::regular::PLAY, swoop_i18n::tr!("启动队列"))
                        };
                        if ghost_button(ui, &p, &format!("{icon}  {label}")).clicked() {
                            toggle_running = true;
                        }
                    });
                });
                ui.add_space(12.0);
            });
            dialog::body(ui, "queue_scroll", |ui| {
                let editor = &mut self.queue_editor;
                dialog::group(ui, &p, None, |ui| {
                    dialog::row(ui, &p, swoop_i18n::tr!("名称"), None, |ui| {
                        ui.add(super::field(&mut editor.name).desired_width(260.0));
                    });
                    dialog::row(ui, &p, swoop_i18n::tr!("同时下载"), None, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut editor.concurrency)
                                .range(1..=MAX_QUEUE_CONCURRENCY)
                                .suffix(swoop_i18n::tr!(" 个任务")),
                        );
                    });
                    dialog::row(
                        ui,
                        &p,
                        swoop_i18n::tr!("同步已完成文件"),
                        Some(swoop_i18n::tr!("每次启动时检查更新；下载成功后替换，保留手动修改过的文件")),
                        |ui| {
                            toggle(ui, &p, &mut editor.sync_enabled);
                            ui.add_space(4.0);
                            if ghost_button(ui, &p, swoop_i18n::tr!("立即同步")).clicked() {
                                sync_now = true;
                            }
                        },
                    );
                });

                dialog::group(ui, &p, Some(swoop_i18n::tr!("计划")), |ui| {
                    dialog::row(ui, &p, swoop_i18n::tr!("定时开始"), None, |ui| {
                        toggle(ui, &p, &mut editor.start_enabled);
                        ui.add_space(4.0);
                        ui.add_enabled(editor.start_enabled, super::field(&mut editor.start).desired_width(170.0));
                    });
                    dialog::row(ui, &p, swoop_i18n::tr!("定时停止"), None, |ui| {
                        toggle(ui, &p, &mut editor.stop_enabled);
                        ui.add_space(4.0);
                        ui.add_enabled(editor.stop_enabled, super::field(&mut editor.stop).desired_width(170.0));
                    });
                    dialog::row(ui, &p, swoop_i18n::tr!("重复执行"), None, |ui| {
                        toggle(ui, &p, &mut editor.repeat);
                        ui.add_space(4.0);
                        ui.add_enabled(
                            editor.repeat,
                            egui::DragValue::new(&mut editor.repeat_minutes)
                                .range(1..=525600)
                                .prefix(swoop_i18n::tr!("每 "))
                                .suffix(swoop_i18n::tr!(" 分钟")),
                        );
                    });
                });

                dialog::group_title(ui, &p, swoop_i18n::tr!("任务顺序"));
                if queue.task_ids.is_empty() {
                    dialog::note(ui, &p, swoop_i18n::tr!("新建下载时可选择此队列"));
                } else {
                    dialog::group(ui, &p, None, |ui| {
                        for (index, id) in queue.task_ids.iter().enumerate() {
                            let Some(task) = self.tasks.iter().find(|t| t.id == *id) else { continue };
                            ui.push_id(id.0, |ui| {
                                let name = if task.file_name.is_empty() { &task.url } else { &task.file_name };
                                dialog::row_with(ui, &p, &format!("{}.  {name}", index + 1), None, 0.8, |ui| {
                                    if icon_button(ui, &p, egui_phosphor::regular::ARROW_DOWN, swoop_i18n::tr!("下移"))
                                        .clicked()
                                        && index + 1 < queue.task_ids.len()
                                    {
                                        let mut next = queue.task_ids.clone();
                                        next.swap(index, index + 1);
                                        order = Some(next);
                                    }
                                    if icon_button(ui, &p, egui_phosphor::regular::ARROW_UP, swoop_i18n::tr!("上移"))
                                        .clicked()
                                        && index > 0
                                    {
                                        let mut next = queue.task_ids.clone();
                                        next.swap(index, index - 1);
                                        order = Some(next);
                                    }
                                });
                            });
                        }
                    });
                }
                if let Some(error) = &self.queue_editor.error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                if primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked() {
                    save = true;
                }
                if ghost_button(ui, &p, swoop_i18n::tr!("关闭")).clicked() {
                    close = true;
                }
                if queue.id != MAIN_QUEUE_ID {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if self.queue_editor.confirm_delete {
                            if danger_button(ui, &p, swoop_i18n::tr!("确认删除")).clicked() {
                                delete = true;
                            }
                            if ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked() {
                                self.queue_editor.confirm_delete = false;
                            }
                            dialog::note(ui, &p, swoop_i18n::tr!("任务将移回主队列"));
                        } else if danger_button(ui, &p, swoop_i18n::tr!("删除队列")).clicked() {
                            self.queue_editor.confirm_delete = true;
                        }
                    });
                }
            });
        });
        let result = if save {
            match self.queue_editor.schedule() {
                Ok(schedule) => self
                    .engine
                    .rename_queue(queue.id, &self.queue_editor.name)
                    .and_then(|_| self.engine.set_queue_concurrency(queue.id, self.queue_editor.concurrency))
                    .and_then(|_| self.engine.set_queue_sync_enabled(queue.id, self.queue_editor.sync_enabled))
                    .and_then(|_| {
                        if schedule == queue.schedule {
                            Ok(())
                        } else {
                            self.engine.set_queue_schedule(queue.id, schedule)
                        }
                    }),
                Err(error) => {
                    self.queue_editor.error = Some(error);
                    return;
                }
            }
        } else if sync_now {
            self.engine.sync_queue(queue.id).map(|count| {
                self.toasts.push_info(swoop_i18n::tr!("检查更新"), swoop_i18n::trf!("已安排 {count} 个文件检查更新"));
            })
        } else if toggle_running {
            if queue.running { self.engine.stop_queue(queue.id) } else { self.engine.start_queue(queue.id) }
        } else if let Some(order) = order {
            self.engine.reorder_queue(queue.id, &order)
        } else if delete {
            self.engine.delete_queue(queue.id)
        } else {
            Ok(())
        };
        match result {
            Err(error) => self.queue_editor.error = Some(error.to_string()),
            Ok(()) if save => {
                self.queue_editor.loaded = None;
                self.toasts.push_info(swoop_i18n::tr!("已保存队列"), &self.queue_editor.name);
            }
            Ok(()) => {}
        }
        if create {
            let mut n = 1;
            while queues.iter().any(|q| q.name == swoop_i18n::trf!("队列 {n}")) {
                n += 1;
            }
            match self.engine.create_queue(swoop_i18n::trf!("队列 {n}"), 3) {
                Ok(id) => self.queue_editor.show(id),
                Err(error) => self.queue_editor.error = Some(error.to_string()),
            }
        }
        if close || modal.should_close() {
            self.queue_editor.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_schedule_round_trip_and_invalid_dates() {
        let value = "2026-10-11 22:30";
        assert_eq!(format_time(parse_time(value).unwrap()), value);
        assert!(parse_time("2026-02-30 22:30").is_err());
        assert!(parse_time("tomorrow").is_err());
    }
    #[test]
    fn schedule_editor_rejects_inverted_and_overlapping_windows() {
        let mut editor = QueueEditor {
            start_enabled: true,
            stop_enabled: true,
            start: "2026-10-11 22:30".into(),
            stop: "2026-10-11 21:30".into(),
            ..Default::default()
        };
        assert!(editor.schedule().is_err());
        editor.stop = "2026-10-11 23:30".into();
        assert!(editor.schedule().is_ok());
        editor.repeat = true;
        editor.repeat_minutes = 30;
        assert!(editor.schedule().is_err());
        editor.repeat_minutes = 120;
        assert!(editor.schedule().is_ok());
    }
}
