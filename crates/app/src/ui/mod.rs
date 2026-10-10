pub mod add_dialog;
pub mod format;
pub mod settings;
pub mod sidebar;
pub mod statusbar;
pub mod task_list;
pub mod topbar;

use egui::{Color32, CornerRadius, Response, RichText, Sense, Ui, Vec2, vec2};

use crate::theme::Palette;

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
            Filter::All => "全部",
            Filter::Active => "下载中",
            Filter::Completed => "已完成",
            Filter::Failed => "失败",
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Filter::All => i::LIST,
            Filter::Active => i::DOWNLOAD_SIMPLE,
            Filter::Completed => i::CHECK_CIRCLE,
            Filter::Failed => i::WARNING_CIRCLE,
        }
    }

    pub fn matches(self, status: &swoop_core::TaskStatus) -> bool {
        use swoop_core::TaskStatus as S;
        match self {
            Filter::All => true,
            Filter::Active => matches!(status, S::Queued | S::Probing | S::Downloading | S::Paused),
            Filter::Completed => matches!(status, S::Completed),
            Filter::Failed => matches!(status, S::Failed(_)),
        }
    }
}

/// 只有图标的小按钮（30x30），hover 时出现浅底。
pub fn icon_button(ui: &mut Ui, p: &Palette, icon: &str, tooltip: &str) -> Response {
    icon_button_sized(ui, p, icon, tooltip, vec2(30.0, 30.0), p.text_muted)
}

fn icon_button_sized(ui: &mut Ui, p: &Palette, icon: &str, tooltip: &str, size: Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() || resp.is_pointer_button_down_on() {
            painter.rect_filled(rect, CornerRadius::same(8), p.surface_hover);
        }
        let color = if resp.hovered() { p.text } else { color };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(17.0),
            color,
        );
    }
    resp.on_hover_text(tooltip)
}

/// 主操作按钮（强调色填充）
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(p.accent)
            .stroke(egui::Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(88.0, 32.0)),
    )
}

/// 次级按钮（描边）
pub fn ghost_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(p.text))
            .fill(p.surface)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(88.0, 32.0)),
    )
}
