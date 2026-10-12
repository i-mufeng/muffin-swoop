//! 弹窗骨架：统一外框、标题栏（图标 + 标题 + 说明 + 关闭）、可滚动主体、分组卡片与设置行、底部按钮区。
//!
//! 用法：外框零边距，依次调用 `header` → `body` → `footer`，各段自带边距：
//! ```ignore
//! let modal = dialog::modal("id", &p).show(ctx, |ui| {
//!     ui.set_width(520.0);
//!     close |= dialog::header(ui, &p, icon, title, Some(hint));
//!     dialog::body(ui, "id_body", |ui| dialog::group(ui, &p, None, |ui| { dialog::row(ui, &p, ...); }));
//!     dialog::footer(ui, &p, |ui| { /* 右到左：主按钮先加 */ });
//! });
//! ```

use egui::epaint::Shadow;
use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin, Rect, Response, RichText, Sense,
    Stroke, Ui, UiBuilder, pos2, vec2,
};

use super::icon_button;
use crate::theme::{Mode, Palette, R_CTRL};

/// 底部按钮区高度（上边距 14 + 按钮 34 + 下边距 16）
const FOOTER_H: f32 = 64.0;
/// 弹窗上下离窗口边缘至少留的距离
const EDGE: f32 = 24.0;
const PAD_X: i8 = 24;

/// 弹窗外框：零内边距，内容各段自带边距
pub fn frame(p: &Palette) -> Frame {
    let shadow_alpha = match p.mode {
        Mode::Dark => 140,
        Mode::Light => 46,
    };
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .shadow(Shadow { offset: [0, 16], blur: 44, spread: 0, color: Color32::from_black_alpha(shadow_alpha) })
}

pub fn modal(id: impl std::hash::Hash + std::fmt::Debug, p: &Palette) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(frame(p))
}

/// 非模态窗口（媒体下载、站点抓取等）：无系统标题栏，拖动空白处移动
pub fn window(title: &str, p: &Palette) -> egui::Window<'static> {
    egui::Window::new(title.to_owned()).title_bar(false).collapsible(false).frame(frame(p))
}

/// 标题栏：强调色图标块 + 标题 + 可选说明 + 关闭按钮。返回 true 表示点了关闭。
pub fn header(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: Option<&str>) -> bool {
    header_tinted(ui, p, icon, p.accent, title, subtitle)
}

/// 同 `header`，图标块用指定颜色（警告类弹窗用琥珀色等）
pub fn header_tinted(ui: &mut Ui, p: &Palette, icon: &str, tint: Color32, title: &str, subtitle: Option<&str>) -> bool {
    Frame::new()
        .inner_margin(Margin { left: PAD_X, right: 16, top: 20, bottom: 14 })
        .show(ui, |ui| heading_impl(ui, p, icon, tint, title, subtitle, true))
        .inner
}

/// 独立窗口里的标题：与 `header` 同样式，但不带外边距和关闭按钮（用系统标题栏关闭）
pub fn heading(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: Option<&str>) {
    heading_impl(ui, p, icon, p.accent, title, subtitle, false);
    ui.add_space(14.0);
}

fn heading_impl(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    tint: Color32,
    title: &str,
    subtitle: Option<&str>,
    closable: bool,
) -> bool {
    let width = ui.available_width();
    let text_left = 40.0 + 12.0;
    let text_width = (width - text_left - if closable { 40.0 } else { 0.0 }).max(80.0);
    let title_galley = super::truncated(ui, title, FontId::proportional(17.0), p.text, text_width);
    let sub_galley =
        subtitle.map(|s| ui.painter().layout(s.to_owned(), FontId::proportional(12.5), p.text_muted, text_width));
    let text_h = title_galley.size().y + sub_galley.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, text_h.max(40.0)), Sense::hover());
    let tile = Rect::from_min_size(pos2(rect.left(), rect.center().y - 20.0), vec2(40.0, 40.0));
    let painter = ui.painter();
    painter.rect_filled(tile, CornerRadius::same(11), p.soft(tint));
    painter.text(tile.center(), Align2::CENTER_CENTER, icon, FontId::proportional(19.0), tint);
    let mut y = rect.center().y - text_h / 2.0;
    let title_h = title_galley.size().y;
    painter.galley(pos2(rect.left() + text_left, y), title_galley, p.text);
    y += title_h + 2.0;
    if let Some(g) = sub_galley {
        painter.galley(pos2(rect.left() + text_left, y), g, p.text_muted);
    }
    if !closable {
        return false;
    }
    let close_rect = Rect::from_center_size(pos2(rect.right() - 14.0, rect.top() + 20.0), vec2(28.0, 28.0));
    ui.put(close_rect, |ui: &mut Ui| icon_button(ui, p, egui_phosphor::regular::X, swoop_i18n::tr!("关闭"))).clicked()
}

