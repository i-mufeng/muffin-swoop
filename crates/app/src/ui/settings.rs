use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui::{Align, Frame, Layout, Margin, RichText};

use super::{primary_button, toggle};
use crate::app::SwoopApp;
use crate::bridge::BridgeMode;
use crate::theme::Mode;

impl SwoopApp {
    pub fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let p = self.palette;
        let mut close = false;
        let mut dirty = false;
        let mut bridge_changed = false;

        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(RichText::new("设置").size(18.0).strong().color(p.text));
                ui.add_space(14.0);

                egui::Grid::new("settings_grid").num_columns(2).spacing([18.0, 14.0]).show(ui, |ui| {
                    ui.label(RichText::new("默认保存目录").color(p.text_muted));
                    let mut dir = self.default_dir.display().to_string();
                    if ui
                        .add(egui::TextEdit::singleline(&mut dir).desired_width(300.0).margin(Margin::symmetric(10, 6)))
                        .changed()
                    {
                        self.default_dir = PathBuf::from(&dir);
                        self.add_dialog.dir = dir;
                        dirty = true;
                    }
                    ui.end_row();

                    ui.label(RichText::new("外观").color(p.text_muted));
                    ui.horizontal(|ui| {
                        if ui.selectable_label(p.mode == Mode::Dark, "深色").clicked() {
                            self.set_theme(ctx, Mode::Dark);
                        }
                        if ui.selectable_label(p.mode == Mode::Light, "浅色").clicked() {
                            self.set_theme(ctx, Mode::Light);
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("进度条样式").color(p.text_muted));
                    ui.horizontal(|ui| {
                        use super::progress::ProgressStyle as S;
                        for (s, label) in [(S::Cells, "像素格"), (S::Lanes, "轨道"), (S::Overlay, "叠加")] {
                            if ui.selectable_label(self.progress_style == s, label).clicked() {
                                self.progress_style = s;
                                dirty = true;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("监听剪贴板链接").color(p.text_muted));
                    ui.horizontal(|ui| {
                        if toggle(ui, &p, &mut self.clipboard_watch).changed() {
                            dirty = true;
                        }
                        ui.label(RichText::new("复制链接后右下角询问是否下载").size(12.0).color(p.text_muted));
                    });
                    ui.end_row();

                    ui.label(RichText::new("启动时自动继续").color(p.text_muted));
                    ui.horizontal(|ui| {
                        if toggle(ui, &p, &mut self.auto_resume).changed() {
                            dirty = true;
                        }
                        ui.label(RichText::new("上次未完成的任务在启动后立即续传").size(12.0).color(p.text_muted));
                    });
                    ui.end_row();

                    if self.tray.is_some() {
                        ui.label(RichText::new("关闭窗口后").color(p.text_muted));
                        ui.horizontal(|ui| {
                            if toggle(ui, &p, &mut self.close_to_tray).changed() {
                                dirty = true;
                            }
                            ui.label(RichText::new("保留在菜单栏，继续下载").size(12.0).color(p.text_muted));
                        });
                        ui.end_row();

                        if cfg!(target_os = "macos") {
                            ui.label(RichText::new("菜单栏显示速度").color(p.text_muted));
                            ui.horizontal(|ui| {
                                if toggle(ui, &p, &mut self.tray_speed).changed() {
                                    dirty = true;
                                }
                                ui.label(RichText::new("下载时在图标旁显示总速度").size(12.0).color(p.text_muted));
                            });
                            ui.end_row();
                        }
                    }

                    ui.label(RichText::new("浏览器集成").color(p.text_muted));
                    ui.horizontal(|ui| {
                        if toggle(ui, &p, &mut self.bridge_enabled).changed() {
                            bridge_changed = true;
                        }
                        ui.label(RichText::new("接管浏览器扩展交来的下载").size(12.0).color(p.text_muted));
                    });
                    ui.end_row();

                    if self.bridge_enabled {
                        ui.label(RichText::new("接收方式").color(p.text_muted));
                        ui.horizontal(|ui| {
                            for (m, label) in [(BridgeMode::Silent, "直接下载"), (BridgeMode::Ask, "弹出确认")] {
                                if ui.selectable_label(self.bridge_mode == m, label).clicked() {
                                    self.bridge_mode = m;
                                    dirty = true;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(RichText::new("端口").color(p.text_muted));
                        ui.horizontal(|ui| {
                            // 输入中的文本放在 egui 内存里，失焦/回车时才生效，避免每敲一位就重绑一次端口
                            let id = ui.id().with("bridge_port_input");
                            let mut text =
                                ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| self.bridge_port.to_string());
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut text).desired_width(72.0).margin(Margin::symmetric(10, 6)),
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

                    ui.label(RichText::new("每任务连接数").color(p.text_muted));
                    ui.label(format!("{}（下个版本支持修改）", self.engine.config().connections));
                    ui.end_row();

                    ui.label(RichText::new("版本").color(p.text_muted));
                    ui.label(format!("Swoop {}", env!("CARGO_PKG_VERSION")));
                    ui.end_row();
                });

                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, &p, "完成").clicked() {
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
            (None, Some(err)) => (format!("未运行：{err}"), p.danger),
            (None, None) => ("未运行".to_string(), p.text_muted),
            (Some(_), _) => match self.bridge.last_ping() {
                // 扩展每 30 秒心跳一次，留一倍余量
                Some(t) if SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(70) => {
                    ("扩展已连接".to_string(), p.success)
                }
                Some(t) => (format!("扩展 {} 连接过", super::format::ago(t)), p.text_muted),
                None => ("运行中，尚未检测到扩展".to_string(), p.text_muted),
            },
        };
        ui.label(RichText::new(text).size(12.0).color(color));
    }
}
