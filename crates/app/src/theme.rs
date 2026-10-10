//! 设计令牌与 egui 样式映射。所有颜色只在这里定义。

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub mode: Mode,
    /// 窗口最底层背景
    pub bg: Color32,
    /// 侧栏/面板
    pub panel: Color32,
    /// 卡片
    pub surface: Color32,
    pub surface_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    /// 预留给排队/限速等提示状态
    #[allow(dead_code)]
    pub warning: Color32,
    pub danger: Color32,
    /// 进度条轨道
    pub track: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            bg: Color32::from_rgb(0x0f, 0x11, 0x15),
            panel: Color32::from_rgb(0x14, 0x17, 0x1e),
            surface: Color32::from_rgb(0x1b, 0x1f, 0x29),
            surface_hover: Color32::from_rgb(0x22, 0x27, 0x33),
            border: Color32::from_rgb(0x2a, 0x30, 0x3d),
            text: Color32::from_rgb(0xe8, 0xeb, 0xf2),
            text_muted: Color32::from_rgb(0x8a, 0x93, 0xa6),
            accent: Color32::from_rgb(0x6c, 0x8c, 0xff),
            accent_soft: Color32::from_rgba_unmultiplied(0x6c, 0x8c, 0xff, 0x30),
            success: Color32::from_rgb(0x3e, 0xcf, 0x8e),
            warning: Color32::from_rgb(0xf5, 0xb1, 0x4c),
            danger: Color32::from_rgb(0xff, 0x6b, 0x6b),
            track: Color32::from_rgb(0x2a, 0x30, 0x3d),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: Mode::Light,
            bg: Color32::from_rgb(0xf4, 0xf5, 0xf8),
            panel: Color32::from_rgb(0xed, 0xef, 0xf4),
            surface: Color32::WHITE,
            surface_hover: Color32::from_rgb(0xf6, 0xf7, 0xfb),
            border: Color32::from_rgb(0xdd, 0xe1, 0xea),
            text: Color32::from_rgb(0x17, 0x1b, 0x24),
            text_muted: Color32::from_rgb(0x6b, 0x74, 0x86),
            accent: Color32::from_rgb(0x4f, 0x72, 0xf5),
            accent_soft: Color32::from_rgba_unmultiplied(0x4f, 0x72, 0xf5, 0x26),
            success: Color32::from_rgb(0x1f, 0xa9, 0x71),
            warning: Color32::from_rgb(0xd9, 0x8c, 0x1e),
            danger: Color32::from_rgb(0xe0, 0x4f, 0x4f),
            track: Color32::from_rgb(0xe3, 0xe6, 0xee),
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = match self.mode {
            Mode::Dark => Visuals::dark(),
            Mode::Light => Visuals::light(),
        };
        v.panel_fill = self.panel;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.surface;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.window_corner_radius = CornerRadius::same(14);
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(90) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(70) };
        v.menu_corner_radius = CornerRadius::same(10);
        v.selection.bg_fill = self.accent_soft;
        v.selection.stroke = Stroke::new(1.0, self.accent);

        let r = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.surface;
        w.noninteractive.weak_bg_fill = self.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text_muted);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = self.surface;
        w.inactive.weak_bg_fill = self.surface;
        w.inactive.bg_stroke = Stroke::new(1.0, self.border);
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = self.surface_hover;
        w.hovered.weak_bg_fill = self.surface_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, self.border);
        w.hovered.fg_stroke = Stroke::new(1.0, self.text);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = self.accent_soft;
        w.active.weak_bg_fill = self.accent_soft;
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        w.active.fg_stroke = Stroke::new(1.0, self.text);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = self.surface_hover;
        w.open.weak_bg_fill = self.surface_hover;
        w.open.bg_stroke = Stroke::new(1.0, self.border);
        w.open.fg_stroke = Stroke::new(1.0, self.text);
        w.open.corner_radius = r;

        ctx.set_visuals(v);
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(12.0, 6.0);
            s.spacing.interact_size.y = 30.0;
            s.spacing.window_margin = egui::Margin::same(18);
            s.spacing.menu_margin = egui::Margin::same(8);
            s.interaction.selectable_labels = false;
            s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
            s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
        });
    }
}
