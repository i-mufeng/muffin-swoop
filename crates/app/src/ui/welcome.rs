//! 首次使用的浏览器集成引导；只在主窗口可见时启动，关闭后持久化。
use std::sync::mpsc;

use super::dialog;
use crate::app::SwoopApp;
use crate::browsers::BrowserRow;

const SEEN: &str = "browser_welcome_seen_v1";
type ScanResult = (Vec<BrowserRow>, Option<String>);

pub struct Welcome {
    pub open: bool,
    started: bool,
    pending: Option<mpsc::Receiver<ScanResult>>,
    rows: Vec<BrowserRow>,
    error: Option<String>,
}

impl Welcome {
    #[cfg(feature = "screenshot")]
    pub fn preview(&mut self) {
        self.open = true;
        self.started = true;
        self.pending = None;
        self.error = None;
        self.rows = ["edge", "chrome", "firefox"]
            .into_iter()
            .map(|id| BrowserRow {
                id: id.into(),
                name: match id {
                    "edge" => "Microsoft Edge",
                    "chrome" => "Google Chrome",
                    _ => "Mozilla Firefox",
                }
                .into(),
                version: None,
                status: if id == "edge" {
                    swoop_browser_bridge::ExtensionStatus::Installed
                } else {
                    swoop_browser_bridge::ExtensionStatus::Unknown
                },
                action: crate::browsers::RowAction::Guide,
            })
            .collect();
    }

    pub fn load(store: &swoop_core::Store) -> Self {
        Self {
            open: !crate::demo::enabled() && store.get_setting(SEEN).ok().flatten().as_deref() != Some("true"),
            started: false,
            pending: None,
            rows: Vec::new(),
            error: None,
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context, hidden: bool) {
        if !hidden && !self.started && !crate::demo::enabled() {
            self.started = true;
            let auto_edge = self.open;
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::browsers::prepare_first_run(auto_edge));
                ctx.request_repaint();
            });
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok((rows, error)) => {
                    self.rows = rows;
                    self.error = error;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some(swoop_i18n::tr!("浏览器扫描未完成，请在设置中重新扫描").into());
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

impl SwoopApp {
    pub(crate) fn welcome_ui(&mut self, ctx: &egui::Context) {
        if !self.welcome.open {
            return;
        }
        let p = self.palette;
        let mut close = false;
        let mut install = false;
        let modal = dialog::modal("browser_welcome", &p).show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ctx.content_rect().width() - 48.0));
            // 欢迎页使用品牌标题区，正文仍沿用统一的可滚动主体和固定底栏。
            egui::Frame::new().inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 28.0).max(0.0));
                    let response = super::icon_button(ui, &p, egui_phosphor::regular::X, swoop_i18n::tr!("关闭"));
                    close |= accessible_button(ui, &p, response, swoop_i18n::tr!("关闭"));
                });
                ui.vertical_centered(|ui| {
                    crate::brand::show_mark(ui, 56.0);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(swoop_i18n::tr!("欢迎使用 Swoop")).size(24.0).strong().color(p.text));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(swoop_i18n::tr!("把浏览器下载交给 Swoop")).size(13.0).color(p.text_muted),
                    );
                    ui.add_space(8.0);
                });
            });
            dialog::body(ui, "browser_welcome_body", |ui| {
                if self.welcome.pending.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(swoop_i18n::tr!("正在准备浏览器扩展…"));
                    });
                } else {
                    if !self.welcome.rows.is_empty() {
                        dialog::group(ui, &p, None, |ui| {
                            for row in &self.welcome.rows {
                                let automatic =
                                    row.id == "edge" && row.status == swoop_browser_bridge::ExtensionStatus::Installed;
                                dialog::row_with(
                                    ui,
                                    &p,
                                    &row.name,
                                    Some(browser_hint(&row.id, row.status)),
                                    0.72,
                                    |ui| {
                                        let (label, bg, fg) = if automatic {
                                            (swoop_i18n::tr!("自动安装"), p.success_soft, p.success)
                                        } else {
                                            (swoop_i18n::tr!("手动安装"), p.inset, p.text_muted)
                                        };
                                        super::pill(ui, label, bg, fg);
                                    },
                                );
                            }
                        });
                    } else {
                        dialog::note(ui, &p, swoop_i18n::tr!("未检测到浏览器，可在设置中重新扫描。"));
                    }
                    if let Some(error) = &self.welcome.error {
                        egui::CollapsingHeader::new(swoop_i18n::tr!("自动安装未完成，可在设置中重试")).show(ui, |ui| {
                            dialog::note(ui, &p, error);
                        });
                    }
                }
                ui.add_space(4.0);
                dialog::note(ui, &p, swoop_i18n::tr!("随时可在设置 → 集成中管理扩展。"));
                ui.add_space(8.0);
            });
            dialog::footer(ui, &p, |ui| {
                let response = super::primary_button(ui, &p, swoop_i18n::tr!("安装扩展"));
                install = accessible_button(ui, &p, response, swoop_i18n::tr!("安装扩展"));
                let response = super::ghost_button(ui, &p, swoop_i18n::tr!("开始使用"));
                close |= accessible_button(ui, &p, response, swoop_i18n::tr!("开始使用"));
            });
        });
        if close || install || modal.should_close() {
            self.welcome.open = false;
            if let Err(error) = self.store.set_setting(SEEN, "true") {
                self.toasts.push_error(swoop_i18n::tr!("保存设置失败"), error.to_string());
            }
        }
        if install {
            self.open_settings_integration(ctx);
        }
    }
}

fn accessible_button(ui: &egui::Ui, p: &crate::theme::Palette, response: egui::Response, label: &str) -> bool {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            crate::theme::R_CTRL,
            egui::Stroke::new(1.5, p.accent),
            egui::StrokeKind::Outside,
        );
    }
    response.clicked()
}

fn browser_hint(id: &str, status: swoop_browser_bridge::ExtensionStatus) -> &'static str {
    match id {
        "edge" if status == swoop_browser_bridge::ExtensionStatus::Installed => {
            swoop_i18n::tr!("已登记，重启浏览器后生效")
        }
        "edge" => swoop_i18n::tr!("前往设置重试，或从商店安装"),
        "chrome" => swoop_i18n::tr!("开启开发者模式，加载扩展文件夹"),
        "firefox" => swoop_i18n::tr!("选择 Mozilla 签名的扩展包"),
        _ => swoop_i18n::tr!("前往设置，按引导完成安装"),
    }
}
