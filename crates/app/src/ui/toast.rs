//! 右下角吐司：剪贴板链接询问、下载完成提示。带淡入滑入/淡出动画，悬停时不计时。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::epaint::Shadow;
use egui::{
    Align, Align2, Area, Color32, CornerRadius, Frame, Id, Label, Layout, Margin, Order, RichText, Sense, Stroke, vec2,
};
use swoop_core::AddOptions;

use super::{ghost_button, primary_button};
use crate::app::SwoopApp;
use crate::completion::FileAction;
use crate::fonts;
use crate::theme::R_CARD;

pub enum ToastKind {
    /// 剪贴板里检测到的链接，带「下载 / 忽略」
    Links(Vec<String>),
    /// 下载完成，带「打开 / 打开文件夹」
    Completed(PathBuf),
    /// 普通提示
    Info,
    Error,
}

pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub body: String,
    pub created: Instant,
    pub ttl: Duration,
    pub closing: bool,
}

#[derive(Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
    next_id: u64,
}

impl Toasts {
    fn push(&mut self, kind: ToastKind, title: String, body: String, ttl: Duration) {
        self.next_id += 1;
        self.items.push(Toast { id: self.next_id, kind, title, body, created: Instant::now(), ttl, closing: false });
    }

    /// 已有未关闭的链接吐司时合并进去，避免连续复制时弹一排
    pub fn push_links(&mut self, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        if let Some(t) = self.items.iter_mut().find(|t| matches!(t.kind, ToastKind::Links(_)) && !t.closing)
            && let ToastKind::Links(list) = &mut t.kind
        {
            for u in urls {
                if !list.contains(&u) {
                    list.push(u);
                }
            }
            t.title = swoop_i18n::trf!("检测到 {} 个链接", list.len());
            t.body = list.last().cloned().unwrap_or_default();
            t.created = Instant::now();
            return;
        }
        let title = if urls.len() == 1 {
            swoop_i18n::tr!("检测到链接").to_string()
        } else {
            swoop_i18n::trf!("检测到 {} 个链接", urls.len())
        };
        let body = urls[0].clone();
        self.push(ToastKind::Links(urls), title, body, Duration::from_secs(12));
    }

    pub fn push_info(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Info, title.into(), body.into(), Duration::from_secs(5));
    }

    pub fn push_completed(&mut self, title: impl Into<String>, body: impl Into<String>, path: PathBuf) {
        self.push(ToastKind::Completed(path), title.into(), body.into(), Duration::from_secs(8));
    }

    pub fn push_error(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Error, title.into(), body.into(), Duration::from_secs(8));
    }
}

enum Action {
    Download(u64, Vec<String>),
    Open(u64, PathBuf, FileAction),
    Close(u64),
}

