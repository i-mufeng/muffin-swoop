//! 右侧详情面板：选中任务的速度曲线、基本信息和各连接区段。
//! 点选列表行打开，× / Esc / 再点一次同一行收起，带滑入滑出动画。

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Label, Layout, Margin, Mesh, Pos2, Rect, RichText, Sense,
    Shape, Stroke, Ui, pos2, vec2,
};
use swoop_core::segment::SegmentState;
use swoop_core::{SPEED_HISTORY_LEN, SegmentSnapshot, TaskId, TaskSnapshot, TaskStatus};

use super::filekind::FileKind;
use super::progress::status_color;
use super::{anim_secs, file_tile, format, icon_button, pill};
use crate::app::SwoopApp;
use crate::theme::{Palette, R_CARD};

/// 面板宽度（含内边距）
const DETAIL_W: f32 = 300.0;
const CHART_H: f32 = 96.0;
const LABEL_W: f32 = 64.0;

enum DetailAction {
    Edit(TaskId),
    Close,
    CopyLink(String),
    Reveal(TaskId),
}

impl SwoopApp {
    pub fn detail_panel(&mut self, ui: &mut egui::Ui) {
        // 选中的任务被移除了就收起
        if let Some(id) = self.selected
            && !self.tasks.iter().any(|t| t.id == id)
        {
            self.selected = None;
        }
        if self.selected.is_some() {
            self.detail_last = self.selected;
        }
        // 收起动画期间继续画最后一个任务
        let Some(t) = self.detail_last.and_then(|id| self.tasks.iter().find(|t| t.id == id)).cloned() else {
            return;
        };
        let history = crate::demo::speed_history(&t).unwrap_or_else(|| self.engine.speed_history(t.id));

        let p = self.palette;
        let mut open = self.selected.is_some();
        let mut action = None;
        egui::Panel::right("detail")
            .exact_size(DETAIL_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin { left: 16, right: 16, top: 16, bottom: 0 }))
            .show_collapsible(ui, &mut open, |ui| {
                // 左边缘细线：画在内边距之外，需要放开裁剪
                let r = ui.max_rect();
                let edge = Rect::from_min_max(pos2(r.left() - 16.0, r.top() - 16.0), pos2(r.left() - 15.0, r.bottom()));
                ui.painter().with_clip_rect(edge).vline(edge.left(), edge.y_range(), Stroke::new(1.0, p.border));
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    action = detail_contents(ui, &p, &t, &history);
                    ui.add_space(16.0);
                });
            });

        match action {
            Some(DetailAction::Edit(id)) => self.edit_task(id),
            Some(DetailAction::Close) => self.selected = None,
            Some(DetailAction::CopyLink(url)) => self.copy_link(ui.ctx(), url),
            Some(DetailAction::Reveal(id)) => {
                if let Some(t) = self.tasks.iter().find(|t| t.id == id) {
                    super::task_list::reveal_in_file_manager(&t.path);
                }
            }
            None => {}
        }
    }
}

