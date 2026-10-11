//! 多连接进度条的几种画法。所有样式共用同一份 `TaskSnapshot`，便于对比。
//! 默认是像素格（Cells），带呼吸亮头、彗星尾、完成扫光等动效。

use egui::{Color32, CornerRadius, Id, Rect, Sense, pos2, vec2};
use swoop_core::segment::SegmentState;
use swoop_core::{TaskSnapshot, TaskStatus};

use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProgressStyle {
    /// 像素格：按字节区间切成格子，按完成度着色，像 BT 的分片图
    #[default]
    Cells,
    /// 轨道式：每个分段一条独立轨道，段间留缝，活动头用亮点标出
    Lanes,
    /// 叠加式：底层连续的整体进度（淡色），上层叠真实分段位置（实色）
    Overlay,
}

impl ProgressStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cells" => Some(Self::Cells),
            "lanes" => Some(Self::Lanes),
            "overlay" => Some(Self::Overlay),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cells => "cells",
            Self::Lanes => "lanes",
            Self::Overlay => "overlay",
        }
    }
}

const BAR_H: f32 = 6.0;
const CELL_W: f32 = 4.0;
const CELL_GAP: f32 = 2.0;
/// 彗星尾长度（格）
const TAIL: usize = 7;

pub fn status_color(p: &Palette, status: &TaskStatus) -> Color32 {
    match status {
        TaskStatus::Queued | TaskStatus::Probing | TaskStatus::Downloading => p.accent,
        TaskStatus::FileConflict => p.tint_amber,
        TaskStatus::Paused => p.text_muted,
        TaskStatus::Completed => p.success,
        TaskStatus::Failed(_) => p.danger,
    }
}

/// 平滑后的各段已完成字节数（0.3s 动画）
fn animated_pos(ui: &egui::Ui, t: &TaskSnapshot) -> Vec<f32> {
    t.segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| ui.ctx().animate_value_with_time(Id::new(("seg", t.id, idx)), seg.pos as f32, 0.3))
        .collect()
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn bar(ui: &mut egui::Ui, p: &Palette, t: &TaskSnapshot, style: ProgressStyle) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let color = status_color(p, &t.status);

    // 完成扫光：首次看到 Completed 时，绿色从左向右扫一遍（0.6s）
    let done_t = ui.ctx().animate_bool_with_time(Id::new(("done", t.id)), t.status == TaskStatus::Completed, 0.6);
    if t.status == TaskStatus::Completed {
        completed(ui, rect, p, color, ease_out(done_t), style);
        return;
    }
    let Some(total) = t.total.filter(|t| *t > 0) else {
        indeterminate(ui, rect, p, t, color, style);
        return;
    };

    let active = t.status == TaskStatus::Downloading;
    match style {
        ProgressStyle::Cells => cells(ui, rect, p, t, total, color, active),
        ProgressStyle::Lanes => lanes(ui, rect, p, t, total, color, active),
        ProgressStyle::Overlay => overlay(ui, rect, p, t, total, color, active),
    }
}

// ───────────────────────── 像素格 ─────────────────────────

struct Grid {
    n: usize,
    bytes_per_cell: f64,
}

impl Grid {
    fn new(rect: Rect, total: u64) -> Self {
        let n = ((rect.width() + CELL_GAP) / (CELL_W + CELL_GAP)).floor().max(1.0) as usize;
        Self { n, bytes_per_cell: total as f64 / n as f64 }
    }
    fn cell_rect(&self, rect: Rect, i: usize) -> Rect {
        let x = rect.left() + i as f32 * (CELL_W + CELL_GAP);
        Rect::from_min_size(pos2(x, rect.top()), vec2(CELL_W, rect.height()))
    }
}

