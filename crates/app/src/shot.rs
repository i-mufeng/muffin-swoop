//! 开发用截图（`--features screenshot`）：启动后打开指定界面，等界面稳定后截下主窗口存成 PNG 并退出。
//!
//! 与 eframe 自带的 `EFRAME_SCREENSHOT_TO`（只抓首帧，看不到 Modal、吐司等 `Area`）不同，这里等若干帧后
//! 发 `ViewportCommand::Screenshot`，wgpu 后端也能用，弹窗和吐司都在图里。独立窗口（新建下载、进度窗口、
//! 完成窗口）在截图模式下改画进主窗口中央（见 `popup::show`），右下角下载卡片画在主窗口右下角，一起截下来。
//!
//! 环境变量：`SWOOP_SHOT=<输出.png>`，`SWOOP_SHOT_VIEW=<视图>`（见 [`VIEWS`]）。一般通过
//! `scripts/screenshot.sh` 调用：它用临时数据目录和演示数据，不碰真实下载记录，也不会被已运行的实例接管。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use egui::ViewportCommand;

use crate::app::SwoopApp;

/// 可截取的视图
pub const VIEWS: &[&str] = &[
    "home",
    "settings",
    "settings:general",
    "settings:download",
    "settings:notify",
    "settings:integration",
    "settings:about",
    "queues",
    "categories",
    "network",
    "quota",
    "completion",
    "site-auth",
    "updates",
    "media",
    "grabber",
    "add",
    "add-pending",
    "task-window",
    "cards",
    "completed",
];

/// 打开视图的帧（前几帧窗口和字体还在初始化）
const OPEN_AT: u32 = 3;
/// 发截图命令的帧：弹窗需要一帧测量尺寸，留足余量
const SHOOT_AT: u32 = 30;
/// 没收到截图时重新请求的间隔（帧）
const RETRY_EVERY: u32 = 60;
/// 超过这么多帧还没收到截图就放弃
const GIVE_UP_AT: u32 = 600;

static FRAME: AtomicU32 = AtomicU32::new(0);

fn output() -> Option<&'static PathBuf> {
    static OUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    OUT.get_or_init(|| std::env::var_os("SWOOP_SHOT").filter(|v| !v.is_empty()).map(PathBuf::from)).as_ref()
}

/// 是否处于截图模式
pub fn active() -> bool {
    output().is_some()
}

/// 每帧 `logic` 调用
pub fn tick(app: &mut SwoopApp, ctx: &egui::Context) {
    let Some(out) = output() else {
        return;
    };
    let frame = FRAME.fetch_add(1, Ordering::SeqCst);
    // 持续重绘，不等事件
    ctx.request_repaint();
    if frame == OPEN_AT {
        let view = std::env::var("SWOOP_SHOT_VIEW").unwrap_or_else(|_| "home".into());
        if !open_view(app, ctx, &view) {
            eprintln!("未知视图 {view:?}，可用：{}", VIEWS.join(", "));
            std::process::exit(2);
        }
    }
    // 偶尔会收不到截图事件，没收到就每隔一段时间再请求一次
    if frame >= SHOOT_AT && (frame - SHOOT_AT).is_multiple_of(RETRY_EVERY) {
        ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
    }
    if frame > SHOOT_AT {
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            std::process::exit(match save(out, &image) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("截图保存失败：{error}");
                    1
                }
            });
        }
    }
    if frame > GIVE_UP_AT {
        eprintln!("等待截图超时");
        std::process::exit(1);
    }
}

fn open_view(app: &mut SwoopApp, ctx: &egui::Context, view: &str) -> bool {
    match view {
        "home" => {}
        "settings" => app.show_settings = true,
        "queues" => app.queue_editor.show(swoop_core::MAIN_QUEUE_ID),
        "categories" => app.category_editor = Some(app.categories.clone()),
        "network" => {
            app.network_editor = Some(crate::ui::network::NetworkEditor::new(app.engine.network_policy()));
        }
        "quota" => app.quota_editor = Some(app.engine.quota().policy),
        "completion" => {
            app.completion_editor = Some(crate::ui::completion::CompletionEditor::new(&app.completion.settings));
        }
        "site-auth" => app.site_auth_editor = Some(crate::ui::site_auth::SiteAuthEditor::new(&app.engine)),
        "updates" => app.updates.open = true,
        "media" => app.media_window.open = true,
        "grabber" => app.grabber.open = true,
        "add" => app.add_dialog.open_with(None),
        // 浏览器刚交来、还在探测的下载
        "add-pending" => app.add_dialog.begin_from_browser(
            1,
            "https://releases.ubuntu.com/24.04/ubuntu-24.04.3-desktop-amd64.iso".into(),
            Vec::new(),
        ),
        // 演示模式下默认选中的那条假任务
        "task-window" => app.task_windows.open(ctx, crate::demo::SELECTED),
        // 自下而上：已完成、失败、下载中，再多一张排队的，看「+1」
        "cards" => {
            app.toasts.items.clear();
            for id in [9006, 9008, 9001, 9002] {
                app.cards.open(swoop_core::TaskId(id), None);
            }
        }
        "completed" => app.completion.dialogs.push_back(app.default_dir.join("ubuntu-24.04.3-desktop-amd64.iso")),
        other => match other.strip_prefix("settings:") {
            Some(tab) => return app.show_settings_tab(ctx, tab),
            None => return false,
        },
    }
    true
}

fn save(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    let buffer = image::RgbaImage::from_raw(w as u32, h as u32, bytes).ok_or("截图尺寸与像素数不符")?;
    buffer.save(path).map_err(|e| e.to_string())
}