/// 可滚动主体；窗口矮时只滚动这一段，底部按钮始终可见
pub fn body<R>(ui: &mut Ui, id_salt: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    body_reserving(ui, id_salt, 0.0, add)
}

/// 同 `body`，主体与底部按钮之间还有不随主体滚动的区域（进度、列表等）时用，`below` 是那块的高度
pub fn body_reserving<R>(ui: &mut Ui, id_salt: &str, below: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    // 主体之上已画的部分（标题栏、固定区）按实际高度扣，副标题换行或多一行提示时不会把底部按钮挤出窗口
    let above = ui.min_rect().height();
    let max_height = (ui.ctx().content_rect().height() - 2.0 * EDGE - above - below - FOOTER_H).max(140.0);
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin { left: PAD_X, right: PAD_X, top: 2, bottom: 6 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui)
                })
                .inner
        })
        .inner
}

/// 不滚动的主体（内容很短，或自己管理滚动）
pub fn body_fixed<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .inner_margin(Margin { left: PAD_X, right: PAD_X, top: 2, bottom: 6 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// 底部按钮区：上方一条分隔线，按钮右对齐。`add` 在右到左布局里执行，先加的在最右（主按钮先加）。
pub fn footer(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter().hline(x, top, Stroke::new(1.0, p.border));
    Frame::new().inner_margin(Margin { left: PAD_X, right: PAD_X, top: 14, bottom: 16 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            add(ui);
        });
    });
}

/// 分组标题（卡片上方的小标题）
pub fn group_title(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(12.5).strong().color(p.text_muted));
    ui.add_space(2.0);
}

/// 分组卡片：浅底圆角框，里面放若干 `row`，行间自动加分隔线
pub fn group<R>(ui: &mut Ui, p: &Palette, title: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    if let Some(title) = title {
        group_title(ui, p, title);
    }
    let inner = Frame::new()
        .fill(p.group)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner;
    ui.add_space(12.0);
    inner
}

/// 行间分隔线：卡片里第一行之前不画
fn divider_if_needed(ui: &mut Ui, p: &Palette) {
    if ui.min_rect().height() >= 1.0 {
        let y = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
    }
}

fn label_galleys(
    ui: &Ui,
    p: &Palette,
    label: &str,
    hint: Option<&str>,
    wrap: f32,
) -> (std::sync::Arc<egui::Galley>, Option<std::sync::Arc<egui::Galley>>, f32) {
    let label = ui.painter().layout(label.to_owned(), FontId::proportional(13.5), p.text, wrap);
    let hint = hint.map(|h| ui.painter().layout(h.to_owned(), FontId::proportional(12.0), p.text_muted, wrap));
    let h = label.size().y + hint.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    (label, hint, h)
}

fn paint_label(
    ui: &Ui,
    rect: Rect,
    label: std::sync::Arc<egui::Galley>,
    hint: Option<std::sync::Arc<egui::Galley>>,
    h: f32,
    p: &Palette,
) {
    let mut y = rect.center().y - h / 2.0;
    let lh = label.size().y;
    ui.painter().galley(pos2(rect.left(), y), label, p.text);
    y += lh + 2.0;
    if let Some(g) = hint {
        ui.painter().galley(pos2(rect.left(), y), g, p.text_muted);
    }
}

/// 设置行：左边标题 + 说明，右边控件（右到左布局，先加的在最右）。控件应为单行高度。
pub fn row<R>(ui: &mut Ui, p: &Palette, label: &str, hint: Option<&str>, control: impl FnOnce(&mut Ui) -> R) -> R {
    row_with(ui, p, label, hint, 0.45, control)
}

