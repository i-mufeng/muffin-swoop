use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui::{Align, Frame, Layout, Margin, RichText, Stroke, vec2};

use super::{dialog, field, segmented, toggle};
use crate::app::SwoopApp;
use crate::bridge::BridgeMode;
use crate::notifications::NotificationTiming;
use crate::theme::Mode;

/// 设置页左侧分类
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    General,
    Download,
    Notify,
    Integration,
    About,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::General, Tab::Download, Tab::Notify, Tab::Integration, Tab::About];

    fn label(self) -> &'static str {
        match self {
            Tab::General => swoop_i18n::tr!("常规"),
            Tab::Download => swoop_i18n::tr!("下载"),
            Tab::Notify => swoop_i18n::tr!("通知"),
            Tab::Integration => swoop_i18n::tr!("集成"),
            Tab::About => swoop_i18n::tr!("关于"),
        }
    }

    fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Tab::General => i::SLIDERS_HORIZONTAL,
            Tab::Download => i::DOWNLOAD_SIMPLE,
            Tab::Notify => i::BELL_SIMPLE,
            Tab::Integration => i::PLUGS_CONNECTED,
            Tab::About => i::INFO,
        }
    }
}

/// 当前分类存在 egui 内存里，关掉再打开设置页时保留
fn tab_id() -> egui::Id {
    egui::Id::new("settings_tab")
}

#[cfg(feature = "screenshot")]
impl SwoopApp {
    /// 开发截图：打开设置页并切到指定分类；名称不认识时返回 false
    pub(crate) fn show_settings_tab(&mut self, ctx: &egui::Context, name: &str) -> bool {
        let tab = match name {
            "general" => Tab::General,
            "download" => Tab::Download,
            "notify" => Tab::Notify,
            "integration" => Tab::Integration,
            "about" => Tab::About,
            _ => return false,
        };
        self.show_settings = true;
        ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
        true
    }
}

/// 本帧设置页里发生的变更，统一在关闭 Modal 后处理
#[derive(Default)]
struct Changes {
    close: bool,
    dirty: bool,
    bridge: bool,
}

impl SwoopApp {
    pub fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        if !self.is_quitting() {
            self.startup.refresh_if_stale(ctx);
        }
        let p = self.palette;
        let mut changes = Changes::default();
        let tab_id = tab_id();
        let mut tab = ctx.data(|d| d.get_temp::<Tab>(tab_id)).unwrap_or(Tab::General);

        let modal = dialog::modal("settings", &p).show(ctx, |ui| {
            let width = if self.language == swoop_i18n::Language::English { 840.0 } else { 760.0 };
            ui.set_width(width);
            changes.close |= dialog::header(ui, &p, egui_phosphor::regular::GEAR, swoop_i18n::tr!("设置"), None);
            let y = ui.cursor().top();
            ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));