fn detail_contents(ui: &mut Ui, p: &Palette, t: &TaskSnapshot, history: &[f64]) -> Option<DetailAction> {
    use egui_phosphor::regular as i;
    let mut action = None;
    ui.spacing_mut().item_spacing.y = 4.0;

    // 头部：类型色块 + 操作按钮，下面是可换行的文件名和状态
    ui.horizontal(|ui| {
        file_tile(ui, p, FileKind::from_name(&t.file_name), 40.0);
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(ui, p, i::X, swoop_i18n::tr!("关闭（Esc）")).clicked() {
                action = Some(DetailAction::Close);
            }
            if !t.file_name.is_empty() && icon_button(ui, p, i::FOLDER_OPEN, swoop_i18n::tr!("在访达中显示")).clicked()
            {
                action = Some(DetailAction::Reveal(t.id));
            }
            if icon_button(ui, p, i::LINK, swoop_i18n::tr!("复制链接")).clicked() {
                action = Some(DetailAction::CopyLink(t.url.clone()));
            }
            if icon_button(ui, p, i::PENCIL_SIMPLE, swoop_i18n::tr!("下载属性")).clicked() {
                action = Some(DetailAction::Edit(t.id));
            }
        });
    });
    ui.add_space(10.0);
    let name = if t.file_name.is_empty() { swoop_i18n::tr!("正在获取文件信息…") } else { &t.file_name };
    wrapped_text(ui, name, FontId::proportional(15.0), p.text, 3);
    ui.add_space(6.0);
    status_pill(ui, p, &t.status);
    ui.add_space(14.0);

    speed_card(ui, p, t, history);
    ui.add_space(16.0);

    // 基本信息
    let muted = |s: &str| RichText::new(s).color(p.text_muted);
    let text = |s: String| RichText::new(s).color(p.text);
    info_row(
        ui,
        p,
        swoop_i18n::tr!("大小"),
        t.total.map(format::bytes).map_or_else(|| muted(swoop_i18n::tr!("未知")), text),
        None,
    );
    let done = match t.progress() {
        Some(pr) => format!("{}（{:.1}%）", format::bytes(t.downloaded), pr * 100.0),
        None => format::bytes(t.downloaded),
    };
    info_row(ui, p, swoop_i18n::tr!("已下载"), text(done), None);
    if t.status.is_active() {
        let eta = t.eta_secs().map(format::duration);
        info_row(ui, p, swoop_i18n::tr!("剩余时间"), eta.map_or_else(|| muted("—"), text), None);
    }
    if t.status == TaskStatus::Downloading {
        info_row(ui, p, swoop_i18n::tr!("连接"), text(swoop_i18n::trf!("{} 个活动", t.active_connections)), None);
    }
    // 探测前不知道是否支持 Range
    let resumable = match (t.segments.is_empty(), t.resumable) {
        (true, _) => muted("—"),
        (false, true) => RichText::new(swoop_i18n::tr!("支持")).color(p.success),
        (false, false) => muted(swoop_i18n::tr!("不支持（单连接）")),
    };
    info_row(ui, p, swoop_i18n::tr!("断点续传"), resumable, None);
    let dir = t.path.parent().map(format::tilde).unwrap_or_default();
    if info_row(ui, p, swoop_i18n::tr!("保存到"), text(dir), Some(swoop_i18n::tr!("在访达中显示"))).clicked() {
        action = Some(DetailAction::Reveal(t.id));
    }
    if info_row(
        ui,
        p,
        swoop_i18n::tr!("链接"),
        RichText::new(&t.url).color(p.accent),
        Some(swoop_i18n::tr!("点击复制")),
    )
    .clicked()
    {
        action = Some(DetailAction::CopyLink(t.url.clone()));
    }
    info_row(ui, p, swoop_i18n::tr!("创建于"), text(format::ago(t.created_at)), None);
    if let Some(at) = t.completed_at {
        info_row(ui, p, swoop_i18n::tr!("完成于"), text(format::ago(at)), None);
    }
    if let TaskStatus::Failed(reason) = &t.status {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let (rect, _) = ui.allocate_exact_size(vec2(info_label_width(ui), 20.0), Sense::hover());
            ui.painter_at(rect).text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                swoop_i18n::tr!("失败原因"),
                FontId::proportional(12.5),
                p.text_muted,
            );
            ui.add(Label::new(RichText::new(reason).size(12.5).color(p.danger)).wrap());
        });
    }

    // 区段：一行一段，动态分段后会比连接数多
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(swoop_i18n::tr!("区段")).size(12.5).strong().color(p.text));
        if !t.segments.is_empty() {
            pill(ui, &t.segments.len().to_string(), p.soft(p.text_muted), p.text_muted);
        }
    });
    ui.add_space(6.0);
    if t.segments.is_empty() {
        ui.label(RichText::new(swoop_i18n::tr!("探测完成后显示")).size(12.0).color(p.text_muted));
    } else {
        let color = status_color(p, &t.status);
        let active = t.status == TaskStatus::Downloading;
        for (idx, seg) in t.segments.iter().enumerate() {
            segment_row(ui, p, idx, seg, color, active);
        }
    }
    action
}

