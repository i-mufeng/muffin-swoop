use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Align, Frame, Key, Layout, Margin, Modifiers, RichText};
use swoop_core::AddOptions;

use super::{ghost_button, primary_button};
use crate::app::SwoopApp;

pub struct AddDialog {
    pub open: bool,
    pub text: String,
    pub dir: String,
    pub error: Option<String>,
    want_focus: bool,
    /// 浏览器交来的链接各自携带的请求头（Cookie 等），只对完全相同的 URL 生效，不会带给手输的其它链接
    browser_headers: HashMap<String, Vec<(String, String)>>,
    /// 来自浏览器时在标题下显示的一行说明
    browser_note: Option<String>,
}

impl AddDialog {
    pub fn new(dir: &Path) -> Self {
        Self {
            open: false,
            text: String::new(),
            dir: dir.display().to_string(),
            error: None,
            want_focus: false,
            browser_headers: HashMap::new(),
            browser_note: None,
        }
    }

    /// 询问模式：浏览器交来的下载，预填链接并记住它的请求头
    pub fn open_from_browser(&mut self, url: String, file_name: &str, size: Option<u64>, headers: Vec<(String, String)>) {
        let size = size.map(|s| format!(" · {}", super::format::bytes(s))).unwrap_or_default();
        self.browser_note = Some(format!("来自浏览器：{file_name}{size}"));
        self.browser_headers.insert(url.clone(), headers);
        self.open_with(Some(url));
    }

    fn close(&mut self) {
        self.open = false;
        self.error = None;
        self.browser_headers.clear();
        self.browser_note = None;
    }

    pub fn open_with(&mut self, prefill: Option<String>) {
        self.open = true;
        self.want_focus = true;
        self.error = None;
        if let Some(t) = prefill {
            if self.text.trim().is_empty() {
                self.text = t;
            } else {
                self.text.push('\n');
                self.text.push_str(&t);
            }
        }
    }
}

/// 把开头的 `~` 展开为用户目录
pub fn expand_tilde(s: &str) -> PathBuf {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('~')
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest.trim_start_matches('/'));
    }
    PathBuf::from(s)
}

impl SwoopApp {
    pub fn add_dialog_ui(&mut self, ctx: &egui::Context) {
        if !self.add_dialog.open {
            return;
        }
        let p = self.palette;
        let mut submit = false;
        let mut cancel = false;

        let modal = egui::Modal::new(egui::Id::new("add_dialog"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new("新建下载").size(18.0).strong().color(p.text));
                ui.label(RichText::new("支持一次添加多个链接，每行一个").size(12.5).color(p.text_muted));
                if let Some(note) = &self.add_dialog.browser_note {
                    ui.add_space(4.0);
                    ui.label(RichText::new(note).size(12.5).color(p.accent));
                }
                ui.add_space(12.0);

                let resp = ui.add(
                    egui::TextEdit::multiline(&mut self.add_dialog.text)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://example.com/file.zip")
                        .font(egui::TextStyle::Monospace)
                        .margin(Margin::same(10)),
                );
                if self.add_dialog.want_focus {
                    resp.request_focus();
                    self.add_dialog.want_focus = false;
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("保存到").color(p.text_muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_dialog.dir)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(10, 6)),
                    );
                });

                if let Some(err) = &self.add_dialog.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).size(12.5).color(p.danger));
                }

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⌘↩ 开始  ·  Esc 取消").size(12.0).color(p.text_muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &p, "开始下载").clicked() {
                            submit = true;
                        }
                        if ghost_button(ui, &p, "取消").clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if modal.should_close() {
            cancel = true;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            submit = true;
        }

        if cancel {
            self.add_dialog.close();
        } else if submit {
            self.submit_add();
        }
    }

    fn submit_add(&mut self) {
        let dir = expand_tilde(&self.add_dialog.dir);
        let urls: Vec<String> =
            self.add_dialog.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
        if urls.is_empty() {
            self.add_dialog.error = Some("请至少输入一个链接".into());
            return;
        }

        let mut failed = Vec::new();
        for url in urls {
            let headers = self.add_dialog.browser_headers.get(&url).cloned().unwrap_or_default();
            if let Err(e) = self.engine.add(AddOptions { url: url.clone(), dir: dir.clone(), file_name: None, headers }) {
                failed.push(format!("{url}\n  {e}"));
            }
        }
        if self.default_dir != dir {
            self.default_dir = dir;
            self.save_settings();
        }

        if failed.is_empty() {
            self.add_dialog.close();
            self.add_dialog.text.clear();
        } else {
            self.add_dialog.error = Some(format!("以下链接未能添加：\n{}", failed.join("\n")));
        }
    }
}
