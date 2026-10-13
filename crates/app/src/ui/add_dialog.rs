use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{Align, Key, Layout, Margin, Modifiers, RichText};
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
    /// 浏览器交来、Swoop 还在探测的链接：桥接令牌 →（原样链接，预览里的键）；探测完之前不能开始下载
    browser_pending: HashMap<u64, (String, String)>,
    /// 收到浏览器下载的时刻，窗口真正画出来时记一条耗时日志
    shown_since: Option<Instant>,
    preview: Option<Result<Vec<String>, String>>,
    preview_limit: usize,
    pub queue: swoop_core::QueueId,
    pub ftp: crate::ftp_settings::FtpEditor,
    import: Option<std::sync::mpsc::Receiver<Result<Option<String>, String>>>,
    /// 独立窗口的位置（打开时按屏幕居中算一次）；`raise` 让已打开的窗口回到前台
    pos: Option<egui::Pos2>,
    raise: bool,
}

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("swoop-add-dialog")
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
            browser_pending: HashMap::new(),
            shown_since: None,
            preview: None,
            preview_limit: 0,
            queue: swoop_core::MAIN_QUEUE_ID,
            ftp: crate::ftp_settings::FtpEditor::default(),
            import: None,
            pos: None,
            raise: false,
        }
    }

    /// 询问模式：浏览器交来下载，Swoop 开始探测前就弹出对话框，预填链接并记住它的请求头
    pub fn begin_from_browser(&mut self, token: u64, url: String, headers: Vec<(String, String)>) {
        self.queue = swoop_core::MAIN_QUEUE_ID;
        self.browser_note = Some(swoop_i18n::tr!("来自浏览器 · 正在获取文件信息…").into());
        let key =
            crate::batch::parse(&url, 1).ok().and_then(|urls| urls.into_iter().next()).unwrap_or_else(|| url.clone());
        self.browser_headers.insert(key.clone(), headers);
        self.open_with(Some(url.clone()));
        self.browser_pending.insert(token, (url, key));
        self.shown_since.get_or_insert_with(Instant::now);
    }

    /// 探测成功：补上文件名和大小。对话框已被关掉（令牌不在了）就什么都不做
    pub fn finish_from_browser(&mut self, token: u64, file_name: &str, size: Option<u64>) {
        let Some((_, key)) = self.browser_pending.remove(&token) else { return };
        let size = size.map(|s| format!(" · {}", super::format::bytes(s))).unwrap_or_default();
        self.browser_note = Some(swoop_i18n::trf!("来自浏览器：{file_name}{size}"));
        self.browser_names.insert(key, file_name.to_string());
    }

    /// 探测失败、浏览器自己下：撤下这条链接，草稿里没别的链接就关窗。返回是否撤下了
    pub fn withdraw_from_browser(&mut self, token: u64) -> bool {
        let Some((url, key)) = self.browser_pending.remove(&token) else { return false };
        self.browser_headers.remove(&key);
        self.browser_names.remove(&key);
        self.text = self.text.lines().filter(|line| line.trim() != url.trim()).collect::<Vec<_>>().join("\n");
        self.preview = None;
        if self.text.trim().is_empty() {
            self.close();
        } else if self.browser_pending.is_empty() {
            self.browser_note = None;
        }
        true
    }

    /// 关闭即丢弃草稿：取消（按钮、Esc、关窗）和提交成功都走这里
    fn close(&mut self) {
        self.open = false;
        self.pos = None;
        self.text.clear();
        self.preview = None;
        self.error = None;
        self.browser_headers.clear();
        self.browser_names.clear();
        self.browser_note = None;
        self.browser_pending.clear();
        self.shown_since = None;
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
        self.raise = true;
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

        let size = egui::vec2(580.0, 500.0);
        if self.add_dialog.pos.is_none() {
            self.add_dialog.pos = super::popup::centered(ctx, size, 0);
        }
        let builder = super::popup::builder(swoop_i18n::tr!("新建下载"), size, self.add_dialog.pos);
        if std::mem::take(&mut self.add_dialog.raise) {
            ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Focus);
        }
        let pending = !self.add_dialog.browser_pending.is_empty();
        let close = super::popup::show(ctx, viewport_id(), builder, &p, |ui| {
            if let Some(since) = self.add_dialog.shown_since.take() {
                tracing::info!(elapsed_ms = since.elapsed().as_millis() as u64, "浏览器下载：新建下载窗口已显示");
            }
            super::dialog::heading(
                ui,
                &p,
                egui_phosphor::regular::DOWNLOAD_SIMPLE,
                swoop_i18n::tr!("新建下载"),
                Some(swoop_i18n::tr!("每行一个链接，支持 {001..010}、{a..z} 批量展开")),
            );
            if let Some(note) = &self.add_dialog.browser_note {
                ui.label(RichText::new(note).size(12.5).color(p.accent));
                ui.add_space(6.0);
            }

            super::modal_body(ui, "add_dialog_body", 50.0, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(swoop_i18n::tr!("链接")).size(12.5).strong().color(p.text_muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(self.add_dialog.import.is_none(), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!(
                                            "{}  {}",
                                            egui_phosphor::regular::FILE_ARROW_UP,
                                            swoop_i18n::tr!("导入链接文件…")
                                        ))
                                        .size(12.5)
                                        .color(p.accent),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                self.start_import(ctx, None);
                            }
                        });
                    });
                });
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut self.add_dialog.text)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://example.com/file.zip")
                        .font(egui::TextStyle::Monospace)
                        .margin(Margin::same(12)),
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
                                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
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
                super::dialog::group(ui, &p, None, |ui| {
                    super::dialog::field_row(ui, &p, swoop_i18n::tr!("保存到"), 64.0, |ui| {
                        ui.add(super::field(&mut self.add_dialog.dir).desired_width(ui.available_width()));
                    });
                    super::dialog::field_row(ui, &p, swoop_i18n::tr!("下载队列"), 64.0, |ui| {
                        let queues = self.engine.queues();
                        if !queues.iter().any(|q| q.id == self.add_dialog.queue) {
                            self.add_dialog.queue = swoop_core::MAIN_QUEUE_ID;
                        }
                        let selected = queues.iter().find(|q| q.id == self.add_dialog.queue);
                        egui::ComboBox::from_id_salt("add_queue")
                            .width(200.0)
                            .selected_text(selected.map(super::queue_name).unwrap_or(swoop_i18n::tr!("主队列")))
                            .show_ui(ui, |ui| {
                                for q in &queues {
                                    ui.selectable_value(&mut self.add_dialog.queue, q.id, super::queue_name(q));
                                }
                            });
                        if selected.is_some_and(|q| !q.running) {
                            ui.label(
                                RichText::new(swoop_i18n::tr!("加入后等待队列启动")).size(12.0).color(p.text_muted),
                            );
                        }
                    });
                });

                if crate::completion::AUTO_INSTALL_SUPPORTED {
                    ui.add_space(6.0);
                    let mut install = self.completion.settings.auto_install;
                    if super::checkbox(ui, &mut install, swoop_i18n::tr!("运行安装包，结束后移到回收站"))
                        .on_hover_text(swoop_i18n::tr!("下载完成后由 Swoop 运行安装包；只对安装包生效"))
                        .changed()
                        && let Err(error) = self.completion.settings.set_auto_install(&self.store, install)
                    {
                        self.add_dialog.error = Some(error);
                    }
                }

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
                    ui.vertical(|ui| {
                        self.add_dialog.ftp.ui(ui, implicit);
                        if super::ghost_button(ui, &p, swoop_i18n::tr!("记住连接设置"))
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
                    super::dialog::error(ui, &p, err);
                }
            });
            // 按钮行贴着窗口底边
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.horizontal(|ui| {
                    if pending {
                        ui.add(egui::Spinner::new().size(12.0).color(p.text_muted));
                        ui.label(RichText::new(swoop_i18n::tr!("正在获取文件信息…")).size(12.0).color(p.text_muted));
                    } else {
                        ui.label(RichText::new(swoop_i18n::tr!("⌘↩ 开始  ·  Esc 取消")).size(12.0).color(p.text_muted));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let start =
                            ui.add_enabled_ui(!pending, |ui| primary_button(ui, &p, swoop_i18n::tr!("开始下载")));
                        if start.inner.clicked() {
                            submit = true;
                        }
                        if ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
            if !pending && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                submit = true;
            }
        });
        if close {
            cancel = true;
        }

        if cancel {
            self.add_dialog.close();
        } else if submit {
            self.submit_add(ctx);
        }
    }

    fn submit_add(&mut self, ctx: &egui::Context) {
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
        let mut started = Vec::new();
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
            match result {
                Ok(id) => started.push(id),
                Err(e) => {
                    failed.push(format!("{url}\n  {e}"));
                    retry.push(url);
                }
            }
        }
        if crate::completion::AUTO_INSTALL_SUPPORTED && self.completion.settings.auto_install && !started.is_empty() {
            for id in &started {
                self.completion.mark_install(&self.store, *id);
            }
            self.toasts.push_info(
                swoop_i18n::tr!("下载完成后会运行安装包"),
                swoop_i18n::tr!("安装程序结束后，安装包会移到回收站"),
            );
        }
        // 只为单个、立即开始的下载弹进度提示；批量或等待队列的不弹。卡片从对话框的位置飞到右下角
        let queue_running = self.engine.queues().iter().any(|q| q.id == self.add_dialog.queue && q.running);
        if let [id] = started[..]
            && queue_running
        {
            let from = ctx.input(|i| i.raw.viewports.get(&viewport_id()).and_then(|v| v.outer_rect));
            self.show_progress(ctx, id, from);
        }
        if self.default_dir != dir {
            self.default_dir = dir;
            self.save_settings();
        }

        if failed.is_empty() {
            self.add_dialog.close();
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
    fn cancel_discards_draft_text_and_login() {
        let mut dialog = private_draft();
        dialog.open = true;
        dialog.close();
        assert!(!dialog.open);
        assert!(dialog.text.is_empty());
        assert!(dialog.preview.is_none());
        assert!(dialog.error.is_none());
        assert_no_credentials_for_other_server(&dialog);
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