/// 带行数上限的换行文本（超出用省略号）
fn wrapped_text(ui: &mut Ui, text: &str, font: FontId, color: Color32, max_rows: usize) {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: ui.available_width(),
        max_rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
    let elided = galley.elided;
    ui.painter().galley(rect.min, galley, color);
    if elided {
        resp.on_hover_text(text);
    }
}

fn status_pill(ui: &mut Ui, p: &Palette, status: &TaskStatus) {
    let (label, bg, fg) = match status {
        TaskStatus::Queued => (swoop_i18n::tr!("排队中"), p.soft(p.text_muted), p.text_muted),
        TaskStatus::Probing => (swoop_i18n::tr!("连接中"), p.accent_soft, p.accent),
        TaskStatus::Downloading => (swoop_i18n::tr!("下载中"), p.accent_soft, p.accent),
        TaskStatus::FileConflict => (swoop_i18n::tr!("等待确认"), p.soft(p.tint_amber), p.tint_amber),
        TaskStatus::Paused => (swoop_i18n::tr!("已暂停"), p.soft(p.text_muted), p.text_muted),
        TaskStatus::Completed => (swoop_i18n::tr!("已完成"), p.success_soft, p.success),
        TaskStatus::Failed(_) => (swoop_i18n::tr!("失败"), p.danger_soft, p.danger),
    };
    pill(ui, label, bg, fg);
}

/// 标签 + 单行截断的值；`click_hint` 不为空时整行可点
fn info_row(ui: &mut Ui, p: &Palette, label: &str, value: RichText, click_hint: Option<&str>) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (rect, _) = ui.allocate_exact_size(vec2(info_label_width(ui), 20.0), Sense::hover());
        ui.painter_at(rect).text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            p.text_muted,
        );
        let mut l = Label::new(value.size(12.5)).truncate();
        if click_hint.is_some() {
            l = l.sense(Sense::click());
        }
        let resp = ui.add(l);
        match click_hint {
            Some(hint) => resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hint),
            None => resp,
        }
    })
    .inner
}

fn info_label_width(ui: &Ui) -> f32 {
    [
        swoop_i18n::tr!("断点续传"),
        swoop_i18n::tr!("剩余时间"),
        swoop_i18n::tr!("已下载"),
        swoop_i18n::tr!("失败原因"),
        swoop_i18n::tr!("连接"),
        swoop_i18n::tr!("完成于"),
    ]
    .into_iter()
    .map(|label| {
        ui.painter().layout_no_wrap(label.into(), FontId::proportional(12.5), ui.visuals().text_color()).size().x + 10.0
    })
    .fold(LABEL_W, f32::max)
    .min(ui.available_width() * 0.48)
}

// ───────────────────────── 速度曲线 ─────────────────────────

fn speed_card(ui: &mut Ui, p: &Palette, t: &TaskSnapshot, history: &[f64]) {
    let color = status_color(p, &t.status);
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(swoop_i18n::tr!("当前速度")).size(11.5).color(p.text_muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(swoop_i18n::trf!("最近 {} 分钟", SPEED_HISTORY_LEN / 60))
                            .size(11.5)
                            .color(p.text_muted),
                    );
                });
            });
            let current = if t.status == TaskStatus::Downloading { format::speed(t.speed_bps) } else { "—".into() };
            ui.label(RichText::new(current).size(20.0).strong().color(color));
            ui.add_space(6.0);

            speed_chart(ui, p, t.id, history, color, t.status == TaskStatus::Downloading);

            if !history.is_empty() {
                ui.add_space(8.0);
                let avg = history.iter().sum::<f64>() / history.len() as f64;
                let peak = history.iter().copied().fold(0.0, f64::max);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(swoop_i18n::tr!("平均")).size(11.5).color(p.text_muted));
                    ui.label(RichText::new(format::speed(avg)).size(11.5).color(p.text));
                    ui.add_space(10.0);
                    ui.label(RichText::new(swoop_i18n::tr!("峰值")).size(11.5).color(p.text_muted));
                    ui.label(RichText::new(format::speed(peak)).size(11.5).color(p.text));
                });
            }
        });
}

