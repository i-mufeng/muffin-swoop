//! 设计令牌与 egui 样式映射。所有颜色、圆角、图标尺寸只在这里定义。

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// 圆角（像素）
pub const R_CARD: u8 = 12;
pub const R_CTRL: u8 = 8;
pub const R_TILE: u8 = 10;

/// 图标尺寸
pub const ICON_SM: f32 = 14.0;
pub const ICON_MD: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub mode: Mode,
    /// 窗口最底层背景
    pub bg: Color32,
    /// 侧栏/状态栏
    pub panel: Color32,
    /// 卡片、输入框
    pub surface: Color32,
    pub surface_hover: Color32,
    /// 弹窗内的分组卡片
    pub group: Color32,
    /// 输入框底色
    pub input: Color32,
    /// 凹陷的轨道（分段控件底槽）
    pub inset: Color32,
    /// 轨道里凸起的选中块
    pub raised: Color32,
    /// 开关关闭时的底色
    pub toggle_off: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    /// 进度条轨道
    pub track: Color32,
    /// 悬停时叠加的半透明层
    pub hover_tint: Color32,
    /// 文件类型色块
    pub tint_blue: Color32,
    pub tint_cyan: Color32,
    pub tint_purple: Color32,
    pub tint_pink: Color32,
    pub tint_red: Color32,
    pub tint_amber: Color32,
    pub tint_green: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            bg: Color32::from_rgb(0x0b, 0x0d, 0x12),
            panel: Color32::from_rgb(0x10, 0x13, 0x1a),
            surface: Color32::from_rgb(0x16, 0x1a, 0x23),
            surface_hover: Color32::from_rgb(0x1d, 0x22, 0x2e),
            group: Color32::from_rgb(0x1a, 0x1e, 0x28),
            input: Color32::from_rgb(0x0f, 0x12, 0x19),
            inset: Color32::from_rgb(0x0f, 0x12, 0x19),
            raised: Color32::from_rgb(0x2a, 0x30, 0x3f),
            toggle_off: Color32::from_rgb(0x36, 0x3d, 0x4d),
            border: Color32::from_rgb(0x25, 0x2b, 0x38),
            text: Color32::from_rgb(0xee, 0xf0, 0xf5),
            text_muted: Color32::from_rgb(0x8f, 0x98, 0xab),
            accent: Color32::from_rgb(0x6d, 0x8c, 0xff),
            accent_soft: Color32::from_rgba_unmultiplied(0x6d, 0x8c, 0xff, 0x2e),
            success: Color32::from_rgb(0x3d, 0xd6, 0x8c),
            success_soft: Color32::from_rgba_unmultiplied(0x3d, 0xd6, 0x8c, 0x26),
            danger: Color32::from_rgb(0xff, 0x6b, 0x6b),
            danger_soft: Color32::from_rgba_unmultiplied(0xff, 0x6b, 0x6b, 0x26),
            track: Color32::from_rgb(0x23, 0x29, 0x36),
            hover_tint: Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x12),
            tint_blue: Color32::from_rgb(0x6d, 0x8c, 0xff),
            tint_cyan: Color32::from_rgb(0x3f, 0xc4, 0xe0),
            tint_purple: Color32::from_rgb(0xa7, 0x8b, 0xfa),
            tint_pink: Color32::from_rgb(0xf0, 0x7a, 0xb8),
            tint_red: Color32::from_rgb(0xff, 0x6b, 0x6b),
            tint_amber: Color32::from_rgb(0xf5, 0xb1, 0x4c),
            tint_green: Color32::from_rgb(0x3d, 0xd6, 0x8c),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: Mode::Light,
            bg: Color32::from_rgb(0xf5, 0xf6, 0xfa),
            panel: Color32::from_rgb(0xee, 0xf0, 0xf5),
            surface: Color32::WHITE,
            surface_hover: Color32::from_rgb(0xf3, 0xf5, 0xf9),
            group: Color32::from_rgb(0xf7, 0xf8, 0xfb),
            input: Color32::WHITE,
            inset: Color32::from_rgb(0xe9, 0xec, 0xf2),
            raised: Color32::WHITE,
            toggle_off: Color32::from_rgb(0xd3, 0xd8, 0xe2),
            border: Color32::from_rgb(0xe1, 0xe5, 0xee),
            text: Color32::from_rgb(0x16, 0x1a, 0x24),
            text_muted: Color32::from_rgb(0x63, 0x6b, 0x7e),
            accent: Color32::from_rgb(0x4a, 0x6c, 0xf2),
            accent_soft: Color32::from_rgba_unmultiplied(0x4a, 0x6c, 0xf2, 0x22),
            success: Color32::from_rgb(0x1c, 0x9c, 0x68),
            success_soft: Color32::from_rgba_unmultiplied(0x1c, 0x9c, 0x68, 0x1f),
            danger: Color32::from_rgb(0xd6, 0x45, 0x45),
            danger_soft: Color32::from_rgba_unmultiplied(0xd6, 0x45, 0x45, 0x1f),
            track: Color32::from_rgb(0xe6, 0xe9, 0xf0),
            hover_tint: Color32::from_rgba_unmultiplied(0x10, 0x16, 0x28, 0x10),
            tint_blue: Color32::from_rgb(0x4a, 0x6c, 0xf2),
            tint_cyan: Color32::from_rgb(0x12, 0x9e, 0xbd),
            tint_purple: Color32::from_rgb(0x7c, 0x5c, 0xe6),
            tint_pink: Color32::from_rgb(0xd9, 0x4f, 0x9a),
            tint_red: Color32::from_rgb(0xd6, 0x45, 0x45),
            tint_amber: Color32::from_rgb(0xd1, 0x88, 0x14),
            tint_green: Color32::from_rgb(0x1c, 0x9c, 0x68),
        }
    }

    /// 浮窗的速率折线保留强调色，略降饱和度；背景进度头保持更低对比。
    pub fn card_curve(&self, color: Color32) -> Color32 {
        color.lerp_to_gamma(self.text_muted, 0.24)
    }

    pub fn card_progress(&self, color: Color32) -> Color32 {
        color.lerp_to_gamma(self.surface, 0.30)
    }

    pub fn card_head(&self, color: Color32) -> (Color32, Color32) {
        let alpha = match self.mode {
            Mode::Dark => 0.14,
            Mode::Light => 0.10,
        };
        (color.gamma_multiply(alpha * 0.12), color.gamma_multiply(alpha))
    }

    /// 色块背景：取色 + 低透明度
    pub fn soft(&self, c: Color32) -> Color32 {
        let a = match self.mode {
            Mode::Dark => 0x2a,
            Mode::Light => 0x1e,
        };
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    }

    /// 原生窗口（标题栏、系统对话框）使用的主题
    pub fn system_theme(&self) -> egui::SystemTheme {
        match self.mode {
            Mode::Dark => egui::SystemTheme::Dark,
            Mode::Light => egui::SystemTheme::Light,
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
        v.text_edit_bg_color = Some(self.input);
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.window_corner_radius = CornerRadius::same(14);
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_shadow = Shadow { offset: [0, 12], blur: 36, spread: 0, color: Color32::from_black_alpha(110) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(70) };
        v.menu_corner_radius = CornerRadius::same(10);
        v.selection.bg_fill = self.accent_soft;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);

        let r = CornerRadius::same(R_CTRL);
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

        // egui 按系统主题在 dark/light 两套样式间切换，`set_visuals` 只改当前那套：
        // 系统是浅色而应用选深色时，控件会用回 egui 默认的浅色样式。两套都写同一份，并锁定主题。
        ctx.set_theme(match self.mode {
            Mode::Dark => egui::Theme::Dark,
            Mode::Light => egui::Theme::Light,
        });
        ctx.set_visuals_of(egui::Theme::Dark, v.clone());
        ctx.set_visuals_of(egui::Theme::Light, v);
        // 原生标题栏（Windows）跟随应用主题而非系统主题；独立弹窗在 `popup::show` 里各自设置
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::SetTheme(self.system_theme()));
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(14.0, 7.0);
            s.spacing.interact_size.y = 32.0;
            s.spacing.window_margin = egui::Margin::same(18);
            s.spacing.menu_margin = egui::Margin::same(8);
            s.spacing.icon_width = 16.0;
            // 滚动条平时隐藏，悬停或滚动时才浮出细把手
            s.spacing.scroll.floating_width = 4.0;
            s.spacing.scroll.dormant_handle_opacity = 0.0;
            s.spacing.scroll.dormant_background_opacity = 0.0;
            s.interaction.selectable_labels = false;
            s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
            s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
        });
    }
}