/// 同 `row`，`label_share` 指定左侧文字最多占行宽的比例
pub fn row_with<R>(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    hint: Option<&str>,
    label_share: f32,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    let wrap = ui.available_width() * label_share;
    let style = RowStyle { wrap, min_label: 0.0, layout: Layout::right_to_left(Align::Center) };
    row_impl(ui, p, label, hint, style, control)
}

/// 表单行：左侧标题列固定宽度，控件左对齐（左到右布局），多行之间输入框左边对齐
pub fn field_row<R>(ui: &mut Ui, p: &Palette, label: &str, label_width: f32, control: impl FnOnce(&mut Ui) -> R) -> R {
    let style = RowStyle { wrap: label_width, min_label: label_width, layout: Layout::left_to_right(Align::Center) };
    row_impl(ui, p, label, None, style, control)
}

/// 行的排版参数：左侧文字折行宽度、标题列最小宽度、控件区布局
struct RowStyle {
    wrap: f32,
    min_label: f32,
    layout: Layout,
}

fn row_impl<R>(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    hint: Option<&str>,
    style: RowStyle,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    let RowStyle { wrap, min_label, layout } = style;
    divider_if_needed(ui, p);
    let width = ui.available_width();
    let (label, hint, text_h) = label_galleys(ui, p, label, hint, wrap);
    let label_w = label.size().x.max(hint.as_ref().map_or(0.0, |g| g.size().x)).max(min_label);
    let height = (text_h + 22.0).max(50.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    paint_label(ui, rect, label, hint, text_h, p);
    let control_rect = Rect::from_min_max(pos2(rect.left() + label_w + 16.0, rect.top()), rect.max);
    ui.scope_builder(UiBuilder::new().max_rect(control_rect).layout(layout), control).inner
}

/// 上下结构的行：标题 + 说明在上，内容占满整行在下（多行控件、长输入框）
pub fn row_stack<R>(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    hint: Option<&str>,
    content: impl FnOnce(&mut Ui) -> R,
) -> R {
    divider_if_needed(ui, p);
    ui.add_space(12.0);
    let width = ui.available_width();
    let (label, hint, text_h) = label_galleys(ui, p, label, hint, width);
    let (rect, _) = ui.allocate_exact_size(vec2(width, text_h), Sense::hover());
    paint_label(ui, rect, label, hint, text_h, p);
    ui.add_space(8.0);
    let inner = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            content(ui)
        })
        .inner;
    ui.add_space(12.0);
    inner
}

/// 跳转行：整行可点，右侧带 ›，用于打开下一级设置
pub fn link_row(ui: &mut Ui, p: &Palette, icon: &str, label: &str, hint: Option<&str>) -> Response {
    divider_if_needed(ui, p);
    let width = ui.available_width();
    let (label, hint, text_h) = label_galleys(ui, p, label, hint, width - 80.0);
    let height = (text_h + 22.0).max(50.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), super::anim_secs(0.12));
        if t > 0.01 {
            let bg = rect.expand2(vec2(14.0, 0.0));
            ui.painter().rect_filled(bg, CornerRadius::ZERO, p.hover_tint.gamma_multiply(t));
        }
        ui.painter().text(
            pos2(rect.left() + 9.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(17.0),
            p.text_muted.lerp_to_gamma(p.accent, t),
        );
        paint_label(ui, rect.with_min_x(rect.left() + 30.0), label, hint, text_h, p);
        ui.painter().text(
            pos2(rect.right() - 2.0, rect.center().y),
            Align2::RIGHT_CENTER,
            egui_phosphor::regular::CARET_RIGHT,
            FontId::proportional(14.0),
            p.text_muted,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 行内说明 / 错误提示（卡片外）
pub fn note(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(p.text_muted));
}

pub fn error(ui: &mut Ui, p: &Palette, text: &str) {
    Frame::new()
        .fill(p.danger_soft)
        .corner_radius(CornerRadius::same(R_CTRL))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(egui_phosphor::regular::WARNING_CIRCLE).size(15.0).color(p.danger));
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.danger)).wrap());
            });
        });
    ui.add_space(8.0);
}