fn cells(ui: &mut egui::Ui, rect: Rect, p: &Palette, t: &TaskSnapshot, total: u64, color: Color32, active: bool) {
    let pos = animated_pos(ui, t);
    let grid = Grid::new(rect, total);
    let time = ui.input(|i| i.time) as f32;
    // 呼吸：约 0.5Hz
    let pulse = 0.5 + 0.5 * (time * 3.0).sin();
    let bright = color.lerp_to_gamma(Color32::WHITE, 0.55);

    // 每格的完成度、以及它离最近活动头有多远（用于彗星尾）
    let mut frac = vec![0.0f32; grid.n];
    let mut tail = vec![0.0f32; grid.n]; // 0 = 不在尾巴上，1 = 就是头
    let mut heads: Vec<f32> = Vec::new();

    for (seg, pos) in t.segments.iter().zip(&pos) {
        if *pos <= 0.0 {
            continue;
        }
        let s = seg.start as f64;
        let e = s + *pos as f64;
        let first = (s / grid.bytes_per_cell).floor().max(0.0) as usize;
        let last = ((e / grid.bytes_per_cell).ceil() as usize).min(grid.n);
        for (i, f) in frac.iter_mut().enumerate().take(last).skip(first) {
            let a = i as f64 * grid.bytes_per_cell;
            let b = a + grid.bytes_per_cell;
            let covered = (e.min(b) - s.max(a)).max(0.0);
            *f = (*f + (covered / grid.bytes_per_cell) as f32).min(1.0);
        }
        let seg_done = e >= seg.end as f64 - 0.5;
        if active && seg.state == SegmentState::Active && !seg_done {
            let head_i = ((e / grid.bytes_per_cell) as usize).min(grid.n - 1);
            for k in 0..=TAIL {
                if head_i < k {
                    break;
                }
                let i = head_i - k;
                let v = 1.0 - k as f32 / (TAIL as f32 + 1.0);
                tail[i] = tail[i].max(v);
            }
            heads.push(grid.cell_rect(rect, head_i).center().x);
        }
    }

    let painter = ui.painter();
    // 头部柔光（画在格子下面）
    for &x in &heads {
        let glow = color.gamma_multiply(0.18 + 0.22 * pulse);
        painter.circle_filled(pos2(x, rect.center().y), 9.0, glow);
    }
    for i in 0..grid.n {
        let base = p.track.lerp_to_gamma(color, frac[i]);
        let c = if tail[i] > 0.0 {
            // 尾巴：越靠近头越亮，整体随呼吸起伏
            let k = tail[i] * (0.55 + 0.45 * pulse);
            base.lerp_to_gamma(bright, k)
        } else {
            base
        };
        painter.rect_filled(grid.cell_rect(rect, i), CornerRadius::same(1), c);
    }
    if active {
        ui.ctx().request_repaint();
    }
}

/// 大小未知：流动的光带（像素格）或滑块（其他样式）
fn indeterminate(ui: &mut egui::Ui, rect: Rect, p: &Palette, t: &TaskSnapshot, color: Color32, style: ProgressStyle) {
    let painter = ui.painter();
    if !t.status.is_active() {
        painter.rect_filled(rect, CornerRadius::same(3), p.track);
        return;
    }
    let time = ui.input(|i| i.time) as f32;
    if style == ProgressStyle::Cells {
        let grid = Grid::new(rect, 1);
        for i in 0..grid.n {
            // 两组错相的行波，叠加后像一串光点在流动
            let w1 = ((i as f32 * 0.28 - time * 5.0).sin()).max(0.0).powi(3);
            let w2 = ((i as f32 * 0.11 - time * 2.0).sin()).max(0.0).powi(2) * 0.5;
            let b = (0.12 + 0.88 * (w1 + w2)).min(1.0);
            painter.rect_filled(grid.cell_rect(rect, i), CornerRadius::same(1), p.track.lerp_to_gamma(color, b));
        }
    } else {
        painter.rect_filled(rect, CornerRadius::same(3), p.track);
        let frac = (time * 0.45) % 1.0;
        let bw = rect.width() * 0.2;
        let x0 = rect.left() + (rect.width() + bw) * frac - bw;
        let seg = Rect::from_min_max(pos2(x0.max(rect.left()), rect.top()), pos2((x0 + bw).min(rect.right()), rect.bottom()));
        if seg.width() > 0.0 {
            painter.rect_filled(seg, CornerRadius::same(3), color);
        }
    }
    ui.ctx().request_repaint();
}