/// 纵轴上限：按 1024 进制换到合适单位后取 1 / 2 / 2.5 / 5 × 10ⁿ。返回（字节/秒，该单位下的数值，单位下标）
fn nice_axis(peak: f64) -> (f64, f64, usize) {
    let mut x = peak.max(64.0 * 1024.0);
    let mut unit = 0;
    while x >= 1000.0 && unit < 4 {
        x /= 1024.0;
        unit += 1;
    }
    let mag = 10f64.powf(x.log10().floor());
    let n = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|c| *c >= x).unwrap_or(10.0 * mag);
    (n * 1024f64.powi(unit as i32), n, unit)
}

fn axis_label(v: f64, unit: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s} {}/s", UNITS[unit])
}

/// 速度曲线本体（面积 + 折线），详情面板和右下角卡片共用。最新的点贴右边，按每秒固定间距往左排，
/// `span` 秒铺满整个宽度，更早的点画不下就不画，不满时左侧留空。返回画出的各点（旧的在前）
pub(super) fn speed_curve(
    painter: &egui::Painter,
    plot: Rect,
    history: &[f64],
    span: usize,
    ymax: f32,
    color: Color32,
) -> Vec<Pos2> {
    let history = &history[history.len().saturating_sub(span)..];
    let step = plot.width() / (span.max(2) - 1) as f32;
    let n = history.len();
    let y_of = |v: f64| plot.bottom() - (v as f32 / ymax.max(1.0)).clamp(0.0, 1.0) * plot.height();
    let pts: Vec<Pos2> =
        history.iter().enumerate().map(|(i, v)| pos2(plot.right() - (n - 1 - i) as f32 * step, y_of(*v))).collect();

    // 面积：从曲线到底部的竖向渐隐
    let mut mesh = Mesh::default();
    let fill = color.gamma_multiply(0.28);
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, fill);
        mesh.colored_vertex(pos2(pt.x, plot.bottom()), Color32::TRANSPARENT);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(mesh);
    painter.add(Shape::line(pts.clone(), Stroke::new(1.6, color)));
    pts
}

fn speed_chart(ui: &mut Ui, p: &Palette, id: TaskId, history: &[f64], color: Color32, active: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CHART_H), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let base = Stroke::new(1.0, p.border);
    painter.hline(rect.x_range(), rect.bottom(), base);

    if history.len() < 2 {
        let hint =
            if active { swoop_i18n::tr!("正在采样…") } else { swoop_i18n::tr!("下载时显示速度曲线") };
        painter.text(rect.center(), Align2::CENTER_CENTER, hint, FontId::proportional(12.0), p.text_muted);
        return;
    }

    // 顶部留白，曲线不贴边；上限变化时平滑过渡，避免整条曲线跳动
    let peak = history.iter().copied().fold(0.0, f64::max);
    let (top, top_n, unit) = nice_axis(peak * 1.1);
    let ymax = ui.ctx().animate_value_with_time(Id::new(("speed_ymax", id)), top as f32, anim_secs(0.4)).max(1.0);
    let plot = Rect::from_min_max(pos2(rect.left(), rect.top() + 14.0), rect.max);
    let y_of = |v: f64| plot.bottom() - (v as f32 / ymax).clamp(0.0, 1.0) * plot.height();

    // 网格：上限与一半处两条虚线；只给上限标数值（放在留白里），中线的标签会压到曲线
    for v in [top, top / 2.0] {
        let y = y_of(v);
        painter.extend(Shape::dashed_line(&[pos2(plot.left(), y), pos2(plot.right(), y)], base, 3.0, 3.0));
    }
    painter.text(
        pos2(plot.left(), y_of(top) - 2.0),
        Align2::LEFT_BOTTOM,
        axis_label(top_n, unit),
        FontId::proportional(10.5),
        p.text_muted,
    );

    let pts = speed_curve(painter, plot, history, SPEED_HISTORY_LEN, ymax, color);
    let step = plot.width() / (SPEED_HISTORY_LEN - 1) as f32;
    let n = history.len();

    let last = pts[n - 1];
    if active {
        let time = ui.input(|i| i.time) as f32;
        let pulse = 0.5 + 0.5 * (time * 3.0).sin();
        painter.circle_filled(last, 6.0, color.gamma_multiply(0.18 + 0.2 * pulse));
    }
    painter.circle_filled(last, 2.8, color);

    // 悬停：竖线 + 该秒的速度
    if let Some(pos) = resp.hover_pos() {
        let back = ((plot.right() - pos.x) / step).round().max(0.0) as usize;
        if back < n {
            let pt = pts[n - 1 - back];
            painter.vline(pt.x, plot.y_range(), Stroke::new(1.0, p.text_muted.gamma_multiply(0.5)));
            painter.circle_filled(pt, 3.5, color);
            let when =
                if back == 0 { swoop_i18n::tr!("最新").to_string() } else { swoop_i18n::trf!("{back} 秒前") };
            resp.on_hover_text_at_pointer(format!("{when} · {}", format::speed(history[n - 1 - back])));
        }
    }
}

