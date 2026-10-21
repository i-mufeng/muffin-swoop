//! 自绘的基础控件：图标按钮、实心/描边按钮、胶囊标签、开关、复选框、分段控件、导航行、文件类型色块。
//! 全部走 `Palette` 令牌，带 120–150ms 的悬停过渡。弹窗骨架在 `dialog`。

pub mod add_dialog;
pub mod categories;
pub mod completion;
pub mod detail;
pub mod dialog;
pub mod download_card;
pub mod file_conflict;
pub mod filekind;
pub mod format;
pub mod grabber;
pub mod media;
mod media_proxy;
pub mod network;
pub mod popup;
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
pub mod task_window;
pub mod toast;
pub mod topbar;
#[cfg(windows)]
pub mod welcome;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Rect, Response, RichText, Sense, Stroke, TextStyle, Ui,
    WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};

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

/// 是否处于自动截图模式（EFRAME_SCREENSHOT_TO，或 `screenshot` 功能的 SWOOP_SHOT）
fn screenshot_mode() -> bool {
    static SHOT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SHOT.get_or_init(|| {
        #[cfg(feature = "screenshot")]
        if crate::shot::active() {
            return true;
        }
        std::env::var_os("EFRAME_SCREENSHOT_TO").is_some()
    })
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
    let size = vec2(galley.size().x + 28.0, 34.0);
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

/// 独立窗口的主体：占满当前 ui 剩余高度，只留出下方 `below`（底部按钮行）的位置，超出时滚动。
/// 按剩余高度而不是窗口高度减常数算，标题多一行提示或窗口被拉高时都不会留白或挤掉按钮。
/// 右侧留出滚动条宽度，滚动条不压在输入框和卡片边框上。
pub fn modal_body<R>(ui: &mut Ui, id: &str, below: f32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let max_height = (ui.available_height() - below).max(80.0);
    let gutter = ui.spacing().scroll.bar_width + 4.0;
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 0, right: gutter as i8, top: 0, bottom: 4 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add_contents(ui)
                })
                .inner
        })
        .inner
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

/// 危险操作按钮（描边 + 红字，悬停泛红）
pub fn danger_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    text_button(ui, text, p.surface, p.danger_soft, p.danger, Some(p.border))
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
    let (rect, mut resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(11), p.toggle_off.lerp_to_gamma(p.accent, t));
        let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
        painter.circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 单行文字，超出 `max_width` 时以省略号截断
fn truncated(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// 复选框：圆角方块，勾选后填强调色、白色对勾。颜色取自当前 `Visuals`，调用处不必传 `Palette`；
/// 不用 egui 自带的，是因为它沿用控件圆角，16px 的方块会被画成圆圈，看起来像单选框。
pub struct Check<'a> {
    checked: &'a mut bool,
    text: WidgetText,
}

impl<'a> Check<'a> {
    pub fn new(checked: &'a mut bool, text: impl Into<WidgetText>) -> Self {
        Self { checked, text: text.into() }
    }
}

impl egui::Widget for Check<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        const BOX: f32 = 16.0;
        const GAP: f32 = 8.0;
        let has_text = !self.text.is_empty();
        let wrap_width = ui.available_width() - BOX - GAP;
        let galley = self.text.into_galley(ui, Some(egui::TextWrapMode::Extend), wrap_width, TextStyle::Body);
        let width = BOX + if has_text { GAP + galley.size().x } else { 0.0 };
        let height = ui.spacing().interact_size.y.max(galley.size().y);
        let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        if resp.clicked() {
            *self.checked = !*self.checked;
            resp.mark_changed();
        }
        let checked = *self.checked;
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), checked, galley.text()));
        if ui.is_rect_visible(rect) {
            let t = hover_t(ui, &resp);
            let on = ui.ctx().animate_bool_with_time(resp.id.with("on"), checked, anim_secs(0.12));
            let v = ui.visuals();
            let accent = v.selection.stroke.color;
            let text_color = v.override_text_color.unwrap_or_else(|| v.text_color());
            let border = v.widgets.inactive.bg_stroke.color.lerp_to_gamma(accent, (t * 0.7).max(on));
            let fill = v.text_edit_bg_color().lerp_to_gamma(accent, on);
            let b = Rect::from_center_size(pos2(rect.left() + BOX / 2.0, rect.center().y), vec2(BOX, BOX));
            let painter = ui.painter();
            painter.rect(b, CornerRadius::same(4), fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
            if on > 0.01 {
                let c = Color32::WHITE.gamma_multiply(on);
                painter.line(
                    vec![
                        pos2(b.left() + 3.8, b.center().y + 0.2),
                        pos2(b.left() + 6.7, b.bottom() - 4.2),
                        pos2(b.right() - 3.6, b.top() + 4.4),
                    ],
                    Stroke::new(1.8, c),
                );
            }
            if has_text {
                let pos = pos2(b.right() + GAP, rect.center().y - galley.size().y / 2.0);
                painter.galley(pos, galley, text_color);
            }
        }
        resp.on_hover_cursor(CursorIcon::PointingHand)
    }
}

/// `ui.checkbox` 的替代
pub fn checkbox(ui: &mut Ui, checked: &mut bool, text: impl Into<WidgetText>) -> Response {
    ui.add(Check::new(checked, text))
}

/// 分段控件：凹槽里一排选项，选中项凸起。替代成排的 `selectable_label`。
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, p: &Palette, value: &mut T, options: &[(T, &str)]) -> Response {
    let options: Vec<_> = options.iter().map(|(v, label)| (v.clone(), *label, true)).collect();
    segmented_with(ui, p, value, &options)
}