/// 已完成：实心绿条；首次完成时从左向右扫光
fn completed(ui: &mut egui::Ui, rect: Rect, p: &Palette, color: Color32, t: f32, style: ProgressStyle) {
    let painter = ui.painter();
    if style == ProgressStyle::Cells {
        let grid = Grid::new(rect, 1);
        for i in 0..grid.n {
            painter.rect_filled(grid.cell_rect(rect, i), CornerRadius::same(1), color.gamma_multiply(0.45));
        }
    } else {
        painter.rect_filled(rect, CornerRadius::same(3), p.track);
    }
    let x = rect.left() + rect.width() * t;
    if x - rect.left() >= 1.0 {
        painter.rect_filled(Rect::from_min_max(rect.min, pos2(x, rect.bottom())), CornerRadius::same(3), color);
    }
    if t < 1.0 {
        // 扫光前沿的亮边
        let edge = Rect::from_min_max(pos2((x - 10.0).max(rect.left()), rect.top()), pos2(x, rect.bottom()));
        painter.rect_filled(edge, CornerRadius::same(3), color.lerp_to_gamma(Color32::WHITE, 0.5));
        ui.ctx().request_repaint();
    }
}

// ───────────────────────── 轨道式 / 叠加式（备选） ─────────────────────────

/// 活动下载头：亮点 + 柔光
fn head(ui: &egui::Ui, x: f32, cy: f32, color: Color32, active: bool) {
    if !active {
        return;
    }
    let painter = ui.painter();
    let bright = color.lerp_to_gamma(Color32::WHITE, 0.45);
    painter.circle_filled(pos2(x, cy), 6.0, color.gamma_multiply(0.35));
    painter.circle_filled(pos2(x, cy), 3.0, bright);
}

fn lanes(ui: &mut egui::Ui, rect: Rect, p: &Palette, t: &TaskSnapshot, total: u64, color: Color32, active: bool) {
    let scale = rect.width() / total as f32;
    let pos = animated_pos(ui, t);
    let gap = 1.5;
    let mut heads = Vec::new();
    for (seg, pos) in t.segments.iter().zip(pos) {
        let x0 = rect.left() + seg.start as f32 * scale;
        let x1 = (rect.left() + seg.end as f32 * scale).min(rect.right());
        if x1 - x0 < 1.0 {
            continue;
        }
        let (a, b) = if x1 - x0 > 8.0 { (x0 + gap, x1 - gap) } else { (x0, x1) };
        let lane = Rect::from_min_max(pos2(a, rect.top()), pos2(b, rect.bottom()));
        ui.painter().rect_filled(lane, CornerRadius::same(3), p.track);
        let fx = (a + pos * scale).min(b);
        if fx - a >= 1.0 {
            ui.painter().rect_filled(Rect::from_min_max(pos2(a, rect.top()), pos2(fx, rect.bottom())), CornerRadius::same(3), color);
        }
        if seg.state == SegmentState::Active && pos > 0.0 && fx < b - 1.0 {
            heads.push(fx);
        }
    }
    for x in heads {
        head(ui, x, rect.center().y, color, active);
    }
}

fn overlay(ui: &mut egui::Ui, rect: Rect, p: &Palette, t: &TaskSnapshot, total: u64, color: Color32, active: bool) {
    let scale = rect.width() / total as f32;
    let pos = animated_pos(ui, t);
    let r = CornerRadius::same(3);
    let painter = ui.painter();
    painter.rect_filled(rect, r, p.track);

    let done: f32 = pos.iter().sum();
    let ox = (rect.left() + done * scale).min(rect.right());
    if ox - rect.left() >= 1.0 {
        painter.rect_filled(Rect::from_min_max(rect.min, pos2(ox, rect.bottom())), r, color.gamma_multiply(0.35));
    }
    let mut heads = Vec::new();
    for (seg, pos) in t.segments.iter().zip(pos) {
        if pos <= 0.0 {
            continue;
        }
        let x0 = rect.left() + seg.start as f32 * scale;
        let x1 = (rect.left() + (seg.start as f32 + pos) * scale).min(rect.right());
        if x1 - x0 >= 1.0 {
            painter.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())), r, color);
        }
        if seg.state == SegmentState::Active && (seg.start as f32 + pos) < seg.end as f32 - 0.5 {
            heads.push(x1);
        }
    }
    for x in heads {
        head(ui, x, rect.center().y, color, active);
    }
}
