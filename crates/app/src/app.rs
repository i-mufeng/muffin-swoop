use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, Modifiers, ViewportCommand};
use swoop_core::{Engine, Event, QueueId, QueueSnapshot, Store, TaskId, TaskSnapshot};
use tokio::runtime::Handle;
use tokio::sync::broadcast;

use crate::bridge::{Bridge, BridgeEvent, BridgeMode};
use crate::clipboard::ClipboardWatcher;
use crate::completion::FileAction;
use crate::notifications::{NotificationAction, NotificationSettings, Notifier};
use crate::settings::AppSettings;
use crate::theme::{Mode, Palette};
use crate::tray::{Tray, TrayCommand, TrayStatus};
use crate::ui::Filter;
use crate::ui::add_dialog::AddDialog;
use crate::ui::progress::ProgressStyle;
use crate::ui::toast::Toasts;

pub struct SwoopApp {
    pub engine: Arc<Engine>,
    pub store: Arc<Store>,
    pub palette: Palette,
    pub language: swoop_i18n::Language,
    pub native_drag: crate::native_drag::NativeDrag,
    /// 本帧的任务快照（每帧从引擎刷新）
    pub tasks: Vec<TaskSnapshot>,
    pub filter: Filter,
    pub search: String,
    pub selected: Option<TaskId>,
    /// 详情面板最后展示的任务；收起动画期间 selected 已为 None，靠它继续画
    pub detail_last: Option<TaskId>,
    pub add_dialog: AddDialog,
    pub show_settings: bool,
    pub default_dir: PathBuf,
    pub limit_enabled: bool,
    pub limit_mbps: f64,
    pub progress_style: ProgressStyle,
    pub clipboard_watch: bool,
    pub auto_resume: bool,
    pub bridge_enabled: bool,
    /// 设置里保存的端口；实际在跑的端口看 `bridge.port()`
    pub bridge_port: u16,
    pub bridge_mode: BridgeMode,
    pub bridge: Bridge,
    /// 最近一次启动桥接失败的原因（端口被占用等），设置页展示
    pub bridge_error: Option<String>,
    pub toasts: Toasts,
    /// 菜单栏图标；创建失败或平台不支持时为 None，此时关窗即退出
    pub tray: Option<Tray>,
    pub close_to_tray: bool,
    pub tray_speed: bool,
    pub notifications: NotificationSettings,
    pub notifier: Option<Notifier>,
    pub batch_limit: usize,
    pub queues: Vec<QueueSnapshot>,
    pub queue_filter: Option<QueueId>,
    pub queue_editor: crate::ui::queues::QueueEditor,
    pub task_transfer: crate::task_transfer::TaskTransfer,
    pub categories: crate::categories::Categories,
    pub category_filter: Option<String>,
    pub category_editor: Option<crate::categories::Categories>,
    pub category_error: Option<String>,
    pub network_editor: Option<crate::ui::network::NetworkEditor>,
    pub network_error: Option<String>,
    pub network_bypass: String,
    pub quota_editor: Option<swoop_core::QuotaPolicy>,
    pub quota_error: Option<String>,
    pub media_tasks: crate::media_tasks::MediaTasks,
    pub media_window: crate::ui::media::MediaWindow,
    pub task_editor: Option<crate::ui::task_properties::TaskEditor>,
    pub grabber: crate::grabber_tasks::GrabberTasks,
    pub completion: crate::completion::CompletionActions,
    pub completion_editor: Option<crate::ui::completion::CompletionEditor>,
    pub startup: crate::startup::StartupController,
    pub updates: crate::updates::Updates,
    update_quiescence: Option<swoop_core::Quiescence>,
    pub site_auth_editor: Option<crate::ui::site_auth::SiteAuthEditor>,
    pub file_conflict: Option<crate::ui::file_conflict::ConflictPrompt>,
    pub progress_window: bool,
    pub task_windows: crate::ui::task_window::TaskWindows,
    pub popup_host: crate::ui::popup::PopupHost,
    /// 窗口已隐藏到菜单栏
    pub(crate) hidden: bool,
    /// 用户从菜单选了退出：放行这一次关窗请求
    quitting: bool,
    clipboard: ClipboardWatcher,
    events: broadcast::Receiver<Event>,
}