/// 同 `segmented`，第三项为 false 的选项置灰不可选
pub fn segmented_with<T: PartialEq + Clone>(
    ui: &mut Ui,
    p: &Palette,
    value: &mut T,
    options: &[(T, &str, bool)],
) -> Response {
    const PAD: f32 = 3.0;
    const H: f32 = 32.0;
    let font = FontId::proportional(13.0);
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, label, _)| ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), p.text))
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 26.0).collect();
    let total = widths.iter().sum::<f32>() + PAD * 2.0;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(total, H), Sense::hover());
    let mut x = rect.left() + PAD;
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(R_CTRL), p.inset);
    }
    for (i, ((option, _, enabled), galley)) in options.iter().zip(galleys).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + PAD), vec2(widths[i], H - PAD * 2.0));
        x += widths[i];
        let sense = if *enabled { Sense::click() } else { Sense::hover() };
        let item = ui.interact(r, resp.id.with(i), sense);
        let selected = *value == *option;
        if item.clicked() && !selected {
            *value = option.clone();
            resp.mark_changed();
        }
        if ui.is_rect_visible(r) {
            let t = hover_t(ui, &item);
            let painter = ui.painter();
            if selected {
                let stroke = match p.mode {
                    crate::theme::Mode::Light => Stroke::new(1.0, p.border),
                    crate::theme::Mode::Dark => Stroke::NONE,
                };
                painter.rect(r, CornerRadius::same(6), p.raised, stroke, egui::StrokeKind::Inside);
            } else if t > 0.01 {
                painter.rect_filled(r, CornerRadius::same(6), p.hover_tint.gamma_multiply(t));
            }
            let color = if selected {
                p.text
            } else if *enabled {
                p.text_muted.lerp_to_gamma(p.text, t)
            } else {
                p.text_muted.gamma_multiply(0.45)
            };
            painter.galley_with_override_text_color(r.center() - galley.size() / 2.0, galley, color);
        }
        if *enabled {
            item.on_hover_cursor(CursorIcon::PointingHand);
        }
    }
    resp
}

/// 整行导航项（侧栏、设置页分类）：宽度占满，图标 + 文字 + 右侧计数。`tint` 给图标固定颜色。
pub fn nav_item(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    tint: Option<Color32>,
    label: &str,
    badge: Option<&str>,
    selected: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let t = hover_t(ui, &resp);
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, CornerRadius::same(R_CTRL), p.accent_soft);
        } else if t > 0.01 {
            painter.rect_filled(rect, CornerRadius::same(R_CTRL), p.hover_tint.gamma_multiply(t));
        }
        let fg = if selected { p.text } else { p.text_muted.lerp_to_gamma(p.text, t) };
        let icon_color = tint.unwrap_or(if selected { p.accent } else { fg });
        painter.text(
            pos2(rect.left() + 20.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(ICON_MD),
            icon_color,
        );
        let mut right = rect.right() - 8.0;
        if let Some(badge) = badge {
            let (bg, color) = if selected { (p.accent, Color32::WHITE) } else { (p.soft(p.text_muted), p.text_muted) };
            let galley = painter.layout_no_wrap(badge.to_owned(), FontId::proportional(11.5), color);
            let w = (galley.size().x + 12.0).max(20.0);
            let pill = Rect::from_min_size(pos2(right - w, rect.center().y - 9.0), vec2(w, 18.0));
            painter.rect_filled(pill, CornerRadius::same(9), bg);
            painter.galley(pill.center() - galley.size() / 2.0, galley, color);
            right = pill.left() - 6.0;
        }
        let left = rect.left() + 38.0;
        let galley = truncated(ui, label, FontId::proportional(13.5), fg, right - left);
        ui.painter().galley(pos2(left, rect.center().y - galley.size().y / 2.0), galley, fg);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 工具栏按钮：34px 高的描边按钮，图标 + 可选文字 + 可选下拉箭头。配合 `egui::Popup::menu` 弹出菜单。
pub fn tool_button(ui: &mut Ui, p: &Palette, icon: &str, text: Option<&str>, chevron: bool) -> Response {
    let font = FontId::proportional(13.5);
    let galley = text.map(|t| ui.painter().layout_no_wrap(t.to_owned(), font, p.text));
    let mut width = 12.0 + ICON_MD + 12.0;
    if let Some(g) = &galley {
        width += g.size().x + 6.0;
    }
    if chevron {
        width += 14.0;
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(34.0), 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let t = hover_t(ui, &resp);
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        let fill = p.surface.lerp_to_gamma(p.surface_hover, if open { 1.0 } else { t });
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::same(R_CTRL), fill, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        let fg = p.text_muted.lerp_to_gamma(p.text, if open { 1.0 } else { t });
        let mut x = if galley.is_none() && !chevron { rect.center().x - ICON_MD / 2.0 } else { rect.left() + 12.0 };
        painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, icon, FontId::proportional(ICON_MD), fg);
        x += ICON_MD + 6.0;
        if let Some(g) = galley {
            let w = g.size().x;
            painter.galley_with_override_text_color(pos2(x, rect.center().y - g.size().y / 2.0), g, p.text);
            x += w + 6.0;
        }
        if chevron {
            painter.text(
                pos2(x, rect.center().y + 1.0),
                Align2::LEFT_CENTER,
                egui_phosphor::regular::CARET_DOWN,
                FontId::proportional(11.0),
                p.text_muted,
            );
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 统一高度的单行输入框
pub fn field(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(10, 7))
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
