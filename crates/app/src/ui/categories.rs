use super::dialog;
use super::filekind::FileKind;
use crate::app::SwoopApp;
use crate::categories::CategoryRule;

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
        let modal = dialog::modal("categories", &p).show(ctx, |ui| {
            ui.set_width(600.0);
            close |= dialog::header(
                ui,
                &p,
                egui_phosphor::regular::FOLDERS,
                swoop_i18n::tr!("分类与保存规则"),
                Some(swoop_i18n::tr!("按列表顺序匹配；相对目录位于默认保存目录内")),
            );
            dialog::body(ui, "categories_scroll", |ui| {
                dialog::group(ui, &p, None, |ui| {
                    dialog::row(ui, &p, swoop_i18n::tr!("新建下载时自动选择分类目录"), None, |ui| {
                        super::toggle(ui, &p, &mut draft.enabled);
                    });
                });
                for (index, rule) in draft.rules.iter_mut().enumerate() {
                    ui.push_id(index, |ui| {
                        dialog::group(ui, &p, None, |ui| {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                let first = rule.extensions.split_whitespace().next().unwrap_or_default();
                                let kind = FileKind::from_name(&format!("x.{first}"));
                                let (tile, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                                ui.painter().rect_filled(tile, egui::CornerRadius::same(8), p.soft(kind.tint(&p)));
                                ui.painter().text(
                                    tile.center(),
                                    egui::Align2::CENTER_CENTER,
                                    kind.outline_icon(),
                                    egui::FontId::proportional(16.0),
                                    kind.tint(&p),
                                );
                                ui.add_space(2.0);
                                ui.add(
                                    super::field(&mut rule.name)
                                        .desired_width(220.0)
                                        .font(egui::FontId::proportional(15.0))
                                        .hint_text(swoop_i18n::tr!("分类名称")),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                                    if index > 0
                                        && super::icon_button(
                                            ui,
                                            &p,
                                            egui_phosphor::regular::ARROW_UP,
                                            swoop_i18n::tr!("提高优先级"),
                                        )
                                        .clicked()
                                    {
                                        up = Some(index);
                                    }
                                });
                            });
                            ui.add_space(10.0);
                            for (label, value, hint) in [
                                (swoop_i18n::tr!("扩展名"), &mut rule.extensions, "zip rar 7z"),
                                (swoop_i18n::tr!("域名"), &mut rule.domains, swoop_i18n::tr!("可选：*.example.com")),
                                (
                                    swoop_i18n::tr!("保存到"),
                                    &mut rule.directory,
                                    swoop_i18n::tr!("分类子目录或绝对目录"),
                                ),
                            ] {
                                dialog::field_row(ui, &p, label, 60.0, |ui| {
                                    ui.add(super::field(value).desired_width(ui.available_width()).hint_text(hint));
                                });
                            }
                        });
                    });
                }
                if super::ghost_button(
                    ui,
                    &p,
                    &format!("{}  {}", egui_phosphor::regular::PLUS, swoop_i18n::tr!("添加分类")),
                )
                .clicked()
                    && draft.rules.len() < 100
                {
                    draft.rules.push(CategoryRule {
                        name: swoop_i18n::trf!("分类 {}", draft.rules.len() + 1),
                        extensions: String::new(),
                        domains: String::new(),
                        directory: "Downloads".into(),
                    });
                }
                ui.add_space(4.0);
                if let Some(error) = &self.category_error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                close |= super::ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked();
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
