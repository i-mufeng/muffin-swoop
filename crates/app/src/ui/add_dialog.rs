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
    browser_names: HashMap<String, String>,
    /// 来自浏览器时在标题下显示的一行说明
    browser_note: Option<String>,
    preview: Option<Result<Vec<String>, String>>,
    preview_limit: usize,
    pub queue: swoop_core::QueueId,
    pub ftp: crate::ftp_settings::FtpEditor,
    import: Option<std::sync::mpsc::Receiver<Result<Option<String>, String>>>,
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
            browser_names: HashMap::new(),
            browser_note: None,
            preview: None,
            preview_limit: 0,
            queue: swoop_core::MAIN_QUEUE_ID,
            ftp: crate::ftp_settings::FtpEditor::default(),
            import: None,
        }
    }

    /// 询问模式：浏览器交来的下载，预填链接并记住它的请求头
    pub fn open_from_browser(
        &mut self,
        url: String,
        file_name: &str,
        size: Option<u64>,
        headers: Vec<(String, String)>,
    ) {
        self.queue = swoop_core::MAIN_QUEUE_ID;
        let size = size.map(|s| format!(" · {}", super::format::bytes(s))).unwrap_or_default();
        self.browser_note = Some(swoop_i18n::trf!("来自浏览器：{file_name}{size}"));
        let key =
            crate::batch::parse(&url, 1).ok().and_then(|urls| urls.into_iter().next()).unwrap_or_else(|| url.clone());
        self.browser_names.insert(key.clone(), file_name.to_string());
        self.browser_headers.insert(key, headers);
        self.open_with(Some(url));
    }

    fn close(&mut self) {
        self.open = false;
        self.error = None;
        self.browser_headers.clear();
        self.browser_names.clear();
        self.browser_note = None;
        self.ftp.clear_credentials();
    }

    fn text_changed(&mut self) {
        self.ftp.clear_credentials();
        self.preview = None;
        self.error = None;
    }

    fn retain_failed(&mut self, retry: Vec<String>, failed: Vec<String>) {
        // 成功项移出草稿后，剩余链接不得继承其 FTP 登录。
        self.ftp.clear_credentials();
        self.text = retry.join("\n");
        self.preview = None;
        self.error = Some(swoop_i18n::trf!("以下链接未能添加：\n{}", failed.join("\n")));
    }

    pub fn open_with(&mut self, prefill: Option<String>) {
        self.open = true;
        self.want_focus = true;
        self.error = None;
        self.preview = None;
        if let Some(t) = prefill {
            // A newly supplied target must never inherit a login typed for an earlier draft.
            self.ftp.clear_credentials();
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
    pub(crate) fn start_dropped_files(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if self.add_dialog.import.is_some() {
            self.toasts.push_error(swoop_i18n::tr!("导入未开始"), swoop_i18n::tr!("另一批链接正在导入"));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let limit = self.batch_limit;
        match std::thread::Builder::new().name("swoop-drop-import".into()).spawn(move || {
            let result = crate::link_import::read_link_files(&paths, limit).map(Some);
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => self.add_dialog.import = Some(rx),
            Err(error) => self.toasts.push_error(swoop_i18n::tr!("导入未开始"), error.to_string()),
        }
    }

    pub(crate) fn start_import(&mut self, ctx: &egui::Context, path: Option<PathBuf>) {
        if self.add_dialog.import.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let limit = self.batch_limit;
        match std::thread::Builder::new().name("swoop-link-import".into()).spawn(move || {
            let result = (|| {
                let path = match path {
                    Some(path) => path,
                    None => {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| e.to_string())?;
                        let Some(file) = rt.block_on(
                            rfd::AsyncFileDialog::new()
                                .set_title(swoop_i18n::tr!("导入下载链接"))
                                .add_filter(swoop_i18n::tr!("链接文件"), &["txt", "url"])
                                .pick_file(),
                        ) else {
                            return Ok(None);
                        };
                        file.path().to_path_buf()
                    }
                };
                crate::link_import::read_links(&path, limit).map(Some)
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => self.add_dialog.import = Some(rx),
            Err(error) => self.toasts.push_error(swoop_i18n::tr!("导入失败"), error.to_string()),
        }
    }

    pub fn poll_import(&mut self, _ctx: &egui::Context) {
        let result = self.add_dialog.import.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(swoop_i18n::tr!("导入任务中断").into())),
        });
        if let Some(result) = result {
            self.add_dialog.import = None;
            match result {
                Ok(Some(text)) => self.add_dialog.open_with(Some(text)),
                Ok(None) => {}
                Err(error) => self.toasts.push_error(swoop_i18n::tr!("导入失败"), error),
            }
        }
    }

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
                ui.label(RichText::new(swoop_i18n::tr!("新建下载")).size(18.0).strong().color(p.text));
                if ui
                    .add_enabled(self.add_dialog.import.is_none(), egui::Button::new(swoop_i18n::tr!("导入链接文件…")))
                    .clicked()
                {
                    self.start_import(ctx, None);
                }
                ui.label(
                    RichText::new(swoop_i18n::tr!("每行一个链接，支持 {001..010}、{a..z} 批量展开"))
                        .size(12.5)
                        .color(p.text_muted),
                );
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
                if resp.changed() {
                    self.add_dialog.text_changed();
                }
                if self.add_dialog.want_focus {
                    resp.request_focus();
                    self.add_dialog.want_focus = false;
                }

                if !self.add_dialog.text.trim().is_empty() {
                    if self.add_dialog.preview.is_none() || self.add_dialog.preview_limit != self.batch_limit {
                        self.add_dialog.preview = Some(crate::batch::parse(&self.add_dialog.text, self.batch_limit));
                        self.add_dialog.preview_limit = self.batch_limit;
                    }
                    match self.add_dialog.preview.as_ref().unwrap() {
                        Ok(urls) => {
                            egui::CollapsingHeader::new(swoop_i18n::trf!("{} 个链接 · 查看预览", urls.len())).show(
                                ui,
                                |ui| {
                                    egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                                        for url in urls.iter().take(100) {
                                            ui.add(egui::Label::new(url).truncate());
                                        }
                                        if urls.len() > 100 {
                                            ui.label(swoop_i18n::trf!("另有 {} 个链接", urls.len() - 100));
                                        }
                                    });
                                },
                            );
                        }
                        Err(error) => {
                            ui.label(RichText::new(error).size(12.0).color(p.danger));
                        }
                    }
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(swoop_i18n::tr!("保存到")).color(p.text_muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_dialog.dir)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(10, 6)),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new(swoop_i18n::tr!("下载队列")).color(p.text_muted));
                    let queues = self.engine.queues();
                    if !queues.iter().any(|q| q.id == self.add_dialog.queue) {
                        self.add_dialog.queue = swoop_core::MAIN_QUEUE_ID;
                    }
                    let selected = queues.iter().find(|q| q.id == self.add_dialog.queue);
                    egui::ComboBox::from_id_salt("add_queue")
                        .selected_text(selected.map(super::queue_name).unwrap_or(swoop_i18n::tr!("主队列")))
                        .show_ui(ui, |ui| {
                            for q in &queues {
                                ui.selectable_value(&mut self.add_dialog.queue, q.id, super::queue_name(q));
                            }
                        });
                    if selected.is_some_and(|q| !q.running) {
                        ui.label(RichText::new(swoop_i18n::tr!("加入后等待队列启动")).size(12.0).color(p.text_muted));
                    }
                });

                if self.categories.enabled {
                    ui.label(RichText::new(swoop_i18n::tr!("已启用分类保存规则")).size(12.0).color(p.text_muted));
                    if let Some(Ok(urls)) = &self.add_dialog.preview {
                        let base = expand_tilde(&self.add_dialog.dir);
                        for url in urls.iter().take(3) {
                            let target = self.categories.target(&base, url, None);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(target.display().to_string()).size(12.0).color(p.text_muted),
                                )
                                .truncate(),
                            );
                        }
                    }
                }

                if let Some(Ok(urls)) = &self.add_dialog.preview
                    && urls.iter().any(|url| crate::ftp_settings::is_ftp(url))
                {
                    let implicit =
                        urls.iter().filter(|url| crate::ftp_settings::is_ftp(url)).all(|url| url.starts_with("ftps:"));
                    egui::ScrollArea::vertical().id_salt("add_ftp_settings").max_height(230.0).show(ui, |ui| {
                        self.add_dialog.ftp.ui(ui, implicit);
                        if ui
                            .small_button(swoop_i18n::tr!("记住连接设置"))
                            .on_hover_text(swoop_i18n::tr!("用户名和密码不保存为默认值"))
                            .clicked()
                        {
                            match self.add_dialog.ftp.save_defaults(&self.engine) {
                                Ok(()) => self
                                    .toasts
                                    .push_info(swoop_i18n::tr!("FTP 设置已保存"), swoop_i18n::tr!("用于后续新建下载")),
                                Err(error) => self.add_dialog.error = Some(error),
                            }
                        }
                    });
                }

                if let Some(err) = &self.add_dialog.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).size(12.5).color(p.danger));
                }

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(swoop_i18n::tr!("⌘↩ 开始  ·  Esc 取消")).size(12.0).color(p.text_muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &p, swoop_i18n::tr!("开始下载")).clicked() {
                            submit = true;
                        }
                        if ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked() {
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
        let urls = match crate::batch::parse(&self.add_dialog.text, self.batch_limit) {
            Ok(urls) => urls,
            Err(error) => {
                self.add_dialog.error = Some(error);
                return;
            }
        };

        if let Err(error) = self.add_dialog.ftp.validate_batch(&urls) {
            self.add_dialog.error = Some(error);
            return;
        }

        let mut failed = Vec::new();
        let mut retry = Vec::new();
        for url in urls {
            let headers = self.add_dialog.browser_headers.get(&url).cloned().unwrap_or_default();
            let file_name = self.add_dialog.browser_names.get(&url).cloned();
            let target_dir = self.categories.target(&dir, &url, file_name.as_deref());
            let options = AddOptions { url: url.clone(), dir: target_dir, file_name, headers };
            let result = if crate::ftp_settings::is_ftp(&url) {
                self.add_dialog.ftp.prepare(&url).and_then(|(ftp, credentials)| {
                    self.engine.enqueue_ftp(options, self.add_dialog.queue, ftp, credentials).map_err(|e| e.to_string())
                })
            } else {
                self.engine.enqueue(options, self.add_dialog.queue).map_err(|e| e.to_string())
            };
            if let Err(e) = result {
                failed.push(format!("{url}\n  {e}"));
                retry.push(url);
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
            // 保留失败项，用户再次提交时不会重复创建之前成功的任务。
            self.add_dialog.retain_failed(retry, failed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoop_core::{FtpProxyPolicy, FtpTlsMode};

    fn private_draft() -> AddDialog {
        let mut dialog = AddDialog::new(Path::new("."));
        dialog.text = "ftp://first.example.test/private.bin".into();
        dialog.ftp.username = "fixture-user".into();
        dialog.ftp.password = "fixture-password".into();
        dialog.ftp.options.username = Some("fixture-user".into());
        dialog.ftp.options.proxy = FtpProxyPolicy::Direct;
        dialog.ftp.options.tls = FtpTlsMode::Explicit;
        dialog.preview = Some(Ok(vec![dialog.text.clone()]));
        dialog.error = Some("previous draft error".into());
        dialog
    }

    fn assert_no_credentials_for_other_server(dialog: &AddDialog) {
        assert!(dialog.ftp.username.is_empty());
        assert!(dialog.ftp.password.is_empty());
        assert!(dialog.ftp.options.username.is_none());
        let target = "ftp://second.example.test/private.bin";
        dialog.ftp.validate_batch(&[target.into()]).unwrap();
        let (options, credentials) = dialog.ftp.prepare(target).unwrap();
        assert!(options.username.is_none());
        assert!(credentials.is_none());
        assert_eq!(options.proxy, FtpProxyPolicy::Direct);
        assert_eq!(options.tls, FtpTlsMode::Explicit);
    }

    #[test]
    fn editing_download_text_clears_previous_login_and_cached_preview() {
        let mut dialog = private_draft();
        dialog.text = "ftp://second.example.test/private.bin".into();
        dialog.text_changed();
        assert_no_credentials_for_other_server(&dialog);
        assert_eq!(dialog.text, "ftp://second.example.test/private.bin");
        assert!(dialog.preview.is_none());
        assert!(dialog.error.is_none());
    }

    #[test]
    fn prefilled_target_clears_login_while_preserving_existing_draft_urls() {
        let mut dialog = private_draft();
        dialog.open_with(Some("ftp://second.example.test/private.bin".into()));
        assert_no_credentials_for_other_server(&dialog);
        assert!(dialog.open && dialog.want_focus);
        assert_eq!(dialog.text.lines().count(), 2);
        assert!(dialog.text.starts_with("ftp://first.example.test/"));
        assert!(dialog.text.ends_with("ftp://second.example.test/private.bin"));
        assert!(dialog.preview.is_none());
        assert!(dialog.error.is_none());
    }

    #[test]
    fn partial_failure_keeps_only_retry_urls_and_discards_successful_ftp_login() {
        let mut dialog = private_draft();
        let failed_url = "https://remaining.example.test/file.bin";
        dialog.text.push_str(&format!("\n{failed_url}"));
        dialog.retain_failed(vec![failed_url.into()], vec![format!("{failed_url}\n  injected insert failure")]);
        assert_no_credentials_for_other_server(&dialog);
        assert_eq!(dialog.text, failed_url);
        assert!(dialog.preview.is_none());
        assert!(dialog.error.as_deref().unwrap().contains("injected insert failure"));
        assert!(!dialog.text.contains("first.example.test"), "successful tasks must not be submitted twice");
    }
}