impl SwoopApp {
    pub fn toasts_ui(&mut self, ctx: &egui::Context) {
        if self.toasts.items.is_empty() {
            return;
        }
        let p = self.palette;
        let now = Instant::now();
        for t in &mut self.toasts.items {
            if !t.closing && now.duration_since(t.created) > t.ttl {
                t.closing = true;
            }
        }

        let mut actions: Vec<Action> = Vec::new();
        let mut hovered: Vec<u64> = Vec::new();
        let width = 360.0;

        Area::new(Id::new("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -48.0)).order(Order::Foreground).show(
            ctx,
            |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 8.0;
                for t in &self.toasts.items {
                    let anim = ctx.animate_bool_with_time(Id::new(("toast", t.id)), !t.closing, super::anim_secs(0.18));
                    if anim <= 0.001 {
                        continue;
                    }
                    ui.scope(|ui| {
                        ui.set_opacity(anim);
                        ui.horizontal(|ui| {
                            ui.add_space((1.0 - anim) * 28.0);
                            let frame = Frame::new()
                                .fill(p.surface)
                                .stroke(Stroke::new(1.0, p.border))
                                .corner_radius(CornerRadius::same(R_CARD))
                                .inner_margin(Margin::same(14))
                                .shadow(Shadow {
                                    offset: [0, 8],
                                    blur: 24,
                                    spread: 0,
                                    color: Color32::from_black_alpha(90),
                                });
                            let resp = frame.show(ui, |ui| {
                                ui.set_width(width - 28.0);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    let (icon, color) = match t.kind {
                                        ToastKind::Links(_) => (egui_phosphor::fill::LINK, p.accent),
                                        ToastKind::Completed(_) | ToastKind::Info => {
                                            (egui_phosphor::fill::CHECK_CIRCLE, p.success)
                                        }
                                        ToastKind::Error => (egui_phosphor::fill::WARNING_CIRCLE, p.danger),
                                    };
                                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                                    let painter = ui.painter();
                                    painter.rect_filled(rect, CornerRadius::same(8), p.soft(color));
                                    painter.text(
                                        rect.center(),
                                        Align2::CENTER_CENTER,
                                        icon,
                                        fonts::icon_fill(16.0),
                                        color,
                                    );

                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.add(
                                            Label::new(RichText::new(&t.title).size(13.5).strong().color(p.text))
                                                .truncate(),
                                        );
                                        ui.add(
                                            Label::new(RichText::new(&t.body).size(12.0).color(p.text_muted))
                                                .truncate(),
                                        );
                                    });
                                });
                                match &t.kind {
                                    ToastKind::Links(urls) => {
                                        ui.add_space(10.0);
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if primary_button(ui, &p, swoop_i18n::tr!("下载")).clicked() {
                                                actions.push(Action::Download(t.id, urls.clone()));
                                            }
                                            if ghost_button(ui, &p, swoop_i18n::tr!("忽略")).clicked() {
                                                actions.push(Action::Close(t.id));
                                            }
                                        });
                                    }
                                    ToastKind::Completed(path) => {
                                        ui.add_space(10.0);
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if primary_button(ui, &p, swoop_i18n::tr!("打开")).clicked() {
                                                actions.push(Action::Open(t.id, path.clone(), FileAction::Open));
                                            }
                                            if ghost_button(ui, &p, swoop_i18n::tr!("打开文件夹")).clicked() {
                                                actions.push(Action::Open(t.id, path.clone(), FileAction::Reveal));
                                            }
                                        });
                                    }
                                    ToastKind::Info | ToastKind::Error => {}
                                }
                            });
                            if resp.response.hovered() {
                                hovered.push(t.id);
                            }
                        });
                    });
                }
            },
        );

        // 悬停时暂停倒计时
        for t in &mut self.toasts.items {
            if hovered.contains(&t.id) && !t.closing {
                t.created = now;
            }
        }
        for a in actions {
            match a {
                Action::Download(id, urls) => {
                    self.add_urls(ctx, urls);
                    self.close_toast(id);
                }
                Action::Open(id, path, action) => {
                    self.close_toast(id);
                    if let Err(error) = self.completion.open_path(path, action) {
                        self.toasts.push_error(swoop_i18n::tr!("无法打开"), error);
                    }
                }
                Action::Close(id) => self.close_toast(id),
            }
        }
        // 淡出完成后移除
        self.toasts.items.retain(|t| {
            !(t.closing && ctx.animate_bool_with_time(Id::new(("toast", t.id)), false, super::anim_secs(0.18)) <= 0.001)
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn close_toast(&mut self, id: u64) {
        if let Some(t) = self.toasts.items.iter_mut().find(|t| t.id == id) {
            t.closing = true;
        }
    }

    /// 按默认目录（启用分类时按分类子目录）添加一组链接；失败的汇总成一条错误吐司
    pub fn add_urls(&mut self, ctx: &egui::Context, urls: Vec<String>) {
        let mut failed = Vec::new();
        let single = urls.len() == 1;
        for url in urls {
            let dir = self.categories.target(&self.default_dir, &url, None);
            match self.engine.add(AddOptions { url: url.clone(), dir, file_name: None, headers: Vec::new() }) {
                Ok(id) if single => {
                    self.show_progress(ctx, id, None);
                }
                Ok(_) => {}
                Err(e) => failed.push(format!("{url}: {e}")),
            }
        }
        if !failed.is_empty() {
            self.toasts.push_error(swoop_i18n::tr!("部分链接未能添加"), failed.join("\n"));
        }
    }
}
