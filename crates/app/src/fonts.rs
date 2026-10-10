//! 字体：egui 默认字体 + Phosphor 图标 + 内嵌的中文子集字体。
//!
//! 中文字体为阿里巴巴普惠体 3.0 Regular 的子集（GB2312 全部汉字 + ASCII + 中英标点 + 常用符号），
//! 由 `scripts/make_font_subset.py` 生成，见 `assets/fonts/NOTICE.md`。
//! 不在子集内的生僻字会显示为方框，这是为了把内存占用从整字体的几十 MB 压到几 MB 的取舍。

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

const CJK_FONT: &[u8] = include_bytes!("../../../assets/fonts/AlibabaPuHuiTi-3-55-Regular.subset.otf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_static(CJK_FONT)));
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().push("cjk".into());
    }
    tracing::info!(bytes = CJK_FONT.len(), "已加载内嵌中文字体");

    ctx.set_fonts(fonts);
}
