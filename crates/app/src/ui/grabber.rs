use crate::app::SwoopApp;
use crate::grabber_tasks::ProjectAction;
use egui::{RichText, TextEdit};

impl SwoopApp {
    pub fn grabber_ui(&mut self, ctx: &egui::Context) {
        if !self.grabber.open {
            return;
        }
        let p = self.palette;
        let busy = self.grabber.busy();
        let mut open = true;
        let mut action = None;
        egui::Window::new(swoop_i18n::tr!("站点抓取"))
            .open(&mut open)
            .default_width(700.0)
            .max_height(ctx.content_rect().height() - 40.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let config = &mut self.grabber.config;
                    egui::Grid::new("grabber_config").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                        ui.label(swoop_i18n::tr!("起始网址"));
                        ui.add(
                            TextEdit::singleline(&mut config.start_url)
                                .desired_width(520.0)
                                .hint_text("https://example.com/"),
                        );
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("项目目录"));
                        let mut dir = config.output_dir.display().to_string();
                        if ui.add(TextEdit::singleline(&mut dir).desired_width(520.0)).changed() {
                            config.output_dir = super::add_dialog::expand_tilde(&dir);
                        }
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("抓取深度"));
                        ui.add(egui::DragValue::new(&mut config.max_depth).range(0..=100));
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("页面与资源上限"));
                        ui.add(egui::DragValue::new(&mut config.max_pages).range(1..=100_000));
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("同时抓取"));
                        ui.add(egui::DragValue::new(&mut config.concurrency).range(1..=16));
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("流量上限"));
                        let mut mib = config.max_total_bytes / (1024 * 1024);
                        if ui.add(egui::DragValue::new(&mut mib).range(1..=1024 * 1024).suffix(" MiB")).changed() {
                            config.max_total_bytes = mib * 1024 * 1024;
                        }
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("单文件上限"));
                        let mut mib = config.max_file_bytes / (1024 * 1024);
                        if ui.add(egui::DragValue::new(&mut mib).range(1..=256).suffix(" MiB")).changed() {
                            config.max_file_bytes = mib * 1024 * 1024;
                        }
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("请求超时"));
                        ui.add(egui::DragValue::new(&mut config.request_timeout_secs).range(1..=120).suffix(swoop_i18n::tr!(" 秒")));
                        ui.end_row();
                        for (label, hint, text) in [
                            (swoop_i18n::tr!("额外域名"), swoop_i18n::tr!("默认仅当前站点"), &mut self.grabber.domains_text),
                            (swoop_i18n::tr!("资源扩展名"), swoop_i18n::tr!("留空下载全部直接资源"), &mut self.grabber.extensions_text),
                        ] {
                            ui.label(label);
                            ui.add(TextEdit::singleline(text).desired_width(520.0).hint_text(hint));
                            ui.end_row();
                        }
                        ui.label(swoop_i18n::tr!("重定向上限"));
                        ui.add(egui::DragValue::new(&mut config.max_redirects).range(0..=20));
                        ui.end_row();
                        ui.label(swoop_i18n::tr!("站点规则"));
                        ui.checkbox(&mut config.respect_robots, swoop_i18n::tr!("遵循 robots.txt"));
                        ui.end_row();
                    });
                    ui.collapsing(swoop_i18n::tr!("资源与排除规则"), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut config.capture.scripts, swoop_i18n::tr!("脚本文件"));
                            ui.checkbox(&mut config.capture.media, swoop_i18n::tr!("音视频与字幕"));
                            ui.checkbox(&mut config.capture.embedded_documents, swoop_i18n::tr!("嵌入页面与文档"));
                            ui.checkbox(&mut config.capture.manifests, swoop_i18n::tr!("网页清单"));
                            ui.checkbox(&mut config.capture.lazy_images, swoop_i18n::tr!("懒加载图片"));
                        });
                        ui.checkbox(&mut config.capture.strip_scripts_from_mirror, swoop_i18n::tr!("从镜像移除 script 元素"))
                            .on_hover_text(swoop_i18n::tr!("仅移除 script 标签，不是 HTML 安全过滤器；事件属性等仍会保留。"));
                        ui.add(TextEdit::multiline(&mut self.grabber.exclusions_text).desired_rows(2).desired_width(640.0)
                            .hint_text(swoop_i18n::tr!("排除 URL，每行一条，例如 /logout* 或 */tracking/*")))
                            .on_hover_text(swoop_i18n::tr!("* 匹配任意字符，? 匹配一个字节。以 / 开头匹配路径与查询，其他规则匹配完整 URL。"));
                    });
                    ui.collapsing(swoop_i18n::tr!("动态页面"), |ui| {
                        ui.checkbox(&mut config.render.enabled, swoop_i18n::tr!("使用隔离浏览器渲染"))
                            .on_hover_text(swoop_i18n::tr!("运行所选 Chromium 浏览器，使用临时用户目录，不读取已登录浏览器的数据。浏览器流量单独计数。HTTPS 仅限制目标域与端口，无法检查加密请求路径的 robots 或排除规则。"));
                        if config.render.enabled {
                            egui::Grid::new("grabber_render_config").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                                ui.label("Chrome / Edge / Chromium");
                                let mut executable = config.render.executable.display().to_string();
                                if ui.add(TextEdit::singleline(&mut executable).desired_width(430.0).hint_text(swoop_i18n::tr!("浏览器可执行文件的完整路径"))).changed() {
                                    config.render.executable = super::add_dialog::expand_tilde(&executable);
                                }
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("渲染范围"));
                                ui.checkbox(&mut config.render.every_html_page, swoop_i18n::tr!("每个 HTML 页面（关闭时仅首页）"));
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("加载等待"));
                                ui.add(egui::DragValue::new(&mut config.render.virtual_time_ms).range(100..=60_000).suffix(" ms"));
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("总超时"));
                                ui.add(egui::DragValue::new(&mut config.render.timeout_secs).range(1..=120).suffix(swoop_i18n::tr!(" 秒")));
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("DOM 上限"));
                                let mut mib = config.render.max_dom_bytes / (1024 * 1024);
                                if ui.add(egui::DragValue::new(&mut mib).range(1..=64).suffix(" MiB")).changed() { config.render.max_dom_bytes = mib * 1024 * 1024; }
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("浏览器流量上限"));
                                let mut mib = config.render.max_network_bytes / (1024 * 1024);
                                if ui.add(egui::DragValue::new(&mut mib).range(1..=1024 * 1024).suffix(" MiB")).changed() { config.render.max_network_bytes = mib * 1024 * 1024; }
                                ui.end_row();
                            });
                        }
                        if let Some(snapshot) = &self.grabber.snapshot {
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("grabber_snapshot_url").selected_text(&self.grabber.snapshot_url).width(430.0).show_ui(ui, |ui| {
                                    for page in &snapshot.pages {
                                        ui.selectable_value(&mut self.grabber.snapshot_url, page.url.clone(), &page.url);
                                    }
                                });
                                if ui.button(swoop_i18n::tr!("导入已渲染 HTML")).on_hover_text(swoop_i18n::tr!("选择保存后的 DOM 快照，然后继续抓取其中的资源；仅接受当前项目已有页面。")).clicked() {
                                    action = Some(ProjectAction::ImportSnapshot);
                                }
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        if super::primary_button(ui, &p, swoop_i18n::tr!("新建并开始")).clicked() {
                            action = Some(ProjectAction::Create);
                        }
                        if ui.button(swoop_i18n::tr!("打开已有项目")).clicked() {
                            action = Some(ProjectAction::Open);
                        }
                        if ui.add_enabled(self.grabber.snapshot.is_some(), egui::Button::new(swoop_i18n::tr!("继续抓取"))).clicked()
                        {
                            action = Some(ProjectAction::Run);
                        }
                        ui.menu_button(swoop_i18n::tr!("最近项目"), |ui| {
                            for path in &self.grabber.recent {
                                if ui.button(path.display().to_string()).clicked() {
                                    self.grabber.config.output_dir = path.clone();
                                    action = Some(ProjectAction::Open);
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                if busy && ui.button(swoop_i18n::tr!("停止抓取")).clicked() {
                    self.grabber.stop();
                }
                if let Some(error) = &self.grabber.error {
                    ui.colored_label(p.danger, error);
                }
                if let Some(snapshot) = &self.grabber.snapshot {
                    use swoop_grabber::ProjectStatus;
                    let status = match snapshot.status {
                        ProjectStatus::Ready => swoop_i18n::tr!("就绪"),
                        ProjectStatus::Running => swoop_i18n::tr!("抓取中"),
                        ProjectStatus::Complete => swoop_i18n::tr!("本轮结束"),
                        ProjectStatus::Cancelled => swoop_i18n::tr!("已停止"),
                        ProjectStatus::Limited => swoop_i18n::tr!("已达限制"),
                    };
                    ui.separator();
                    ui.label(swoop_i18n::trf!(
                        "{status} · {} 已保存 · {} 失败 · {}",
                        snapshot.downloaded(),
                        snapshot.failed(),
                        super::format::bytes(snapshot.received_bytes)
                    ));
                    if snapshot.rendered_network_bytes > 0 {
                        ui.label(swoop_i18n::trf!("浏览器流量：{}", super::format::bytes(snapshot.rendered_network_bytes)));
                    }
                    if let Some(entry) = &snapshot.entry_path {
                        if ui.button(swoop_i18n::tr!("显示离线首页")).clicked() {
                            super::task_list::reveal_in_file_manager(entry);
                        }
                        ui.add(
                            egui::Label::new(RichText::new(entry.display().to_string()).size(12.0).color(p.text_muted))
                                .truncate(),
                        );
                    }
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for page in snapshot.pages.iter().rev().take(100) {
                            let state = match &page.state {
                                swoop_grabber::PageState::Pending => swoop_i18n::tr!("等待"),
                                swoop_grabber::PageState::Downloaded => swoop_i18n::tr!("已保存"),
                                swoop_grabber::PageState::Failed(_) => swoop_i18n::tr!("失败"),
                                swoop_grabber::PageState::Skipped(_) => swoop_i18n::tr!("跳过"),
                            };
                            let response = ui.add(egui::Label::new(format!("{state}  {}", page.url)).truncate());
                            if let swoop_grabber::PageState::Failed(error) | swoop_grabber::PageState::Skipped(error) =
                                &page.state
                            {
                                response.on_hover_text(error);
                            }
                        }
                    });
                }
            });
        self.grabber.open = open;
        if let Some(action) = action {
            self.grabber.start(action, ctx);
        }
    }
}