            let height = (ctx.content_rect().height() - 150.0).clamp(260.0, 540.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // 左侧分类
                ui.allocate_ui_with_layout(vec2(180.0, height), Layout::top_down(Align::Min), |ui| {
                    ui.set_min_size(vec2(180.0, height));
                    Frame::new().inner_margin(Margin { left: 14, right: 12, top: 14, bottom: 14 }).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for t in Tab::ALL {
                            if super::nav_item(ui, &p, t.icon(), None, t.label(), None, tab == t).clicked() {
                                tab = t;
                            }
                        }
                    });
                });
                let rect = ui.max_rect();
                let x = ui.cursor().left();
                ui.painter().vline(x, rect.top()..=rect.top() + height, Stroke::new(1.0, p.border));
                // 右侧内容
                ui.allocate_ui_with_layout(vec2(ui.available_width(), height), Layout::top_down(Align::Min), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), height));
                    egui::ScrollArea::vertical()
                        .id_salt(("settings_scroll", tab as u8))
                        .max_height(height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            Frame::new().inner_margin(Margin { left: 22, right: 22, top: 16, bottom: 10 }).show(
                                ui,
                                |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 6.0;
                                    match tab {
                                        Tab::General => self.settings_general(ui, ctx, &mut changes),
                                        Tab::Download => self.settings_download(ui, &mut changes),
                                        Tab::Notify => self.settings_notify(ui, &mut changes),
                                        Tab::Integration => self.settings_integration(ui, ctx, &mut changes),
                                        Tab::About => self.settings_about(ui, ctx),
                                    }
                                },
                            );
                        });
                });
            });
        });
        ctx.data_mut(|d| d.insert_temp(tab_id, tab));

        if changes.bridge {
            self.apply_bridge();
            changes.dirty = true;
        }
        if changes.dirty {
            self.save_settings();
        }
        if changes.close || modal.should_close() {
            self.show_settings = false;
        }
    }

    fn settings_general(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, changes: &mut Changes) {
        let p = self.palette;
        dialog::group(ui, &p, Some(swoop_i18n::tr!("外观")), |ui| {
            dialog::row(ui, &p, swoop_i18n::tr!("主题"), None, |ui| {
                let mut mode = p.mode;
                if segmented(
                    ui,
                    &p,
                    &mut mode,
                    &[(Mode::Dark, swoop_i18n::tr!("深色")), (Mode::Light, swoop_i18n::tr!("浅色"))],
                )
                .changed()
                {
                    self.set_theme(ctx, mode);
                }
            });
            dialog::row(ui, &p, swoop_i18n::tr!("语言 / Language"), None, |ui| {
                let mut language = self.language;
                if segmented(
                    ui,
                    &p,
                    &mut language,
                    &[
                        (swoop_i18n::Language::Chinese, swoop_i18n::tr!("简体中文")),
                        (swoop_i18n::Language::English, "English"),
                    ],
                )
                .changed()
                {
                    self.set_language(ctx, language);
                }
            });
            dialog::row(ui, &p, swoop_i18n::tr!("进度条样式"), None, |ui| {
                use super::progress::ProgressStyle as S;
                changes.dirty |= segmented(
                    ui,
                    &p,
                    &mut self.progress_style,
                    &[
                        (S::Cells, swoop_i18n::tr!("像素格")),
                        (S::Lanes, swoop_i18n::tr!("轨道")),
                        (S::Overlay, swoop_i18n::tr!("叠加")),
                    ],
                )
                .changed();
            });
        });

        dialog::group(ui, &p, Some(swoop_i18n::tr!("文件")), |ui| {
            dialog::row(ui, &p, swoop_i18n::tr!("默认保存目录"), None, |ui| {
                let mut dir = self.default_dir.display().to_string();
                if ui.add(field(&mut dir).desired_width(ui.available_width())).changed() {
                    self.default_dir = PathBuf::from(&dir);
                    self.add_dialog.dir = dir;
                    changes.dirty = true;
                }
            });
            dialog::row(ui, &p, swoop_i18n::tr!("同名文件"), None, |ui| {
                use swoop_core::FileConflictPolicy as Policy;
                let previous = self.engine.file_conflict_policy();
                let mut selected = previous;
                segmented(
                    ui,
                    &p,
                    &mut selected,
                    &[
                        (Policy::Ask, swoop_i18n::tr!("询问")),
                        (Policy::Rename, swoop_i18n::tr!("重命名")),
                        (Policy::Overwrite, swoop_i18n::tr!("覆盖")),
                        (Policy::Skip, swoop_i18n::tr!("跳过")),
                    ],
                );
                if selected != previous
                    && let Err(error) = self.engine.set_file_conflict_policy(selected)
                {
                    self.toasts.push_error(swoop_i18n::tr!("同名策略未保存"), error.to_string());
                }
            });
        });
    }

    fn settings_download(&mut self, ui: &mut egui::Ui, changes: &mut Changes) {
        let p = self.palette;
        dialog::group(ui, &p, Some(swoop_i18n::tr!("任务")), |ui| {
            dialog::row(
                ui,
                &p,
                swoop_i18n::tr!("每任务连接数"),
                Some(swoop_i18n::tr!("新建或继续任务时生效")),
                |ui| {
                    let mut connections = self.engine.connections();
                    if ui.add(egui::DragValue::new(&mut connections).range(1..=swoop_core::MAX_CONNECTIONS)).changed() {
                        self.engine.set_connections(connections);
                        changes.dirty = true;
                    }
                },
            );
            dialog::row(ui, &p, swoop_i18n::tr!("批量添加上限"), None, |ui| {
                changes.dirty |= ui
                    .add(
                        egui::DragValue::new(&mut self.batch_limit)
                            .range(1..=crate::batch::MAX_BATCH_LIMIT)
                            .suffix(swoop_i18n::tr!(" 个")),
                    )
                    .changed();
            });
            dialog::row(
                ui,
                &p,
                swoop_i18n::tr!("启动时自动继续"),
                Some(swoop_i18n::tr!("上次未完成的任务在启动后立即续传")),
                |ui| changes.dirty |= toggle(ui, &p, &mut self.auto_resume).changed(),
            );
            dialog::row(
                ui,
                &p,
                swoop_i18n::tr!("进度提示"),
                Some(swoop_i18n::tr!("单个下载开始时显示")),
                |ui| {
                    use crate::ui::download_card::ProgressPopup;
                    changes.dirty |= segmented(
                        ui,
                        &p,
                        &mut self.progress_popup,
                        &[
                            (ProgressPopup::Card, swoop_i18n::tr!("右下角卡片")),
                            (ProgressPopup::Window, swoop_i18n::tr!("居中窗口")),
                            (ProgressPopup::Off, swoop_i18n::tr!("关闭")),
                        ],
                    )
                    .changed();
                },
            );
        });

        dialog::group(ui, &p, Some(swoop_i18n::tr!("添加链接")), |ui| {
            dialog::row(
                ui,
                &p,
                swoop_i18n::tr!("监听剪贴板链接"),
                Some(swoop_i18n::tr!("复制链接后右下角询问是否下载")),
                |ui| changes.dirty |= toggle(ui, &p, &mut self.clipboard_watch).changed(),
            );
            dialog::row_stack(ui, &p, swoop_i18n::tr!("拖放"), None, |ui| {
                self.native_drag.settings_ui(ui, &self.store);
            });
        });

        dialog::group(ui, &p, Some(swoop_i18n::tr!("更多")), |ui| {
            use egui_phosphor::regular as i;
            if dialog::link_row(ui, &p, i::WIFI_HIGH, swoop_i18n::tr!("网络与重试"), None).clicked() {
                self.network_editor = Some(crate::ui::network::NetworkEditor::new(self.engine.network_policy()));
                self.network_bypass =
                    self.engine.network_policy().proxy.as_ref().map(|p| p.bypass.join(", ")).unwrap_or_default();
                self.network_error = None;
                changes.close = true;
            }
            if dialog::link_row(ui, &p, i::CHECK_SQUARE_OFFSET, swoop_i18n::tr!("下载完成后"), None).clicked() {
                self.completion_editor = Some(super::completion::CompletionEditor::new(&self.completion.settings));
                changes.close = true;
            }
            if dialog::link_row(ui, &p, i::GAUGE, swoop_i18n::tr!("文件下载配额"), None).clicked() {
                self.quota_editor = Some(self.engine.quota().policy);
                self.quota_error = None;
                changes.close = true;
            }
        });
    }

    fn settings_notify(&mut self, ui: &mut egui::Ui, changes: &mut Changes) {
        let p = self.palette;
        dialog::group(ui, &p, Some(swoop_i18n::tr!("系统通知")), |ui| {
            dialog::row(ui, &p, swoop_i18n::tr!("下载完成时通知"), None, |ui| {
                changes.dirty |= toggle(ui, &p, &mut self.notifications.on_complete).changed();
            });
            dialog::row(ui, &p, swoop_i18n::tr!("下载失败时通知"), None, |ui| {
                changes.dirty |= toggle(ui, &p, &mut self.notifications.on_failure).changed();
            });
            if self.notifications.on_complete || self.notifications.on_failure {
                dialog::row(ui, &p, swoop_i18n::tr!("通知时机"), None, |ui| {
                    changes.dirty |= segmented(
                        ui,
                        &p,
                        &mut self.notifications.timing,
                        &[
                            (NotificationTiming::Background, swoop_i18n::tr!("仅后台")),
                            (NotificationTiming::Always, swoop_i18n::tr!("始终")),
                        ],
                    )
                    .changed();
                });
            }
            dialog::row(ui, &p, swoop_i18n::tr!("通知声音"), None, |ui| {
                changes.dirty |= toggle(ui, &p, &mut self.notifications.sound).changed();
            });
        });
        if let Some(notifier) = &self.notifier
            && super::ghost_button(ui, &p, swoop_i18n::tr!("测试通知")).clicked()
        {
            notifier.test(self.notifications.sound);
        }
    }

    fn settings_integration(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, changes: &mut Changes) {
        let p = self.palette;
        dialog::group(ui, &p, Some(swoop_i18n::tr!("浏览器")), |ui| {
            dialog::row(
                ui,
                &p,
                swoop_i18n::tr!("浏览器集成"),
                Some(swoop_i18n::tr!("接管浏览器扩展交来的下载")),
                |ui| changes.bridge |= toggle(ui, &p, &mut self.bridge_enabled).changed(),
            );
            if self.bridge_enabled {
                dialog::row(ui, &p, swoop_i18n::tr!("接收方式"), None, |ui| {
                    changes.dirty |= segmented(
                        ui,
                        &p,
                        &mut self.bridge_mode,
                        &[
                            (BridgeMode::Silent, swoop_i18n::tr!("直接下载")),
                            (BridgeMode::Ask, swoop_i18n::tr!("弹出确认")),
                        ],
                    )
                    .changed();
                });
                dialog::row(ui, &p, swoop_i18n::tr!("端口"), None, |ui| {
                    // 输入中的文本放在 egui 内存里，失焦/回车时才生效，避免每敲一位就重绑一次端口
                    let id = ui.id().with("bridge_port_input");
                    let mut text =
                        ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| self.bridge_port.to_string());
                    let resp = ui.add(field(&mut text).desired_width(72.0));
                    if resp.lost_focus() {
                        match text.trim().parse::<u16>() {
                            Ok(port) if port >= 1024 && port != self.bridge_port => {
                                self.bridge_port = port;
                                changes.bridge = true;
                            }
                            _ => text = self.bridge_port.to_string(),
                        }
                    }
                    ui.data_mut(|d| d.insert_temp(id, text));
                    ui.add_space(4.0);
                    self.bridge_status_label(ui);
                });
            }
        });

        dialog::group(ui, &p, Some(swoop_i18n::tr!("系统")), |ui| {
            dialog::row_stack(ui, &p, swoop_i18n::tr!("登录时启动"), None, |ui| {
                let busy = self.startup.busy();
                let packaged = self.startup.state.as_ref().is_some_and(|state| state.packaged);
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        crate::ui::checkbox(ui, &mut self.startup.desired, swoop_i18n::tr!("启用"));
                        ui.add_space(8.0);
                        if ui
                            .add_enabled(!self.startup.desired || packaged, egui::Button::new(swoop_i18n::tr!("应用")))
                            .clicked()
                        {
                            self.startup.apply(ctx);
                        }
                        if ui.button(swoop_i18n::tr!("刷新")).clicked() {
                            self.startup.refresh(ctx);
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                });
                if let Some(state) = &self.startup.state {
                    dialog::note(ui, &p, &state.detail);
                    if !state.packaged {
                        dialog::note(ui, &p, swoop_i18n::tr!("从完整安装包启动后可启用"));
                    } else if state.enabled && !state.matches_current {
                        dialog::note(ui, &p, swoop_i18n::tr!("点击应用以更新启动路径"));
                    }
                }
                if let Some(error) = &self.startup.error {
                    ui.colored_label(p.danger, error);
                }
            });
            if self.tray.is_some() {
                dialog::row(
                    ui,
                    &p,
                    swoop_i18n::tr!("关闭窗口后"),
                    Some(swoop_i18n::tr!("保留在菜单栏，继续下载")),
                    |ui| changes.dirty |= toggle(ui, &p, &mut self.close_to_tray).changed(),
                );
                if cfg!(target_os = "macos") {
                    dialog::row(
                        ui,
                        &p,
                        swoop_i18n::tr!("菜单栏显示速度"),
                        Some(swoop_i18n::tr!("下载时在图标旁显示总速度")),
                        |ui| changes.dirty |= toggle(ui, &p, &mut self.tray_speed).changed(),
                    );
                }
            }
        });

        dialog::group(ui, &p, Some(swoop_i18n::tr!("站点认证")), |ui| {
            if dialog::link_row(ui, &p, egui_phosphor::regular::KEY, swoop_i18n::tr!("账号与登录方式"), None).clicked()
            {
                self.site_auth_editor = Some(crate::ui::site_auth::SiteAuthEditor::new(&self.engine));
                changes.close = true;
            }
        });
    }

    fn settings_about(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.palette;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            crate::brand::show_mark(ui, 44.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("Swoop").size(20.0).strong().color(p.text));
                ui.label(
                    RichText::new(swoop_i18n::trf!("版本 {}", env!("CARGO_PKG_VERSION")))
                        .size(12.5)
                        .color(p.text_muted),
                );
            });
        });
        ui.add_space(14.0);
        let busy = self.updates.busy();
        let idle = self.update_install_idle();
        dialog::group(ui, &p, None, |ui| {
            dialog::row(ui, &p, swoop_i18n::tr!("自动更新"), None, |ui| {
                let mut automatic = self.updates.config.automatic_check;
                if super::toggle(ui, &p, &mut automatic).changed() {
                    self.updates.set_automatic_check(automatic);
                }
            });
            dialog::row(ui, &p, swoop_i18n::tr!("软件更新"), None, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if super::ghost_button(ui, &p, swoop_i18n::tr!("检查更新")).clicked() {
                        self.updates.check(ctx);
                    }
                });
                if busy {
                    ui.spinner();
                }
                if !self.updates.message.is_empty() {
                    ui.label(RichText::new(&self.updates.message).size(12.0).color(p.text_muted));
                }
            });
        });
        if let Some(candidate) = self.updates.candidate.clone() {
            ui.add_space(8.0);
            Frame::new()
                .fill(p.group)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&candidate.release.version).size(16.0).strong().color(p.text));
                        super::pill(ui, swoop_i18n::tr!("新版本"), p.accent_soft, p.accent);
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        ui.label(RichText::new(&candidate.release.notes).color(p.text_muted));
                    });
                    ui.add_space(8.0);
                    if self.updates.archive.is_none() {
                        ui.add_enabled_ui(!busy, |ui| {
                            if super::primary_button(ui, &p, swoop_i18n::tr!("下载更新")).clicked() {
                                self.updates.download(ctx);
                            }
                        });
                    } else {
                        if !idle {
                            ui.label(
                                RichText::new(swoop_i18n::tr!("请先暂停或结束当前任务"))
                                    .size(12.0)
                                    .color(p.tint_amber),
                            );
                            ui.add_space(4.0);
                        }
                        ui.add_enabled_ui(!busy && idle, |ui| {
                            if super::primary_button(ui, &p, swoop_i18n::tr!("安装并重启")).clicked() {
                                self.begin_update_install(ctx);
                            }
                        });
                    }
                });
        }
        if let Some(error) = &self.updates.error {
            ui.add_space(8.0);
            dialog::error(ui, &p, error);
        }
    }

    /// 安装更新期间保证设置页停留在关于分类，进度与结果始终可见
    pub(crate) fn open_settings_about(&mut self, ctx: &egui::Context) {
        self.show_settings = true;
        ctx.data_mut(|d| d.insert_temp(tab_id(), Tab::About));
    }

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
