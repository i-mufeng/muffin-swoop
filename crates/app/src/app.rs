use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, Modifiers};
use swoop_core::{Engine, Event, Store, TaskId, TaskSnapshot};
use tokio::sync::broadcast;

use crate::clipboard::ClipboardWatcher;
use crate::settings::AppSettings;
use crate::theme::{Mode, Palette};
use crate::ui::Filter;
use crate::ui::add_dialog::AddDialog;
use crate::ui::progress::ProgressStyle;
use crate::ui::toast::Toasts;

pub struct SwoopApp {
    pub engine: Arc<Engine>,
    pub store: Arc<Store>,
    pub palette: Palette,
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
    pub toasts: Toasts,
    clipboard: ClipboardWatcher,
    events: broadcast::Receiver<Event>,
}

impl SwoopApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Arc<Engine>, store: Arc<Store>, settings: AppSettings) -> Self {
        // 环境变量只影响本次运行，不写回设置
        let mode = std::env::var("SWOOP_THEME").ok().and_then(|v| Mode::parse(&v)).unwrap_or(settings.theme);
        let progress_style =
            std::env::var("SWOOP_PROGRESS").ok().and_then(|v| ProgressStyle::parse(&v)).unwrap_or(settings.progress_style);
        let palette = match mode {
            Mode::Dark => Palette::dark(),
            Mode::Light => Palette::light(),
        };
        palette.apply(&cc.egui_ctx);
        let events = engine.subscribe();
        let mut toasts = Toasts::default();
        if crate::demo::enabled() {
            toasts.push_info("下载完成", "rustup-init");
            toasts.push_links(vec!["https://releases.ubuntu.com/24.04/ubuntu-24.04.3-desktop-amd64.iso".into()]);
        }
        Self {
            engine,
            store,
            palette,
            tasks: Vec::new(),
            filter: Filter::All,
            search: String::new(),
            // 演示模式默认选中第一条，方便截图看到详情面板
            selected: crate::demo::enabled().then_some(crate::demo::SELECTED),
            detail_last: None,
            add_dialog: AddDialog::new(&settings.default_dir),
            show_settings: false,
            default_dir: settings.default_dir,
            limit_enabled: settings.speed_limit_on,
            limit_mbps: settings.speed_limit_mbps,
            progress_style,
            clipboard_watch: settings.clipboard_watch,
            auto_resume: settings.auto_resume,
            toasts,
            clipboard: ClipboardWatcher::new(),
            events,
        }
    }

    /// 把当前 UI 状态写回设置表
    pub fn save_settings(&self) {
        AppSettings {
            theme: self.palette.mode,
            default_dir: self.default_dir.clone(),
            progress_style: self.progress_style,
            clipboard_watch: self.clipboard_watch,
            auto_resume: self.auto_resume,
            speed_limit_on: self.limit_enabled,
            speed_limit_mbps: self.limit_mbps,
        }
        .save(&self.store);
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, mode: Mode) {
        self.palette = match mode {
            Mode::Dark => Palette::dark(),
            Mode::Light => Palette::light(),
        };
        self.palette.apply(ctx);
        self.save_settings();
    }

    pub fn toggle_theme(&mut self, ctx: &egui::Context) {
        let next = match self.palette.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        };
        self.set_theme(ctx, next);
    }

    /// 复制到系统剪贴板并提示；自己复制的链接不触发剪贴板监听
    pub fn copy_link(&mut self, ctx: &egui::Context, url: String) {
        self.clipboard.ignore(&url);
        ctx.copy_text(url.clone());
        self.toasts.push_info("已复制链接", url);
    }

    /// 当前过滤器 + 搜索词下可见的任务
    pub fn visible_tasks(&self) -> Vec<TaskSnapshot> {
        let q = self.search.trim().to_lowercase();
        self.tasks
            .iter()
            .filter(|t| self.filter.matches(&t.status))
            .filter(|t| q.is_empty() || t.file_name.to_lowercase().contains(&q) || t.url.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
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
    fn poll_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(Event::Completed(id)) => {
                    if let Some(t) = self.engine.snapshot_one(id) {
                        self.toasts.push_info("下载完成", t.file_name);
                    }
                }
                Ok(Event::Failed(id, reason)) => {
                    let name = self.engine.snapshot_one(id).map(|t| t.file_name).unwrap_or_default();
                    self.toasts.push_error(format!("下载失败：{name}"), reason);
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    /// 剪贴板里出现新链接 → 吐司询问；新建对话框打开时不打扰
    fn poll_clipboard(&mut self) {
        if !self.clipboard_watch {
            return;
        }
        let urls = self.clipboard.poll();
        if urls.is_empty() || self.add_dialog.open {
            return;
        }
        self.toasts.push_links(urls);
    }
}

pub fn looks_like_url(s: &str) -> bool {
    s.lines().any(|l| {
        let l = l.trim();
        l.starts_with("http://") || l.starts_with("https://")
    })
}

impl eframe::App for SwoopApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tasks = self.engine.snapshot();
        if crate::demo::enabled() {
            self.tasks.extend(crate::demo::tasks(&self.default_dir));
        }
        self.poll_events();
        self.poll_clipboard();
        self.handle_shortcuts(&ctx);

        // 面板顺序决定布局：侧栏占满左侧，状态栏横跨右侧，详情面板在状态栏之上贴右，顶栏只占列表宽度
        self.sidebar(ui);
        self.statusbar(ui);
        self.detail_panel(ui);
        self.topbar(ui);
        self.task_list(ui);
        self.add_dialog_ui(&ctx);
        self.settings_ui(&ctx);
        self.toasts_ui(&ctx);

        // 有任务在跑就 10fps 刷新进度，否则低频；剪贴板轮询也靠这个节拍
        let any_active = self.tasks.iter().any(|t| t.status.is_active());
        ctx.request_repaint_after(if any_active { Duration::from_millis(100) } else { Duration::from_millis(500) });
    }
}