impl SwoopApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        handle: Handle,
        engine: Arc<Engine>,
        store: Arc<Store>,
        settings: AppSettings,
        events: broadcast::Receiver<Event>,
    ) -> Self {
        let language = store
            .get_setting("language")
            .ok()
            .flatten()
            .and_then(|value| swoop_i18n::Language::parse(&value))
            .unwrap_or_default();
        swoop_i18n::set_language(language);
        // 环境变量只影响本次运行，不写回设置
        let mode = std::env::var("SWOOP_THEME").ok().and_then(|v| Mode::parse(&v)).unwrap_or(settings.theme);
        let progress_style = std::env::var("SWOOP_PROGRESS")
            .ok()
            .and_then(|v| ProgressStyle::parse(&v))
            .unwrap_or(settings.progress_style);
        let palette = match mode {
            Mode::Dark => Palette::dark(),
            Mode::Light => Palette::light(),
        };
        palette.apply(&cc.egui_ctx);
        let mut toasts = Toasts::default();
        if crate::demo::enabled() {
            toasts.push_info(swoop_i18n::tr!("下载完成"), "rustup-init");
            toasts.push_links(vec!["https://releases.ubuntu.com/24.04/ubuntu-24.04.3-desktop-amd64.iso".into()]);
        }
        let ctx = cc.egui_ctx.clone();
        let bridge = Bridge::new(
            handle.clone(),
            engine.clone(),
            settings.bridge_mode,
            settings.default_dir.clone(),
            move || ctx.request_repaint(),
        );
        let task_transfer = crate::task_transfer::TaskTransfer::load(&store);
        let categories = crate::categories::Categories::load(&store);
        bridge.set_categories(categories.clone());
        let media_tasks = crate::media_tasks::MediaTasks::load(handle.clone(), store.clone(), engine.target_registry());
        let updates = crate::updates::Updates::load(store.clone(), handle.clone());
        let native_drag = crate::native_drag::NativeDrag::load(&store);
        let grabber = crate::grabber_tasks::GrabberTasks::load(store.clone(), handle);
        let completion = crate::completion::CompletionActions::new(&store);
        let mut add_dialog = AddDialog::new(&settings.default_dir);
        add_dialog.ftp = crate::ftp_settings::FtpEditor::load(&store);
        let mut app = Self {
            engine,
            store,
            palette,
            language,
            native_drag,
            tasks: Vec::new(),
            filter: Filter::All,
            search: String::new(),
            // 演示模式默认选中第一条，方便截图看到详情面板
            selected: crate::demo::enabled().then_some(crate::demo::SELECTED),
            detail_last: None,
            add_dialog,
            show_settings: false,
            default_dir: settings.default_dir,
            limit_enabled: settings.speed_limit_on,
            limit_mbps: settings.speed_limit_mbps,
            progress_style,
            clipboard_watch: settings.clipboard_watch,
            auto_resume: settings.auto_resume,
            bridge_enabled: settings.bridge_enabled,
            bridge_port: settings.bridge_port,
            bridge_mode: settings.bridge_mode,
            bridge,
            bridge_error: None,
            toasts,
            tray: Tray::new(&cc.egui_ctx),
            close_to_tray: settings.close_to_tray,
            tray_speed: settings.tray_speed,
            notifications: settings.notifications,
            notifier: {
                let ctx = cc.egui_ctx.clone();
                Notifier::new(move || ctx.request_repaint())
            },
            batch_limit: settings.batch_limit,
            queues: Vec::new(),
            queue_filter: None,
            queue_editor: crate::ui::queues::QueueEditor::default(),
            task_transfer,
            categories,
            category_filter: None,
            category_editor: None,
            category_error: None,
            network_editor: None,
            network_error: None,
            network_bypass: String::new(),
            quota_editor: None,
            quota_error: None,
            media_tasks,
            media_window: crate::ui::media::MediaWindow::default(),
            task_editor: None,
            grabber,
            completion,
            completion_editor: None,
            startup: crate::startup::StartupController::new(&cc.egui_ctx),
            updates,
            update_quiescence: None,
            site_auth_editor: None,
            file_conflict: None,
            progress_window: settings.progress_window,
            task_windows: Default::default(),
            popup_host: Default::default(),
            hidden: false,
            quitting: false,
            clipboard: ClipboardWatcher::new(),
            events,
        };
        app.apply_bridge();
        app
    }

    /// 按当前设置启动/重启/停止浏览器桥接。端口被占用只报错，不影响其它功能。
    pub fn apply_bridge(&mut self) {
        if !self.bridge_enabled {
            self.bridge.stop();
            self.bridge_error = None;
            return;
        }
        if self.bridge.port() == Some(self.bridge_port) {
            return;
        }
        match self.bridge.start(self.bridge_port) {
            Ok(_) => self.bridge_error = None,
            Err(e) => {
                tracing::warn!("浏览器桥接启动失败: {e}");
                self.toasts
                    .push_error(swoop_i18n::tr!("浏览器集成未启动"), swoop_i18n::trf!("{e}，可在设置里换一个端口"));
                self.bridge_error = Some(e);
            }
        }
    }

    /// 把当前 UI 状态写回设置表
    pub fn save_settings(&mut self) {
        let result = AppSettings {
            theme: self.palette.mode,
            default_dir: self.default_dir.clone(),
            progress_style: self.progress_style,
            clipboard_watch: self.clipboard_watch,
            auto_resume: self.auto_resume,
            speed_limit_on: self.limit_enabled,
            speed_limit_mbps: self.limit_mbps,
            bridge_enabled: self.bridge_enabled,
            bridge_port: self.bridge_port,
            bridge_mode: self.bridge_mode,
            close_to_tray: self.close_to_tray,
            tray_speed: self.tray_speed,
            notifications: self.notifications,
            connections: self.engine.connections(),
            batch_limit: self.batch_limit,
            progress_window: self.progress_window,
        }
        .save(&self.store);
        if let Err(error) = result {
            self.toasts.push_error(swoop_i18n::tr!("设置未保存"), error.to_string());
        }
        self.bridge.set_mode(self.bridge_mode);
        self.bridge.set_dir(self.default_dir.clone());
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, mode: Mode) {
        self.palette = match mode {
            Mode::Dark => Palette::dark(),
            Mode::Light => Palette::light(),
        };
        self.palette.apply(ctx);
        self.save_settings();
    }

    pub fn set_language(&mut self, ctx: &egui::Context, language: swoop_i18n::Language) {
        match self.store.set_setting("language", language.as_str()) {
            Ok(()) => {
                self.language = language;
                swoop_i18n::set_language(language);
                self.tray = Tray::new(ctx);
                ctx.request_repaint();
            }
            Err(error) => self.toasts.push_error(swoop_i18n::tr!("语言设置未保存"), error.to_string()),
        }
    }

    pub fn toggle_theme(&mut self, ctx: &egui::Context) {
        let next = match self.palette.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        };
        self.set_theme(ctx, next);
    }

    /// 复制到系统剪贴板并提示；自己复制的链接不触发剪贴板监听
    pub fn copy_link(&mut self, _ctx: &egui::Context, url: String) {
        match self.clipboard.write(&url) {
            Ok(()) => {
                self.toasts.push_info(swoop_i18n::tr!("已复制链接"), swoop_i18n::trf!("{} 个链接", url.lines().count()))
            }
            Err(error) => self.toasts.push_error(swoop_i18n::tr!("复制失败"), error),
        }
    }

    /// 当前过滤器 + 搜索词下可见的任务
    pub fn visible_tasks(&self) -> Vec<TaskSnapshot> {
        let q = self.search.trim().to_lowercase();
        let members = self.queue_filter.map(|id| {
            self.queues
                .iter()
                .find(|queue| queue.id == id)
                .map(|queue| queue.task_ids.iter().copied().collect::<std::collections::HashSet<_>>())
                .unwrap_or_default()
        });
        self.tasks
            .iter()
            .filter(|t| members.as_ref().is_none_or(|members| members.contains(&t.id)))
            .filter(|t| self.filter.matches(&t.status))
            .filter(|t| {
                self.category_filter.as_ref().is_none_or(|name| {
                    self.categories.classify(&t.url, Some(&t.file_name)).is_some_and(|rule| &rule.name == name)
                })
            })
            .filter(|t| q.is_empty() || t.file_name.to_lowercase().contains(&q) || t.url.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
            self.add_dialog.queue = self.queue_filter.unwrap_or(swoop_core::MAIN_QUEUE_ID);
            self.add_dialog.open_with(None);
        }
        // Esc 收起详情面板；弹窗打开或输入框聚焦时 Esc 归它们
        if self.selected.is_some()
            && !self.add_dialog.open
            && !self.show_settings
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.selected = None;
        }
        // 没有输入框聚焦时，⌘V 粘贴链接直接打开新建对话框
        if !ctx.egui_wants_keyboard_input() && !self.add_dialog.open {
            let pasted: Vec<String> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Paste(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            });
            for text in pasted {
                if looks_like_url(&text) {
                    self.add_dialog.open_with(Some(text.trim().to_string()));
                }
            }
        }
    }

    /// 引擎事件：下载完成/失败时弹吐司
    fn poll_events(&mut self, ctx: &egui::Context) {
        let background = self.hidden || ctx.input(|i| !i.focused || i.viewport().minimized == Some(true));
        loop {
            match self.events.try_recv() {
                Ok(event @ (Event::Completed(_) | Event::Failed(_, _))) => {
                    let id = match &event {
                        Event::Completed(id) | Event::Failed(id, _) => *id,
                        _ => unreachable!(),
                    };
                    let task = self.engine.snapshot_one(id);
                    let name = task.as_ref().map(|t| t.file_name.as_str());
                    if let Some(message) = self.notifications.message(&event, name, background)
                        && let Some(notifier) = &self.notifier
                    {
                        notifier.send(match (&event, &task) {
                            (Event::Completed(_), Some(task)) => message.with_file(task.path.clone()),
                            _ => message,
                        });
                    }
                    match event {
                        Event::Completed(_) => {
                            if let Some(task) = &task {
                                self.toasts.push_completed(
                                    swoop_i18n::tr!("下载完成"),
                                    task.file_name.clone(),
                                    task.path.clone(),
                                );
                            }
                            if !self.quitting
                                && let Some(task) = task
                                && let Err(error) =
                                    self.completion.submit(task.path, !self.task_windows.contains(task.id))
                            {
                                self.toasts.push_error(swoop_i18n::tr!("完成动作未执行"), error);
                            }
                        }
                        Event::Failed(_, reason) => {
                            self.toasts.push_error(swoop_i18n::trf!("下载失败：{}", name.unwrap_or_default()), reason);
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(Event::FileConflict(_)) if !self.quitting => self.show_window(ctx),
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    /// 系统通知上的按钮：打开文件、打开所在文件夹，点正文只调出窗口
    fn poll_notifications(&mut self, ctx: &egui::Context) {
        let Some(notifier) = &self.notifier else {
            return;
        };
        for action in notifier.poll() {
            let (path, action) = match action {
                NotificationAction::Open(path) => (path, FileAction::Open),
                NotificationAction::Reveal(path) => (path, FileAction::Reveal),
                NotificationAction::Show => {
                    self.show_window(ctx);
                    continue;
                }
            };
            if let Err(error) = self.completion.open_path(path, action) {
                self.toasts.push_error(swoop_i18n::tr!("无法打开"), error);
            }
        }
    }

    /// 浏览器扩展交来的下载：静默模式已入队，只提示；询问模式弹预填好的新建对话框
    fn poll_bridge(&mut self, ctx: &egui::Context) {
        while let Some(ev) = self.bridge.try_recv() {
            match ev {
                BridgeEvent::Show => self.show_window(ctx),
                BridgeEvent::Control(request) => request.respond(|command| {
                    if self.quitting || self.updates.installing() {
                        Err(swoop_i18n::tr!("界面正在退出").into())
                    } else {
                        execute_desktop_control(command, &mut self.media_tasks, &mut self.grabber, ctx, &self.engine)
                    }
                }),
                BridgeEvent::Added { id, file_name } => {
                    self.toasts.push_info(swoop_i18n::tr!("已接收浏览器下载"), file_name);
                    self.open_task_window(ctx, id);
                }
                BridgeEvent::Ask { url, file_name, size, headers } => {
                    // 新建对话框是独立窗口，主窗口隐藏时也不必调出来
                    self.add_dialog.open_from_browser(url, &file_name, size, headers);
                }
                BridgeEvent::Media { source } => {
                    self.show_window(ctx);
                    self.media_window.receive(source, &self.default_dir);
                }
            }
        }
    }

    /// 菜单栏菜单的命令；窗口隐藏时也在 logic 里执行
    fn poll_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        let mut cmds = Vec::new();
        while let Some(cmd) = tray.try_recv() {
            cmds.push(cmd);
        }
        for cmd in cmds {
            if self.updates.installing() && !matches!(cmd, TrayCommand::Quit | TrayCommand::Show) {
                continue;
            }
            match cmd {
                TrayCommand::Show => self.show_window(ctx),
                TrayCommand::NewTask => {
                    self.show_window(ctx);
                    self.add_dialog.open_with(None);
                }
                TrayCommand::ResumeAll => self.engine.resume_all(),
                TrayCommand::PauseAll => self.engine.pause_all(),
                TrayCommand::Quit => self.request_quit(ctx),
            }
        }
    }

    /// 点关闭按钮：开了驻留且有菜单栏图标时只隐藏窗口，下载继续
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting || !self.close_to_tray || self.tray.is_none() {
            self.quitting = true;
            self.completion.cancel_all();
            self.updates.cancel();
            if self.media_tasks.active()
                || self.grabber.busy()
                || self.completion.busy()
                || self.startup.busy()
                || self.updates.busy()
                || self.site_auth_editor.as_ref().is_some_and(|editor| editor.busy())
                || self.network_editor.as_ref().is_some_and(|editor| editor.busy())
                || self.task_transfer.busy()
            {
                self.quitting = true;
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.media_tasks.cancel_all();
                self.grabber.stop();
                self.completion.cancel_all();
                ctx.request_repaint();
            }
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        crate::tray::set_dock_visible(false);
        self.hidden = true;
    }

    pub(crate) fn show_window(&mut self, ctx: &egui::Context) {
        if self.hidden {
            crate::tray::set_dock_visible(true);
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            self.hidden = false;
        }
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn update_tray(&mut self) {
        let Some(tray) = &mut self.tray else { return };
        let mut s = TrayStatus { speed: self.engine.global_speed(), ..Default::default() };
        for t in &self.tasks {
            match t.status {
                swoop_core::TaskStatus::Paused | swoop_core::TaskStatus::Failed(_) => s.resumable += 1,
                ref st if st.is_active() => s.active += 1,
                _ => {}
            }
        }
        tray.update(s, self.tray_speed);
    }

    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.updates.cancel();
        self.completion.cancel_all();
        self.media_tasks.cancel_all();
        self.grabber.stop();
        self.bridge.stop();
        if !self.media_tasks.active()
            && !self.grabber.busy()
            && !self.completion.busy()
            && !self.startup.busy()
            && !self.updates.busy()
            && !self.site_auth_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.network_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.task_transfer.busy()
        {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    pub(crate) fn is_quitting(&self) -> bool {
        self.quitting
    }

    pub(crate) fn update_install_idle(&self) -> bool {
        !self
            .engine
            .snapshot()
            .iter()
            .any(|task| task.status.is_active() || task.status == swoop_core::TaskStatus::FileConflict)
            && !self.media_tasks.active()
            && !self.grabber.busy()
            && !self.completion.busy()
            && !self.startup.busy()
            && self.file_conflict.is_none()
            && !self.site_auth_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.network_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.task_transfer.busy()
    }

    pub(crate) fn begin_update_install(&mut self, ctx: &egui::Context) {
        if self.updates.busy() || !self.update_install_idle() {
            return;
        }
        let Some(guard) = self.engine.try_quiesce() else {
            self.updates.error = Some(swoop_i18n::tr!("任务状态已改变，请结束下载后重新点击安装").into());
            return;
        };
        self.update_quiescence = Some(guard);
        self.bridge.stop();
        self.updates.prepare_install(ctx);
    }

    /// 剪贴板里出现新链接 → 吐司询问；新建对话框打开时不打扰
    fn poll_clipboard(&mut self) {
        // 隐藏时不读：等窗口回来再提示最新复制的链接
        if !self.clipboard_watch || self.hidden {
            return;
        }
        let urls = self.clipboard.poll();
        if urls.is_empty() || self.add_dialog.open {
            return;
        }
        self.toasts.push_links(urls);
    }
}

/// Runs only on the application's UI thread; no second engine or project worker set.
fn execute_desktop_control(
    command: crate::instance::Command,
    media: &mut crate::media_tasks::MediaTasks,
    grabber: &mut crate::grabber_tasks::GrabberTasks,
    ctx: &egui::Context,
    engine: &Engine,
) -> Result<serde_json::Value, String> {
    use crate::grabber_tasks::ProjectAction;
    use crate::instance::Command;
    use serde_json::json;
    command.validate()?;
    media.update_file_policy(engine);
    match command {
        Command::MediaStatus { id } => media.control_status(id),
        Command::MediaAdd { url, output, duration, max_height, audio_language, speed_limit_bps } => {
            let mut selection = media.preferences.selection();
            if let Some(height) = max_height {
                selection.video = swoop_media::VideoSelection::MaxHeight(height);
            }
            if let Some(language) = audio_language {
                selection.audio = swoop_media::AudioSelection::PreferredLanguage(language);
            }
            let id = media.add_control(
                swoop_media::MediaSource { url, headers: vec![] },
                output,
                duration,
                selection,
                speed_limit_bps,
                ctx,
            )?;
            Ok(json!({"accepted":true,"id":id}))
        }
        Command::MediaLimit { id, speed_limit_bps } => {
            media.set_speed_limit(id, speed_limit_bps)?;
            Ok(json!({"accepted":true,"id":id,"speed_limit_bps":speed_limit_bps}))
        }
        Command::MediaStop { id } => {
            let job = media.jobs.iter().find(|j| j.id == id).ok_or(swoop_i18n::tr!("媒体记录不存在"))?;
            if !job.status.active() {
                return Err(swoop_i18n::tr!("媒体任务未在运行").into());
            }
            media.cancel(id);
            Ok(json!({"accepted":true,"id":id,"status":"cancelling"}))
        }
        Command::MediaResolve { id, action } => {
            media.resolve_file_conflict(id, action.core(), ctx)?;
            Ok(json!({"accepted":true,"id":id}))
        }
        Command::MediaRetry { id } => {
            media.restart(id, ctx)?;
            Ok(json!({"accepted":true,"id":id}))
        }
        Command::GrabberStatus => Ok(grabber.control_status()),
        Command::GrabberStop => {
            if !grabber.busy() {
                return Err(swoop_i18n::tr!("没有运行中的项目操作").into());
            }
            grabber.stop();
            if let Some(operation) = &mut grabber.operation {
                operation.state = "cancelling";
            }
            Ok(json!({"accepted":true,"operation_id":grabber.operation.as_ref().map(|o| o.id),"status":"cancelling"}))
        }
        Command::GrabberCreate { url, directory, max_depth, max_pages, max_bytes, concurrency } => {
            if grabber.busy() {
                return Err(swoop_i18n::tr!("已有项目操作正在运行").into());
            }
            grabber.config.start_url = url;
            grabber.config.output_dir = directory;
            if let Some(value) = max_depth {
                grabber.config.max_depth = value;
            }
            if let Some(value) = max_pages {
                grabber.config.max_pages = value;
            }
            if let Some(value) = max_bytes {
                grabber.config.max_total_bytes = value;
            }
            if let Some(value) = concurrency {
                grabber.config.concurrency = value;
            }
            let id = grabber.try_start(ProjectAction::CreateOnly, ctx)?;
            Ok(json!({"accepted":true,"operation_id":id}))
        }
        Command::GrabberOpen { directory } => {
            if grabber.busy() {
                return Err(swoop_i18n::tr!("已有项目操作正在运行").into());
            }
            grabber.config.output_dir = directory;
            let id = grabber.try_start(ProjectAction::Open, ctx)?;
            Ok(json!({"accepted":true,"operation_id":id}))
        }
        Command::GrabberRun => {
            if grabber.busy() {
                return Err(swoop_i18n::tr!("已有项目操作正在运行").into());
            }
            let config = grabber.snapshot.as_ref().ok_or(swoop_i18n::tr!("请先创建或打开项目"))?.config.clone();
            grabber.config = config;
            grabber.domains_text = grabber.config.allowed_domains.join(", ");
            grabber.extensions_text = grabber.config.resource_extensions.join(", ");
            let id = grabber.try_start(ProjectAction::Run, ctx)?;
            Ok(json!({"accepted":true,"operation_id":id}))
        }
        _ => Err(swoop_i18n::tr!("不是桌面控制命令").into()),
    }
}

pub fn looks_like_url(s: &str) -> bool {
    s.lines().any(|l| {
        let l = l.trim();
        let lower = l.to_ascii_lowercase();
        ["http://", "https://", "ftp://", "ftps://"].iter().any(|scheme| lower.starts_with(scheme))
    })
}

impl eframe::App for SwoopApp {
    /// 每帧 ui 之前调用；窗口隐藏到菜单栏时 eframe 不跑 ui，只要有 repaint 请求就还会调这里
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Process close intent before a ready update helper can authorize shutdown.
        self.handle_close(ctx);
        self.startup.poll();
        self.media_tasks.update_file_policy(&self.engine);
        self.tasks = self.engine.snapshot();
        self.queues = self.engine.queues();
        if self.queue_filter.is_some_and(|id| !self.queues.iter().any(|q| q.id == id)) {
            self.queue_filter = None;
        }
        if crate::demo::enabled() {
            self.tasks.extend(crate::demo::tasks(&self.default_dir));
        }
        self.poll_tray(ctx);
        self.poll_task_transfers(ctx);
        self.poll_import(ctx);
        if !self.quitting && !self.updates.installing() {
            for payload in self.native_drag.update(frame, ctx) {
                self.show_window(ctx);
                match payload {
                    crate::native_drag::DropPayload::Urls(text) => self.add_dialog.open_with(Some(text)),
                    crate::native_drag::DropPayload::Files(paths) => self.start_dropped_files(ctx, paths),
                }
            }
            if let Some(error) = self.native_drag.take_error() {
                self.toasts.push_error(swoop_i18n::tr!("拖放失败"), error);
            }
        }
        self.grabber.poll();
        self.site_auth_logic(ctx);
        self.network_logic();
        for (success, name, error, path) in self.media_tasks.poll() {
            let event = if success { Event::Completed(TaskId(0)) } else { Event::Failed(TaskId(0), error.clone()) };
            let background = self.hidden || ctx.input(|i| !i.focused || i.viewport().minimized == Some(true));
            if let Some(message) = self.notifications.message(&event, Some(&name), background)
                && let Some(notifier) = &self.notifier
            {
                notifier.send(match &path {
                    Some(path) if success => message.with_file(path.clone()),
                    _ => message,
                });
            }
            if success {
                match &path {
                    Some(path) => self.toasts.push_completed(swoop_i18n::tr!("媒体下载完成"), name, path.clone()),
                    None => self.toasts.push_info(swoop_i18n::tr!("媒体下载完成"), name),
                }
                if !self.quitting
                    && let Some(path) = path
                    && let Err(error) = self.completion.submit(path, true)
                {
                    self.toasts.push_error(swoop_i18n::tr!("完成动作未执行"), error);
                }
            } else {
                self.toasts.push_error(name, error);
            }
        }
        self.poll_events(ctx);
        self.poll_notifications(ctx);
        self.poll_clipboard();
        self.poll_bridge(ctx);
        self.poll_file_conflicts(ctx);
        self.update_tray();
        self.poll_completion(ctx);
        self.popup_logic(ctx);
        if self.updates.poll(ctx, self.update_install_idle(), self.quitting) {
            if let Some(guard) = self.update_quiescence.take() {
                guard.commit();
            }
            self.request_quit(ctx);
        }
        if !self.updates.installing() && self.update_quiescence.is_some() {
            if self.quitting {
                self.update_quiescence.take().unwrap().commit();
            } else {
                self.update_quiescence.take();
                self.apply_bridge();
            }
        }
        // 等待 FFmpeg 退出、临时文件清理及站点快照保存完成，再真正退出。
        if self.quitting
            && !self.media_tasks.active()
            && !self.grabber.busy()
            && !self.completion.busy()
            && !self.startup.busy()
            && !self.updates.busy()
            && !self.site_auth_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.network_editor.as_ref().is_some_and(|editor| editor.busy())
            && !self.task_transfer.busy()
        {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // 有任务在跑就 10fps 刷新进度，否则低频；隐藏时只需维持菜单栏速度。剪贴板轮询也靠这个节拍
        let any_active = self.tasks.iter().any(|t| t.status.is_active()) || self.media_tasks.active();
        let every = match (self.hidden, any_active) {
            (true, _) => Duration::from_secs(1),
            (false, true) => Duration::from_millis(100),
            (false, false) => Duration::from_millis(500),
        };
        ctx.request_repaint_after(every);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.quitting {
            ui.label(swoop_i18n::tr!("正在退出…"));
            return;
        }
        if self.updates.installing() {
            // All other entry points remain disabled until install preparation is cancelled or fails.
            self.updates.open = true;
            self.updates_ui(&ctx);
            return;
        }
        if self.hidden {
            // 主窗口隐藏时只画独立弹窗（有弹窗可见时 eframe 仍会跑根视口的 ui）
            self.popups_ui(&ctx);
            return;
        }
        self.handle_shortcuts(&ctx);

        // 面板顺序决定布局：侧栏占满左侧，状态栏横跨右侧，详情面板在状态栏之上贴右，顶栏只占列表宽度
        self.sidebar(ui);
        self.statusbar(ui);
        self.detail_panel(ui);
        self.topbar(ui);
        self.task_list(ui);
        self.settings_ui(&ctx);
        self.updates_ui(&ctx);
        if self.updates.installing() {
            return;
        }
        self.site_auth_ui(&ctx);
        self.queues_ui(&ctx);
        self.categories_ui(&ctx);
        self.network_ui(&ctx);
        self.quota_ui(&ctx);
        self.media_ui(&ctx);
        self.task_properties_ui(&ctx);
        self.grabber_ui(&ctx);
        self.completion_ui(&ctx);
        self.file_conflict_ui(&ctx);
        self.toasts_ui(&ctx);
        self.popups_ui(&ctx);
    }
}

#[cfg(test)]
mod control_tests {
    use super::execute_desktop_control;
    use crate::instance::{self, Command, ControlRequest, ControlServer, InstanceGuard};
    use crate::{grabber_tasks::GrabberTasks, media_tasks::MediaTasks};
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper_util::rt::TokioIo;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};
    use swoop_core::{Config, Engine, Store};

    struct Worker {
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Worker {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(join) = self.join.take() {
                join.join().unwrap();
            }
        }
    }
    async fn request(data: &std::path::Path, command: Command) -> Value {
        instance::send(data, &command, false).await.unwrap()
    }
    async fn finished(data: &std::path::Path, operation: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let value = request(data, Command::GrabberStatus).await;
            assert_eq!(value["data"]["operation"]["id"], operation);
            if value["data"]["busy"] == false {
                return value["data"].clone();
            }
            assert!(Instant::now() < deadline, "{value}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn stopped(data: &std::path::Path, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let value = request(data, Command::MediaStatus { id: Some(id) }).await;
            let state = value["data"]["tasks"][0]["status"].as_str().unwrap();
            if state == "paused" {
                return value;
            }
            assert!(Instant::now() < deadline, "{value}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn real_control_http_dispatches_media_and_grabber_on_one_controller_thread() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("swoop.db")).unwrap());
        let guard = InstanceGuard::acquire(directory.path()).unwrap().unwrap();
        let engine = Arc::new(Engine::new(runtime.handle().clone(), Config::default()));
        let (tx, rx) = mpsc::channel::<ControlRequest>();
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        let handle = runtime.handle().clone();
        let saved = store.clone();
        let controller_engine = engine.clone();
        let worker = Worker {
            stop,
            join: Some(std::thread::spawn(move || {
                let mut media = MediaTasks::load(handle.clone(), saved.clone(), controller_engine.target_registry());
                let mut grabber = GrabberTasks::load(saved, handle);
                let context = egui::Context::default();
                while !running.load(Ordering::SeqCst) {
                    media.poll();
                    grabber.poll();
                    if let Ok(request) = rx.recv_timeout(Duration::from_millis(5)) {
                        request.respond(|command| {
                            execute_desktop_control(command, &mut media, &mut grabber, &context, &controller_engine)
                        });
                    }
                }
                media.cancel_all();
                grabber.stop();
                let deadline = Instant::now() + Duration::from_secs(10);
                while media.active() || grabber.busy() {
                    assert!(Instant::now() < deadline);
                    media.poll();
                    grabber.poll();
                    std::thread::sleep(Duration::from_millis(5));
                }
            })),
        };
        let output = directory.path().to_owned();
        let server = ControlServer::start(
            &guard,
            engine,
            runtime.handle(),
            Arc::new(|| true),
            Arc::new(move |_, _| output.clone()),
            Arc::new(move |request| tx.send(request).is_ok()),
        )
        .unwrap();
        let slow = Arc::new(AtomicBool::new(true));
        let requests = Arc::new(AtomicUsize::new(0));
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let delay = slow.clone();
            let count = requests.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let delay = delay.clone();
                    let count = count.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                            let delay = delay.clone();
                            let count = count.clone();
                            async move {
                                let path = req.uri().path();
                                if path != "/robots.txt" {
                                    count.fetch_add(1, Ordering::SeqCst);
                                }
                                if path == "/slow.m3u8" || (path == "/index.html" && delay.load(Ordering::SeqCst)) {
                                    tokio::time::sleep(Duration::from_secs(20)).await;
                                }
                                let body = match path {
                                    "/robots.txt" => "User-agent: *\nAllow: /\n",
                                    "/index.html" => "<html><a href='/child.html'>child</a></html>",
                                    _ => "<html>done</html>",
                                };
                                Ok::<_, std::convert::Infallible>(
                                    hyper::Response::builder()
                                        .header(
                                            "Content-Type",
                                            if path == "/robots.txt" { "text/plain" } else { "text/html" },
                                        )
                                        .body(Full::new(Bytes::from_static(body.as_bytes())))
                                        .unwrap(),
                                )
                            }
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
            let data = directory.path();
            let media = |name: &str| Command::MediaAdd {
                url: format!("{base}/slow.m3u8?token=secret-value"),
                output: data.join(name),
                duration: None,
                max_height: None,
                audio_language: None,
                speed_limit_bps: None,
            };
            let (first, second) = tokio::join!(request(data, media("first.mp4")), request(data, media("second.mp4")));
            assert_eq!(first["data"]["accepted"], true);
            assert_eq!(second["data"]["accepted"], true);
            let first = first["data"]["id"].as_u64().unwrap();
            let second = second["data"]["id"].as_u64().unwrap();
            assert_ne!(first, second);
            assert_eq!(request(data, Command::MediaStop { id: first }).await["ok"], true);
            assert_eq!(request(data, Command::MediaStop { id: second }).await["ok"], true);
            stopped(data, first).await;
            stopped(data, second).await;
            assert_eq!(request(data, Command::MediaRetry { id: first }).await["data"]["accepted"], true);
            assert_eq!(request(data, Command::MediaStop { id: first }).await["ok"], true);
            stopped(data, first).await;
            assert!(!data.join("first.mp4").exists());
            assert!(!data.join("second.mp4").exists());
            let state = request(data, Command::MediaStatus { id: None }).await;
            assert_eq!(state["data"]["tasks"].as_array().unwrap().len(), 2);
            assert_eq!(state["data"]["persistence_error"], false);
            assert!(!state.to_string().contains("secret-value"));
            assert!(!state.to_string().contains("headers"));
            assert_eq!(request(data, Command::MediaRetry { id: 999 }).await["ok"], false);
            assert_eq!(request(data, Command::MediaStatus { id: Some(0) }).await["ok"], false);
            use crate::instance::ConflictAction;
            std::fs::write(data.join("exists.mp4"), "preserve").unwrap();
            for action in [ConflictAction::Rename, ConflictAction::Overwrite, ConflictAction::Skip] {
                let added = request(data, media("exists.mp4")).await;
                let id = added["data"]["id"].as_u64().unwrap();
                assert_eq!(
                    request(data, Command::MediaStatus { id: Some(id) }).await["data"]["tasks"][0]["status"],
                    "waiting_for_confirmation"
                );
                assert_eq!(request(data, Command::MediaResolve { id, action: ConflictAction::Ask }).await["ok"], false);
                assert_eq!(request(data, Command::MediaResolve { id, action }).await["data"]["accepted"], true);
                if action != ConflictAction::Skip {
                    assert_eq!(request(data, Command::MediaStop { id }).await["ok"], true);
                }
                let state = stopped(data, id).await;
                let output = state["data"]["tasks"][0]["path"].as_str().unwrap();
                if action == ConflictAction::Rename {
                    assert_ne!(std::path::Path::new(output), data.join("exists.mp4"));
                    assert!(!std::path::Path::new(output).exists());
                }
                assert_eq!(std::fs::read(data.join("exists.mp4")).unwrap(), b"preserve");
                if action == ConflictAction::Overwrite {
                    assert_eq!(request(data, Command::MediaRetry { id }).await["ok"], true);
                    assert_eq!(
                        request(data, Command::MediaStatus { id: Some(id) }).await["data"]["tasks"][0]["status"],
                        "waiting_for_confirmation"
                    );
                    assert_eq!(
                        request(data, Command::MediaResolve { id, action: ConflictAction::Skip }).await["ok"],
                        true
                    );
                }
            }
            request(data, Command::ConflictPolicy { policy: Some(ConflictAction::Rename) }).await;
            let automatic = request(data, media("exists.mp4")).await["data"]["id"].as_u64().unwrap();
            let automatic_state = request(data, Command::MediaStatus { id: Some(automatic) }).await;
            assert_ne!(automatic_state["data"]["tasks"][0]["status"], "waiting_for_confirmation");
            assert_eq!(request(data, Command::MediaStop { id: automatic }).await["ok"], true);
            stopped(data, automatic).await;
            request(data, Command::ConflictPolicy { policy: Some(ConflictAction::Ask) }).await;
            assert_eq!(request(data, Command::GrabberRun).await["ok"], false);
            let project = data.join("站点 项目");
            let added = request(
                data,
                Command::GrabberCreate {
                    url: format!("{base}/index.html?token=secret-value"),
                    directory: project.clone(),
                    max_depth: Some(1),
                    max_pages: Some(10),
                    max_bytes: Some(1024 * 1024),
                    concurrency: Some(1),
                },
            )
            .await;
            let created = finished(data, added["data"]["operation_id"].as_u64().unwrap()).await;
            assert_eq!(created["operation"]["status"], "succeeded");
            assert_eq!(created["project"]["status"], "ready");
            let before = requests.load(Ordering::SeqCst);
            let started = request(data, Command::GrabberRun).await;
            assert_eq!(request(data, Command::GrabberRun).await["ok"], false, "busy operation must reject second run");
            let deadline = Instant::now() + Duration::from_secs(5);
            while requests.load(Ordering::SeqCst) == before {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(request(data, Command::GrabberStop).await["data"]["accepted"], true);
            let cancelled = finished(data, started["data"]["operation_id"].as_u64().unwrap()).await;
            assert_eq!(cancelled["operation"]["status"], "cancelled");
            slow.store(false, Ordering::SeqCst);
            let opened = request(data, Command::GrabberOpen { directory: project.clone() }).await;
            assert_eq!(
                finished(data, opened["data"]["operation_id"].as_u64().unwrap()).await["operation"]["status"],
                "succeeded"
            );
            let started = request(data, Command::GrabberRun).await;
            let completed = finished(data, started["data"]["operation_id"].as_u64().unwrap()).await;
            assert_eq!(completed["project"]["status"], "complete");
            assert_eq!(completed["project"]["downloaded"], 2);
            assert!(!completed.to_string().contains("secret-value"));
            assert!(!completed.to_string().contains("start_url"));
            assert!(std::path::Path::new(completed["project"]["entry_path"].as_str().unwrap()).is_file());
            let failure = request(data, Command::GrabberOpen { directory: data.join("missing") }).await;
            assert_eq!(failure["data"]["accepted"], true);
            let failed = finished(data, failure["data"]["operation_id"].as_u64().unwrap()).await;
            assert_eq!(failed["operation"]["status"], "failed");
            assert_eq!(failed["error"], true);
        });
        drop(server);
        drop(worker);
        let restored = MediaTasks::load(runtime.handle().clone(), store.clone(), swoop_core::TargetRegistry::default());
        assert_eq!(restored.jobs.len(), 6);
        assert!(restored.jobs.iter().all(|j| j.status == crate::media_tasks::MediaStatus::Paused));
        assert!(store.get_setting("grabber_recent").unwrap().unwrap().contains("站点 项目"));
    }
}
