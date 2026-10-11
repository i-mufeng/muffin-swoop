use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui::{Align, Frame, Layout, Margin, RichText};

use super::{primary_button, toggle};
use crate::app::SwoopApp;
use crate::bridge::BridgeMode;
use crate::notifications::NotificationTiming;
use crate::theme::Mode;

impl SwoopApp {
    pub fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        if !self.is_quitting() {
            self.startup.refresh_if_stale(ctx);
        }
        let p = self.palette;
        let mut close = false;
        let mut dirty = false;
        let mut bridge_changed = false;

        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(if self.language == swoop_i18n::Language::English { 600.0 } else { 480.0 });
                ui.label(RichText::new(swoop_i18n::tr!("设置")).size(18.0).strong().color(p.text));
                ui.add_space(14.0);

                egui::ScrollArea::vertical()
                    .id_salt("settings_scroll")
                    .max_height((ctx.content_rect().height() - 160.0).max(160.0))
                    .show(ui, |ui| {
                        egui::Grid::new("settings_grid").num_columns(2).spacing([18.0, 14.0]).show(ui, |ui| {
                            ui.label(RichText::new(swoop_i18n::tr!("默认保存目录")).color(p.text_muted));
                            let mut dir = self.default_dir.display().to_string();
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut dir)
                                        .desired_width(300.0)
                                        .margin(Margin::symmetric(10, 6)),
                                )
                                .changed()
                            {
                                self.default_dir = PathBuf::from(&dir);
                                self.add_dialog.dir = dir;
                                dirty = true;
                            }
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("软件更新")).color(p.text_muted));
                            if ui.button(swoop_i18n::tr!("检查与更新…")).clicked() {
                                self.updates.open = true;
                                close = true;
                            }
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("站点认证")).color(p.text_muted));
                            if ui.button(swoop_i18n::tr!("账号与登录方式…")).clicked() {
                                self.site_auth_editor = Some(crate::ui::site_auth::SiteAuthEditor::new(&self.engine));
                                close = true;
                            }
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("同名文件")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                use swoop_core::FileConflictPolicy as Policy;
                                let previous = self.engine.file_conflict_policy();
                                let mut selected = previous;
                                for (policy, label) in [
                                    (Policy::Ask, swoop_i18n::tr!("询问")),
                                    (Policy::Rename, swoop_i18n::tr!("重命名")),
                                    (Policy::Overwrite, swoop_i18n::tr!("覆盖")),
                                    (Policy::Skip, swoop_i18n::tr!("跳过")),
                                ] {
                                    ui.selectable_value(&mut selected, policy, label);
                                }
                                if selected != previous
                                    && let Err(error) = self.engine.set_file_conflict_policy(selected)
                                {
                                    self.toasts.push_error(swoop_i18n::tr!("同名策略未保存"), error.to_string());
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("外观")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                if ui.selectable_label(p.mode == Mode::Dark, swoop_i18n::tr!("深色")).clicked() {
                                    self.set_theme(ctx, Mode::Dark);
                                }
                                if ui.selectable_label(p.mode == Mode::Light, swoop_i18n::tr!("浅色")).clicked() {
                                    self.set_theme(ctx, Mode::Light);
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("语言 / Language")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                for (language, label) in [
                                    (swoop_i18n::Language::Chinese, swoop_i18n::tr!("简体中文")),
                                    (swoop_i18n::Language::English, "English"),
                                ] {
                                    if ui.selectable_label(self.language == language, label).clicked()
                                        && self.language != language
                                    {
                                        self.set_language(ctx, language);
                                    }
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("进度条样式")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                use super::progress::ProgressStyle as S;
                                for (s, label) in [
                                    (S::Cells, swoop_i18n::tr!("像素格")),
                                    (S::Lanes, swoop_i18n::tr!("轨道")),
                                    (S::Overlay, swoop_i18n::tr!("叠加")),
                                ] {
                                    if ui.selectable_label(self.progress_style == s, label).clicked() {
                                        self.progress_style = s;
                                        dirty = true;
                                    }
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("监听剪贴板链接")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                if toggle(ui, &p, &mut self.clipboard_watch).changed() {
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("复制链接后右下角询问是否下载"))
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("进度窗口")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                if toggle(ui, &p, &mut self.progress_window).changed() {
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("单个下载开始时弹出独立的进度窗口"))
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("拖放")).color(p.text_muted));
                            ui.vertical(|ui| {
                                self.native_drag.settings_ui(ui, &self.store);
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("启动时自动继续")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                if toggle(ui, &p, &mut self.auto_resume).changed() {
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("上次未完成的任务在启动后立即续传"))
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("登录时启动")).color(p.text_muted));
                            ui.vertical(|ui| {
                                let busy = self.startup.busy();
                                let packaged = self.startup.state.as_ref().is_some_and(|state| state.packaged);
                                ui.add_enabled_ui(!busy, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut self.startup.desired, swoop_i18n::tr!("启用"));
                                        if ui
                                            .add_enabled(
                                                !self.startup.desired || packaged,
                                                egui::Button::new(swoop_i18n::tr!("应用")),
                                            )
                                            .clicked()
                                        {
                                            self.startup.apply(ctx);
                                        }
                                        if ui.small_button(swoop_i18n::tr!("刷新")).clicked() {
                                            self.startup.refresh(ctx);
                                        }
                                    });
                                });
                                if busy {
                                    ui.spinner();
                                }
                                if let Some(state) = &self.startup.state {
                                    ui.label(RichText::new(&state.detail).size(12.0).color(p.text_muted));
                                    if !state.packaged {
                                        ui.label(
                                            RichText::new(swoop_i18n::tr!("从完整安装包启动后可启用"))
                                                .size(12.0)
                                                .color(p.text_muted),
                                        );
                                    } else if state.enabled && !state.matches_current {
                                        ui.label(
                                            RichText::new(swoop_i18n::tr!("点击应用以更新启动路径"))
                                                .size(12.0)
                                                .color(p.text_muted),
                                        );
                                    }
                                }
                                if let Some(error) = &self.startup.error {
                                    ui.colored_label(p.danger, error);
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("系统通知")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                dirty |= toggle(ui, &p, &mut self.notifications.on_complete).changed();
                                ui.label(swoop_i18n::tr!("完成"));
                                ui.add_space(8.0);
                                dirty |= toggle(ui, &p, &mut self.notifications.on_failure).changed();
                                ui.label(swoop_i18n::tr!("失败"));
                            });
                            ui.end_row();

                            if self.notifications.on_complete || self.notifications.on_failure {
                                ui.label(RichText::new(swoop_i18n::tr!("通知时机")).color(p.text_muted));
                                ui.horizontal(|ui| {
                                    for (timing, label) in [
                                        (NotificationTiming::Background, swoop_i18n::tr!("仅后台")),
                                        (NotificationTiming::Always, swoop_i18n::tr!("始终")),
                                    ] {
                                        if ui.selectable_label(self.notifications.timing == timing, label).clicked() {
                                            self.notifications.timing = timing;
                                            dirty = true;
                                        }
                                    }
                                    if let Some(notifier) = &self.notifier
                                        && ui.small_button(swoop_i18n::tr!("测试通知")).clicked()
                                    {
                                        notifier.test(self.notifications.sound);
                                    }
                                });
                                ui.end_row();
                            }

                            ui.label(RichText::new(swoop_i18n::tr!("通知声音")).color(p.text_muted));
                            dirty |= toggle(ui, &p, &mut self.notifications.sound).changed();
                            ui.end_row();

                            if self.tray.is_some() {
                                ui.label(RichText::new(swoop_i18n::tr!("关闭窗口后")).color(p.text_muted));
                                ui.horizontal(|ui| {
                                    if toggle(ui, &p, &mut self.close_to_tray).changed() {
                                        dirty = true;
                                    }
                                    ui.label(
                                        RichText::new(swoop_i18n::tr!("保留在菜单栏，继续下载"))
                                            .size(12.0)
                                            .color(p.text_muted),
                                    );
                                });
                                ui.end_row();

                                if cfg!(target_os = "macos") {
                                    ui.label(RichText::new(swoop_i18n::tr!("菜单栏显示速度")).color(p.text_muted));
                                    ui.horizontal(|ui| {
                                        if toggle(ui, &p, &mut self.tray_speed).changed() {
                                            dirty = true;
                                        }
                                        ui.label(
                                            RichText::new(swoop_i18n::tr!("下载时在图标旁显示总速度"))
                                                .size(12.0)
                                                .color(p.text_muted),
                                        );
                                    });
                                    ui.end_row();
                                }
                            }

                            ui.label(RichText::new(swoop_i18n::tr!("浏览器集成")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                if toggle(ui, &p, &mut self.bridge_enabled).changed() {
                                    bridge_changed = true;
                                }
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("接管浏览器扩展交来的下载"))
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            ui.end_row();

                            if self.bridge_enabled {
                                ui.label(RichText::new(swoop_i18n::tr!("接收方式")).color(p.text_muted));
                                ui.horizontal(|ui| {
                                    for (m, label) in [
                                        (BridgeMode::Silent, swoop_i18n::tr!("直接下载")),
                                        (BridgeMode::Ask, swoop_i18n::tr!("弹出确认")),
                                    ] {
                                        if ui.selectable_label(self.bridge_mode == m, label).clicked() {
                                            self.bridge_mode = m;
                                            dirty = true;
                                        }
                                    }
                                });
                                ui.end_row();

                                ui.label(RichText::new(swoop_i18n::tr!("端口")).color(p.text_muted));
                                ui.horizontal(|ui| {
                                    // 输入中的文本放在 egui 内存里，失焦/回车时才生效，避免每敲一位就重绑一次端口
                                    let id = ui.id().with("bridge_port_input");
                                    let mut text = ui
                                        .data_mut(|d| d.get_temp::<String>(id))
                                        .unwrap_or_else(|| self.bridge_port.to_string());
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut text)
                                            .desired_width(72.0)
                                            .margin(Margin::symmetric(10, 6)),
                                    );
                                    if resp.lost_focus() {
                                        match text.trim().parse::<u16>() {
                                            Ok(port) if port >= 1024 && port != self.bridge_port => {
                                                self.bridge_port = port;
                                                bridge_changed = true;
                                            }
                                            _ => text = self.bridge_port.to_string(),
                                        }
                                    }
                                    ui.data_mut(|d| d.insert_temp(id, text));
                                    self.bridge_status_label(ui);
                                });
                                ui.end_row();
                            }

                            ui.label(RichText::new(swoop_i18n::tr!("每任务连接数")).color(p.text_muted));
                            ui.horizontal(|ui| {
                                let mut connections = self.engine.connections();
                                if ui
                                    .add(egui::DragValue::new(&mut connections).range(1..=swoop_core::MAX_CONNECTIONS))
                                    .changed()
                                {
                                    self.engine.set_connections(connections);
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new(swoop_i18n::tr!("新建或继续任务时生效"))
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("批量添加上限")).color(p.text_muted));
                            dirty |= ui
                                .add(
                                    egui::DragValue::new(&mut self.batch_limit)
                                        .range(1..=crate::batch::MAX_BATCH_LIMIT)
                                        .suffix(swoop_i18n::tr!(" 个")),
                                )
                                .changed();
                            ui.end_row();

                            ui.label(RichText::new(swoop_i18n::tr!("版本")).color(p.text_muted));
                            ui.label(format!("Swoop {}", env!("CARGO_PKG_VERSION")));
                            ui.end_row();
                        });
                        ui.add_space(12.0);
                        if ui.button(swoop_i18n::tr!("网络与重试…")).clicked() {
                            self.network_editor =
                                Some(crate::ui::network::NetworkEditor::new(self.engine.network_policy()));
                            self.network_bypass = self
                                .engine
                                .network_policy()
                                .proxy
                                .as_ref()
                                .map(|p| p.bypass.join(", "))
                                .unwrap_or_default();
                            self.network_error = None;
                            self.show_settings = false;
                        }
                        if ui.button(swoop_i18n::tr!("下载完成后…")).clicked() {
                            self.completion_editor =
                                Some(super::completion::CompletionEditor::new(&self.completion.settings));
                            self.show_settings = false;
                        }
                        if ui.button(swoop_i18n::tr!("文件下载配额…")).clicked() {
                            self.quota_editor = Some(self.engine.quota().policy);
                            self.quota_error = None;
                            self.show_settings = false;
                        }
                    });

                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, &p, swoop_i18n::tr!("完成")).clicked() {
                        close = true;
                    }
                });
            });

        if bridge_changed {
            self.apply_bridge();
            dirty = true;
        }
        if dirty {
            self.save_settings();
        }
        if close || modal.should_close() {
            self.show_settings = false;
        }
    }
}

impl SwoopApp {
    /// 端口后面的一行状态：运行情况 + 扩展最近一次心跳
    fn bridge_status_label(&self, ui: &mut egui::Ui) {
        let p = self.palette;
        let (text, color) = match (self.bridge.port(), &self.bridge_error) {
            (None, Some(err)) => (swoop_i18n::trf!("未运行：{err}"), p.danger),
            (None, None) => (swoop_i18n::tr!("未运行").to_string(), p.text_muted),
            (Some(_), _) => match self.bridge.last_ping() {
                // 扩展每 30 秒心跳一次，留一倍余量
                Some(t) if SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(70) => {
                    (swoop_i18n::tr!("扩展已连接").to_string(), p.success)
                }
                Some(t) => (swoop_i18n::trf!("扩展 {} 连接过", super::format::ago(t)), p.text_muted),
                None => (swoop_i18n::tr!("运行中，尚未检测到扩展").to_string(), p.text_muted),
            },
        };
        ui.label(RichText::new(text).size(12.0).color(color));
    }
}
