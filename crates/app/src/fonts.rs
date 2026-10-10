//! 字体：内嵌普惠体子集为主字体（含 Latin）+ egui 默认字体兜底 + Phosphor 图标（Regular / Fill 两套）。
//!
//! 中文字体为阿里巴巴普惠体 3.0 Regular 的子集（GB2312 全部汉字 + ASCII + 中英标点 + 常用符号），
//! 由 `scripts/make_font_subset.py` 生成，见 `assets/fonts/NOTICE.md`。
//! 不在子集内的生僻字会显示为方框，这是为了把内存占用从整字体的几十 MB 压到几 MB 的取舍。

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId};

const CJK_FONT: &[u8] = include_bytes!("../../../assets/fonts/AlibabaPuHuiTi-3-55-Regular.subset.otf");

/// Phosphor Fill 变体的字体族名；Regular 变体直接混在 Proportional 里。
pub const ICON_FILL_FAMILY: &str = "phosphor-fill";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // 普惠体放在最前面：中英文同一套字体，避免 Latin 用 Ubuntu、中文用普惠体的混搭感
    fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_static(CJK_FONT)));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "cjk".into());
    fonts.families.entry(FontFamily::Monospace).or_default().push("cjk".into());

    // 图标：Regular 混入 Proportional（可直接写在普通文本里），Fill 单独成族
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    egui_phosphor::add_font_bytes_as_family(&mut fonts, ICON_FILL_FAMILY, egui_phosphor::Variant::Fill.font_bytes());

    tracing::info!(bytes = CJK_FONT.len(), "已加载内嵌中文字体");
    ctx.set_fonts(fonts);
}

pub fn icon_fill(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICON_FILL_FAMILY.into()))
}
