//! 自绘的基础控件：图标按钮、实心/描边按钮、胶囊标签、开关、文件类型色块。
//! 全部走 `Palette` 令牌，带 120–150ms 的悬停过渡。

pub mod add_dialog;
pub mod categories;
pub mod completion;
pub mod detail;
pub mod file_conflict;
pub mod filekind;
pub mod format;
pub mod grabber;
pub mod media;
mod media_proxy;
pub mod network;
pub mod progress;
pub mod queues;
pub mod quota;
pub mod settings;
pub mod sidebar;
pub mod site_auth;
pub mod statusbar;
pub mod task_list;
pub mod task_properties;
pub mod task_table;
pub mod toast;
pub mod topbar;
pub mod updates;

use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Response, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::fonts;
use crate::theme::{ICON_MD, Palette, R_CTRL, R_TILE};
use filekind::FileKind;

/// 列表过滤器 / 侧栏分组
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Active,
    Completed,
    Failed,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::Active, Filter::Completed, Filter::Failed];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => swoop_i18n::tr!("全部"),
            Filter::Active => swoop_i18n::tr!("下载中"),
            Filter::Completed => swoop_i18n::tr!("已完成"),
            Filter::Failed => swoop_i18n::tr!("失败"),
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Filter::All => i::TRAY_ARROW_DOWN,
            Filter::Active => i::ARROW_CIRCLE_DOWN,
            Filter::Completed => i::CHECK_CIRCLE,
            Filter::Failed => i::WARNING_CIRCLE,
        }
    }

    pub fn matches(self, status: &swoop_core::TaskStatus) -> bool {
        use swoop_core::TaskStatus as S;
        match self {
            Filter::All => true,
            Filter::Active => matches!(status, S::Queued | S::Probing | S::Downloading | S::Paused | S::FileConflict),
            Filter::Completed => matches!(status, S::Completed),
            Filter::Failed => matches!(status, S::Failed(_)),
        }
    }
}

/// Regular 图标文本，可与普通文字混排
pub fn icon_text(glyph: &str, size: f32, color: Color32) -> RichText {
    RichText::new(glyph).size(size).color(color)
}

/// 是否处于自动截图模式（EFRAME_SCREENSHOT_TO）
fn screenshot_mode() -> bool {
    static SHOT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SHOT.get_or_init(|| std::env::var_os("EFRAME_SCREENSHOT_TO").is_some())
}

/// 动画时长；截图模式下近似为 0，让第 2 帧就呈现最终状态。
/// 不能真的给 0：egui 内部会算 elapsed / duration，得到 NaN。
pub fn anim_secs(secs: f32) -> f32 {
    if screenshot_mode() { 0.001 } else { secs }
}

/// 悬停过渡系数 0..1
fn hover_t(ui: &Ui, resp: &Response) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), anim_secs(0.12))
}

/// 28×28 图标按钮，悬停出现浅底，颜色从 muted 过渡到 text。
pub fn icon_button(ui: &mut Ui, p: &Palette, glyph: &str, tooltip: &str) -> Response {
    icon_button_colored(ui, p, glyph, tooltip, p.text_muted, p.text)
}

pub fn icon_button_colored(
    ui: &mut Ui,
    p: &Palette,
    glyph: &str,
    tooltip: &str,
    color: Color32,
    hover_color: Color32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let t = hover_t(ui, &resp);
        let painter = ui.painter();
        if t > 0.01 {
            painter.rect_filled(rect, CornerRadius::same(7), p.hover_tint.gamma_multiply(t));
        }
        let c = color.lerp_to_gamma(hover_color, t);
        painter.text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(ICON_MD), c);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tooltip)
}

fn text_button(
    ui: &mut Ui,
    text: &str,
    fill: Color32,
    fill_hover: Color32,
    fg: Color32,
    stroke: Option<Color32>,
) -> Response {
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = vec2(galley.size().x + 26.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let t = hover_t(ui, &resp);
        let mut bg = fill.lerp_to_gamma(fill_hover, t);
        if resp.is_pointer_button_down_on() {
            bg = bg.gamma_multiply(0.88);
        }
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(R_CTRL), bg);
        if let Some(s) = stroke {
            painter.rect_stroke(rect, CornerRadius::same(R_CTRL), Stroke::new(1.0, s), egui::StrokeKind::Inside);
        }
        let pos = rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, fg);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 弹窗主体：超出窗口可用高度时滚动。`reserve` 是标题、底部按钮和弹窗边距占用的高度，
/// 底部按钮放在主体之外，窗口再矮也点得到。
pub fn modal_body<R>(ui: &mut Ui, id: &str, reserve: f32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let max_height = (ui.ctx().content_rect().height() - reserve).max(120.0);
    egui::ScrollArea::vertical().id_salt(id).max_height(max_height).show(ui, add_contents).inner
}

/// 主操作按钮（强调色填充）
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let hover = p.accent.lerp_to_gamma(Color32::WHITE, 0.12);
    text_button(ui, text, p.accent, hover, Color32::WHITE, None)
}

/// 次级按钮（描边）
pub fn ghost_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    text_button(ui, text, p.surface, p.surface_hover, p.text, Some(p.border))
}

/// 小胶囊标签
pub fn pill(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = vec2(galley.size().x + 16.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(10), bg);
        painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    resp
}

/// iOS 风格开关
pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(10), p.track.lerp_to_gamma(p.accent, t));
        let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        painter.circle_filled(pos2(x, rect.center().y), 7.0, Color32::WHITE);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 文件类型色块：圆角方块 + Fill 图标
pub fn file_tile(ui: &mut Ui, p: &Palette, kind: FileKind, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let tint = kind.tint(p);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(R_TILE), p.soft(tint));
        painter.text(rect.center(), Align2::CENTER_CENTER, kind.icon(), fonts::icon_fill(size * 0.5), tint);
    }
}
/// Localize only the built-in default label; user-renamed queues keep their stored names.
pub fn queue_name(queue: &swoop_core::QueueSnapshot) -> &str {
    if queue.id == swoop_core::MAIN_QUEUE_ID && queue.name == "主队列" {
        swoop_i18n::tr!("主队列")
    } else {
        &queue.name
    }
}