// ───────────────────────── 区段 ─────────────────────────

fn segment_row(ui: &mut Ui, p: &Palette, idx: usize, seg: &SegmentSnapshot, color: Color32, active: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let small = FontId::proportional(11.5);
    painter.text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        format!("#{}", idx + 1),
        small.clone(),
        p.text_muted,
    );

    let known = seg.end != u64::MAX;
    let len = seg.end.saturating_sub(seg.start);
    let frac = if known && len > 0 { (seg.pos as f64 / len as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let right_text = if known { format!("{:.0}%", frac * 100.0) } else { format::bytes(seg.pos) };
    painter.text(pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, &right_text, small, p.text_muted);

    let bar = Rect::from_min_max(
        pos2(rect.left() + 30.0, rect.center().y - 2.0),
        pos2(rect.right() - 46.0, rect.center().y + 2.0),
    );
    let r = CornerRadius::same(2);
    painter.rect_filled(bar, r, p.track);
    let fill_w = if known { bar.width() * frac } else { bar.width() };
    if fill_w >= 1.0 {
        let c = if known { color } else { color.gamma_multiply(0.45) };
        painter.rect_filled(Rect::from_min_size(bar.min, vec2(fill_w, bar.height())), r, c);
    }
    if active && seg.state == SegmentState::Active && known && frac < 1.0 {
        painter.circle_filled(
            pos2(bar.left() + fill_w, bar.center().y),
            3.0,
            color.lerp_to_gamma(Color32::WHITE, 0.45),
        );
    }

    if resp.hovered() {
        let range = if known {
            format!("{} – {}", format::bytes(seg.start), format::bytes(seg.end))
        } else {
            swoop_i18n::trf!("{} 起，大小未知", format::bytes(seg.start))
        };
        let state = match seg.state {
            SegmentState::Pending => swoop_i18n::tr!("等待"),
            SegmentState::Active => swoop_i18n::tr!("下载中"),
            SegmentState::Done => swoop_i18n::tr!("完成"),
        };
        resp.on_hover_text(swoop_i18n::trf!("{range}\n已下 {}  ·  {state}", format::bytes(seg.pos)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_axis_steps() {
        const MB: f64 = 1024.0 * 1024.0;
        assert_eq!(nice_axis(0.0), (100.0 * 1024.0, 100.0, 1)); // 下限 64 KB/s → 100 KB/s
        assert_eq!(nice_axis(38.4 * MB), (50.0 * MB, 50.0, 2));
        assert_eq!(nice_axis(1.7 * MB), (2.0 * MB, 2.0, 2));
        assert_eq!(nice_axis(2.2 * MB), (2.5 * MB, 2.5, 2));
        // 接近 1024 时进位到下一个单位，不出现 "1000 KB/s"
        assert_eq!(nice_axis(1010.0 * 1024.0), (MB, 1.0, 2));
    }

    #[test]
    fn axis_label_trims_zeros() {
        assert_eq!(axis_label(50.0, 2), "50 MB/s");
        assert_eq!(axis_label(1.25, 2), "1.25 MB/s");
        assert_eq!(axis_label(2.5, 1), "2.5 KB/s");
    }
}
