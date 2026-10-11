use crate::app::SwoopApp;
use crate::categories::CategoryRule;
use egui::{Frame, Margin, RichText};

impl SwoopApp {
    pub fn categories_ui(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.category_editor else {
            return;
        };
        let p = self.palette;
        let mut save = false;
        let mut close = false;
        let mut remove = None;
        let mut up = None;
        let modal = egui::Modal::new(egui::Id::new("categories"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.label(RichText::new(swoop_i18n::tr!("分类与保存规则")).size(18.0).strong());
                ui.checkbox(&mut draft.enabled, swoop_i18n::tr!("新建下载时自动选择分类目录"));
                ui.label(
                    RichText::new(swoop_i18n::tr!("按列表顺序匹配；相对目录位于默认保存目录内"))
                        .size(12.0)
                        .color(p.text_muted),
                );
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 240.0).max(160.0)).show(
                    ui,
                    |ui| {
                        for (index, rule) in draft.rules.iter_mut().enumerate() {
                            ui.push_id(index, |ui| {
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rule.name)
                                            .desired_width(200.0)
                                            .hint_text(swoop_i18n::tr!("分类名称")),
                                    );
                                    if super::icon_button(
                                        ui,
                                        &p,
                                        egui_phosphor::regular::ARROW_UP,
                                        swoop_i18n::tr!("提高优先级"),
                                    )
                                    .clicked()
                                        && index > 0
                                    {
                                        up = Some(index);
                                    }
                                    if super::icon_button(
                                        ui,
                                        &p,
                                        egui_phosphor::regular::TRASH,
                                        swoop_i18n::tr!("移除规则"),
                                    )
                                    .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                                egui::Grid::new("rule").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                    ui.label(swoop_i18n::tr!("扩展名"));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rule.extensions)
                                            .desired_width(400.0)
                                            .hint_text("zip rar 7z"),
                                    );
                                    ui.end_row();
                                    ui.label(swoop_i18n::tr!("域名"));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rule.domains)
                                            .desired_width(400.0)
                                            .hint_text(swoop_i18n::tr!("可选：*.example.com")),
                                    );
                                    ui.end_row();
                                    ui.label(swoop_i18n::tr!("保存到"));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rule.directory)
                                            .desired_width(400.0)
                                            .hint_text(swoop_i18n::tr!("分类子目录或绝对目录")),
                                    );
                                    ui.end_row();
                                });
                            });
                        }
                        if ui.button(swoop_i18n::tr!("添加分类")).clicked() && draft.rules.len() < 100 {
                            draft.rules.push(CategoryRule {
                                name: swoop_i18n::trf!("分类 {}", draft.rules.len() + 1),
                                extensions: String::new(),
                                domains: String::new(),
                                directory: "Downloads".into(),
                            });
                        }
                    },
                );
                if let Some(error) = &self.category_error {
                    ui.colored_label(p.danger, error);
                }
                ui.horizontal(|ui| {
                    save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                    close = super::ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked();
                });
            });
        if let Some(index) = remove {
            draft.rules.remove(index);
        }
        if let Some(index) = up.filter(|index| *index < draft.rules.len()) {
            draft.rules.swap(index, index - 1);
        }
        if save {
            match draft.save(&self.store) {
                Ok(()) => {
                    self.categories = draft.clone();
                    self.bridge.set_categories(self.categories.clone());
                    if self
                        .category_filter
                        .as_ref()
                        .is_some_and(|name| !self.categories.rules.iter().any(|r| &r.name == name))
                    {
                        self.category_filter = None;
                    }
                    self.category_error = None;
                    self.category_editor = None;
                    self.toasts.push_info(swoop_i18n::tr!("分类已保存"), swoop_i18n::tr!("新建下载使用更新后的规则"));
                }
                Err(error) => self.category_error = Some(error),
            }
        } else if close || modal.should_close() {
            self.category_editor = None;
            self.category_error = None;
        }
    }
}
