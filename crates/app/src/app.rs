use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, Modifiers};
use swoop_core::{Engine, TaskId, TaskSnapshot};

use crate::theme::{Mode, Palette};
use crate::ui::Filter;
use crate::ui::add_dialog::AddDialog;

pub struct SwoopApp {
    pub engine: Arc<Engine>,
    pub palette: Palette,
    /// 本帧的任务快照（每帧从引擎刷新）
    pub tasks: Vec<TaskSnapshot>,
    pub filter: Filter,
    pub search: String,
    pub selected: Option<TaskId>,
    pub add_dialog: AddDialog,
    pub show_settings: bool,
    pub default_dir: PathBuf,
    pub limit_enabled: bool,
    pub limit_mbps: f64,
}

impl SwoopApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Arc<Engine>) -> Self {
        let palette = Palette::dark();
        palette.apply(&cc.egui_ctx);
        let default_dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            engine,
            palette,
            tasks: Vec::new(),
            filter: Filter::All,
            search: String::new(),
            selected: None,
            add_dialog: AddDialog::new(&default_dir),
            show_settings: false,
            default_dir,
            limit_enabled: false,
            limit_mbps: 5.0,
        }
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, mode: Mode) {
        self.palette = match mode {
            Mode::Dark => Palette::dark(),
            Mode::Light => Palette::light(),
        };
        self.palette.apply(ctx);
    }

    pub fn toggle_theme(&mut self, ctx: &egui::Context) {
        let next = match self.palette.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        };
        self.set_theme(ctx, next);
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
        self.handle_shortcuts(&ctx);

        // 面板顺序决定布局：侧栏占满左侧，状态栏/顶栏只占右侧剩余宽度
        self.sidebar(ui);
        self.statusbar(ui);
        self.topbar(ui);
        self.task_list(ui);
        self.add_dialog_ui(&ctx);
        self.settings_ui(&ctx);

        // 有任务在跑就 10fps 刷新进度，否则低频
        let any_active = self.tasks.iter().any(|t| t.status.is_active());
        ctx.request_repaint_after(if any_active { Duration::from_millis(100) } else { Duration::from_millis(500) });
    }
}
