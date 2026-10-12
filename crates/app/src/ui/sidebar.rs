use egui::{Frame, Margin, Sense, Stroke, UiBuilder, vec2};

use super::filekind::FileKind;
use super::{Filter, nav_item};
use crate::app::SwoopApp;

/// 底部固定区（分隔 + 设置行）的高度。媒体下载、站点抓取等进阶入口在顶栏「更多操作」里，不占侧栏。
const FOOTER_H: f32 = 34.0 + 2.0 + 17.0;
/// 侧栏顶部留白：macOS 透明标题栏要给红绿灯让位；其他平台有系统标题栏，与顶栏同高，Logo 行和顶栏标题行对齐
const TOP_INSET: i8 = if cfg!(target_os = "macos") { 44 } else { 18 };
/// Logo 行高，与顶栏按钮同高
const BRAND_H: f32 = 34.0;

/// 左上角品牌行：图标与字标在行内垂直居中，字标用只含 "Swoop" 的普惠体 Bold
fn brand_row(ui: &mut egui::Ui, color: egui::Color32) {
    const MARK: f32 = 32.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BRAND_H), Sense::hover());
    let mark =
        egui::Rect::from_center_size(egui::pos2(rect.left() + 4.0 + MARK / 2.0, rect.center().y), vec2(MARK, MARK));
    ui.scope_builder(UiBuilder::new().max_rect(mark), |ui| crate::brand::show_mark(ui, MARK));
    let galley = ui.painter().layout_no_wrap("Swoop".into(), crate::fonts::brand(22.0), color);
    let pos = egui::pos2(mark.right() + 10.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

impl SwoopApp {
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let ctx = ui.ctx().clone();
        egui::Panel::left("sidebar")
            .exact_size(208.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin { left: 12, right: 12, top: TOP_INSET, bottom: 12 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                brand_row(ui, p.text);
                ui.add_space(10.0);

                egui::ScrollArea::vertical()
                    .id_salt("sidebar_sections")
                    .max_height((ui.available_height() - FOOTER_H).max(60.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.section_header(ui, swoop_i18n::tr!("任务"), None);
                        let mut clicked = None;
                        for f in Filter::ALL {
                            let count = self.tasks.iter().filter(|t| f.matches(&t.status)).count();
                            let selected =
                                self.queue_filter.is_none() && self.category_filter.is_none() && self.filter == f;
                            let badge = (count > 0).then(|| count.to_string());
                            if nav_item(ui, &p, f.icon(), None, f.label(), badge.as_deref(), selected).clicked() {
                                clicked = Some(f);
                            }
                        }
                        if let Some(f) = clicked {
                            self.filter = f;
                            self.queue_filter = None;
                            self.category_filter = None;
                        }

                        ui.add_space(14.0);
                        if self.section_header(
                            ui,
                            swoop_i18n::tr!("队列"),
                            Some((egui_phosphor::regular::GEAR_SIX, swoop_i18n::tr!("管理队列与计划"))),
                        ) {
                            self.queue_editor.show(self.queue_filter.unwrap_or(swoop_core::MAIN_QUEUE_ID));
                        }
                        let mut picked = None;
                        let mut edit = None;
                        for queue in &self.queues {
                            let icon = if queue.running {
                                egui_phosphor::regular::PLAY_CIRCLE
                            } else {
                                egui_phosphor::regular::PAUSE_CIRCLE
                            };
                            let tint = queue.running.then_some(p.success);
                            let count = queue.active_count + queue.waiting_count;
                            let badge = (count > 0).then(|| count.to_string());
                            let response = nav_item(
                                ui,
                                &p,
                                icon,
                                tint,
                                super::queue_name(queue),
                                badge.as_deref(),
                                self.queue_filter == Some(queue.id),
                            )
                            .on_hover_text(swoop_i18n::tr!("双击编辑队列"));
                            if response.clicked() {
                                picked = Some(queue.id);
                            }
                            if response.double_clicked() {
                                edit = Some(queue.id);
                            }
                        }
                        if let Some(id) = picked {
                            self.queue_filter = Some(id);
                            self.filter = Filter::All;
                            self.category_filter = None;
                        }
                        if let Some(id) = edit {
                            self.queue_editor.show(id);
                        }

                        ui.add_space(14.0);
                        if self.section_header(
                            ui,
                            swoop_i18n::tr!("分类"),
                            Some((egui_phosphor::regular::GEAR_SIX, swoop_i18n::tr!("分类与保存规则"))),
                        ) {
                            self.category_editor = Some(self.categories.clone());
                            self.category_error = None;
                        }
                        let mut picked = None;
                        for rule in &self.categories.rules {
                            let first = rule.extensions.split_whitespace().next().unwrap_or_default();
                            let kind = FileKind::from_name(&format!("x.{first}"));
                            let count = self
                                .tasks
                                .iter()
                                .filter(|t| {
                                    self.categories
                                        .classify(&t.url, Some(&t.file_name))
                                        .is_some_and(|matched| matched.name == rule.name)
                                })
                                .count();
                            let badge = (count > 0).then(|| count.to_string());
                            if nav_item(
                                ui,
                                &p,
                                kind.outline_icon(),
                                Some(kind.tint(&p)),
                                &rule.name,
                                badge.as_deref(),
                                self.category_filter.as_ref() == Some(&rule.name),
                            )
                            .clicked()
                            {
                                picked = Some(rule.name.clone());
                            }
                        }
                        if let Some(name) = picked {
                            self.category_filter = Some(name);
                            self.queue_filter = None;
                            self.filter = Filter::All;
                        }
                        ui.add_space(6.0);
                    });

                // 底部固定区：设置 + 主题，贴着侧栏底边
                let bottom = ui.max_rect().bottom();
                let rect = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().left(), bottom - FOOTER_H),
                    egui::pos2(ui.max_rect().right(), bottom),
                );
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    use egui_phosphor::regular as i;
                    ui.add_space(7.0);
                    let y = ui.cursor().top();
                    ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let width = ui.available_width() - 34.0;
                        let settings = ui
                            .allocate_ui(vec2(width, 34.0), |ui| {
                                nav_item(ui, &p, i::GEAR, None, swoop_i18n::tr!("设置"), None, self.show_settings)
                            })
                            .inner;
                        if settings.clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        let icon = match p.mode {
                            crate::theme::Mode::Dark => i::SUN,
                            crate::theme::Mode::Light => i::MOON,
                        };
                        let theme = ui
                            .allocate_ui(vec2(30.0, 34.0), |ui| {
                                ui.add_space(3.0);
                                super::icon_button(ui, &p, icon, swoop_i18n::tr!("切换主题"))
                            })
                            .inner;
                        if theme.clicked() {
                            self.toggle_theme(&ctx);
                        }
                    });
                });
            });
    }

    /// 分组小标题，右侧可带一个图标按钮；返回按钮是否被点击
    fn section_header(&self, ui: &mut egui::Ui, title: &str, action: Option<(&str, &str)>) -> bool {
        let p = self.palette;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(11.5),
            p.text_muted,
        );
        let Some((icon, tooltip)) = action else {
            return false;
        };
        let button = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), vec2(28.0, 26.0));
        ui.put(button, |ui: &mut egui::Ui| super::icon_button(ui, &p, icon, tooltip)).clicked()
    }
}
