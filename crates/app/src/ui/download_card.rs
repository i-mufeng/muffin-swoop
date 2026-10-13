//! 右下角下载卡片：单个下载开始时贴着屏幕右下角出现——从右侧滑入；在新建下载窗口里点「开始下载」的，
//! 从对话框的位置飞过去。卡片显示最近一分钟的速度曲线和进度，完成后原地变成完成卡片，几秒后自己收起。
//! 每张卡片是独立的置顶小窗口，不抢焦点，主窗口隐藏到菜单栏时照样显示。设置里可换回居中的进度窗口。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, Id, Label, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
    ViewportId, pos2, vec2,
};
use swoop_core::{TaskId, TaskSnapshot, TaskStatus};

use super::filekind::FileKind;
use super::progress::status_color;
use super::{anim_secs, file_tile, format, ghost_button, icon_button, pill, popup, primary_button};
use crate::app::SwoopApp;
use crate::completion::FileAction;
use crate::theme::{Palette, R_TILE};

/// 单个下载开始时的进度提示
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressPopup {
    /// 屏幕右下角的下载卡片
    #[default]
    Card,
    /// 屏幕中央的进度窗口（对标 IDM 的下载进度对话框）
    Window,
    Off,
}

impl ProgressPopup {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "card" => Some(Self::Card),
            "window" => Some(Self::Window),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Window => "window",
            Self::Off => "off",
        }
    }
}

pub const SIZE: Vec2 = vec2(340.0, 110.0);
/// 同时显示的卡片上限；其余排队，最上面那张标出还有几个
const MAX_VISIBLE: usize = 3;
/// 与屏幕可用区域边缘的距离、卡片之间的间距
const MARGIN: f32 = 16.0;
const GAP: f32 = 10.0;
/// 滑入的起点在目标位置右侧多远
const SLIDE_IN: f32 = 56.0;
/// 滑入、上下挪位的时长；从新建下载窗口飞过来的路程长，慢一点
const MOVE_SECS: f64 = 0.28;
const FLY_SECS: f64 = 0.42;
/// 完成后停留多久自动收起；鼠标停在卡片上时重新计时
const LINGER: Duration = Duration::from_secs(8);
/// 曲线横跨的秒数
const CHART_SPAN: usize = 60;
const CHART_H: f32 = 34.0;

/// 窗口的一段移动；从窗口建好、第一次画出内容时才开始计时，免得建窗口那几帧把动画吃掉
#[derive(Clone, Copy, Debug)]
struct Motion {
    from: Pos2,
    to: Pos2,
    secs: f64,
    started: Option<f64>,
}

impl Motion {
    /// `now` 时刻的位置（三次缓出）和是否已经到位
    fn at(&self, now: f64) -> (Pos2, bool) {
        let Some(started) = self.started else { return (self.from, false) };
        let t = ((now - started) / self.secs).clamp(0.0, 1.0) as f32;
        let eased = 1.0 - (1.0 - t).powi(3);
        (self.from.lerp(self.to, eased), t >= 1.0)
    }
}

struct Card {
    id: TaskId,
    motion: Option<Motion>,
    /// 最近一帧窗口所在的位置；还没出现过时为 None
    pos: Option<Pos2>,
    /// 飞入起点（新建下载窗口里卡片大小的那块居中位置），只用于第一次出现
    origin: Option<Pos2>,
    done_since: Option<Instant>,
}

#[derive(Default)]
pub struct Cards {
    cards: Vec<Card>,
    /// 屏幕可用区域：第一张卡片出现时按鼠标所在屏幕取，后来的卡片沿用，全部收起后再重新取
    area: Option<Rect>,
}

impl Cards {
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn contains(&self, id: TaskId) -> bool {
        self.cards.iter().any(|c| c.id == id)
    }

    /// 卡片正显示在屏幕上（不是排在后面等空位）
    pub fn visible(&self, id: TaskId) -> bool {
        self.cards.iter().take(MAX_VISIBLE).any(|c| c.id == id)
    }

    /// `from` 是新建下载窗口的位置，卡片从那里飞过来；None 时从右侧滑入
    pub fn open(&mut self, id: TaskId, from: Option<Rect>) {
        if self.contains(id) {
            return;
        }
        if self.cards.is_empty() {
            self.area = crate::screen::work_area_at_cursor();
        }
        let origin = from.map(|rect| rect.center() - SIZE / 2.0);
        self.cards.push(Card { id, motion: None, pos: None, origin, done_since: None });
    }

    pub fn close(&mut self, id: TaskId) {
        self.cards.retain(|c| c.id != id);
        if self.cards.is_empty() {
            self.area = None;
        }
    }
}

/// 第 `slot` 张卡片（0 在最下面）的左上角
fn slot_pos(area: Rect, slot: usize) -> Pos2 {
    pos2(area.right() - MARGIN - SIZE.x, area.bottom() - MARGIN - SIZE.y - slot as f32 * (SIZE.y + GAP))
}

/// 问不到系统时，按主窗口所在屏幕估算，底部让出任务栏 / Dock 的高度
fn fallback_area(ctx: &egui::Context) -> Rect {
    let size = ctx.input_for(ViewportId::ROOT, |i| i.viewport().monitor_size).unwrap_or(vec2(1440.0, 900.0));
    Rect::from_min_max(Pos2::ZERO, pos2(size.x, size.y - 48.0))
}

fn viewport_id(id: TaskId) -> ViewportId {
    ViewportId::from_hash_of(("swoop-download-card", id))
}

enum CardAction {
    /// 点卡片空白处：调出主窗口并选中这个任务
    Show(TaskId),
    Close(TaskId),
    Pause(TaskId),
    Resume(TaskId),
    Open(TaskId, PathBuf, FileAction),
}

impl SwoopApp {
    pub fn cards_ui(&mut self, ctx: &egui::Context) {
        // 本帧 ui 里刚建的任务（新建下载窗口、吐司上的「下载」）还不在帧开头取的快照里，向引擎补一份；
        // 两边都没有才是任务被移除了
        let fresh: Vec<TaskSnapshot> = self
            .cards
            .cards
            .iter()
            .filter(|c| !self.tasks.iter().any(|t| t.id == c.id))
            .filter_map(|c| self.engine.snapshot_one(c.id))
            .collect();
        let tasks: Vec<&TaskSnapshot> = self.tasks.iter().chain(&fresh).collect();
        self.cards.cards.retain(|c| tasks.iter().any(|t| t.id == c.id));
        if self.cards.cards.is_empty() {
            self.cards.area = None;
            return;
        }
        let area = *self.cards.area.get_or_insert_with(|| fallback_area(ctx));
        let p = self.palette;
        let now = ctx.input(|i| i.time);
        let visible = self.cards.cards.len().min(MAX_VISIBLE);
        let more = self.cards.cards.len() - visible;
        let mut actions = Vec::new();
        let mut moving = false;
        for (slot, card) in self.cards.cards.iter_mut().take(MAX_VISIBLE).enumerate() {
            let Some(task) = tasks.iter().find(|t| t.id == card.id) else { continue };
            let target = slot_pos(area, slot);
            if card.motion.is_none_or(|m| m.to != target) {
                // 下面的卡片收起后，上面的跟着挪下来；没出现过的从新建窗口飞来或从右侧滑入
                let (from, secs) = match (card.pos, card.origin.take()) {
                    (Some(pos), _) => (pos, MOVE_SECS),
                    (None, Some(origin)) => (origin, FLY_SECS),
                    (None, None) => (target + vec2(SLIDE_IN, 0.0), MOVE_SECS),
                };
                card.motion = Some(Motion { from, to: target, secs, started: None });
            }
            let motion = card.motion.as_mut().expect("上面刚设置过");
            let (pos, arrived) = motion.at(now);
            moving |= !arrived;
            card.pos = Some(pos);

            if task.status == TaskStatus::Completed {
                card.done_since.get_or_insert_with(Instant::now);
            } else {
                card.done_since = None;
            }
            let history = crate::demo::speed_history(task).unwrap_or_else(|| self.engine.speed_history(task.id));
            let extra = if slot + 1 == visible { more } else { 0 };
            let shown = popup::show_card(ctx, viewport_id(card.id), SIZE, pos, slot, &p, |ui| {
                let hovered = ui.input(|i| i.pointer.has_pointer());
                (card_contents(ui, &p, task, &history, extra), hovered)
            });
            if let Some(((action, hovered), close)) = shown {
                if motion.started.is_none() {
                    motion.started = Some(now);
                    moving = true;
                }
                if hovered && let Some(done) = &mut card.done_since {
                    *done = Instant::now();
                }
                actions.extend(action);
                if close {
                    actions.push(CardAction::Close(card.id));
                }
            }
            if card.done_since.is_some_and(|done| done.elapsed() >= LINGER) {
                actions.push(CardAction::Close(card.id));
            }
        }
        if moving {
            ctx.request_repaint();
        } else if self.cards.cards.iter().any(|c| c.done_since.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        for action in actions {
            match action {
                CardAction::Show(id) => {
                    self.show_window(ctx);
                    self.selected = Some(id);
                }
                CardAction::Close(id) => self.cards.close(id),
                CardAction::Pause(id) => {
                    if let Err(error) = self.engine.pause(id) {
                        self.toasts.push_error(swoop_i18n::tr!("暂停失败"), error.to_string());
                    }
                }
                CardAction::Resume(id) => {
                    if let Err(error) = self.engine.resume(id) {
                        self.toasts.push_error(swoop_i18n::tr!("继续失败"), error.to_string());
                    }
                }
                CardAction::Open(id, path, action) => {
                    self.cards.close(id);
                    if let Err(error) = self.completion.open_path(path, action) {
                        self.toasts.push_error(swoop_i18n::tr!("无法打开"), error);
                    }
                }
            }
        }
    }
}

/// 卡片处于哪个阶段；阶段变了（例如下载完成）下半部分淡入新内容
fn phase(status: &TaskStatus) -> u8 {
    match status {
        TaskStatus::Completed => 1,
        TaskStatus::Failed(_) => 2,
        _ => 0,
    }
}

fn card_contents(ui: &mut Ui, p: &Palette, t: &TaskSnapshot, history: &[f64], more: usize) -> Option<CardAction> {
    let mut action = None;
    // 整张卡片可点，点空白处调出主窗口；先登记，后加的按钮叠在上面优先响应
    let body = ui.interact(ui.max_rect(), ui.id().with("card_body"), Sense::click());
    if body.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        action = Some(CardAction::Show(t.id));
    }

    let now = ui.input(|i| i.time);
    let key = Id::new(("card_phase", t.id));
    let current = phase(&t.status);
    // 第一次出现不淡入（窗口本身在滑动），之后阶段一变就从头淡入
    let (last, since) = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(key, || (current, f64::NEG_INFINITY)));
    let since = if last == current {
        since
    } else {
        ui.data_mut(|d| d.insert_temp(key, (current, now)));
        now
    };
    let fade = ((now - since) / f64::from(anim_secs(0.25))).clamp(0.0, 1.0) as f32;
    if fade < 1.0 {
        ui.ctx().request_repaint();
    }

    let color = status_color(p, &t.status);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        tile(ui, p, t);
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            use egui_phosphor::regular as i;
            if icon_button(ui, p, i::X, swoop_i18n::tr!("关闭")).clicked() {
                action = Some(CardAction::Close(t.id));
            }
            if t.status.is_active() {
                if icon_button(ui, p, i::PAUSE, swoop_i18n::tr!("暂停")).clicked() {
                    action = Some(CardAction::Pause(t.id));
                }
            } else if t.status == TaskStatus::Paused && icon_button(ui, p, i::PLAY, swoop_i18n::tr!("继续")).clicked()
            {
                action = Some(CardAction::Resume(t.id));
            }
            if more > 0 {
                ui.add_space(4.0);
                pill(ui, &format!("+{more}"), p.accent_soft, p.accent)
                    .on_hover_text(swoop_i18n::trf!("还有 {} 个下载", more));
            }
            ui.add_space(6.0);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(Label::new(RichText::new(&t.file_name).size(13.5).strong().color(p.text)).truncate());
                let (line, line_color) = status_line(p, t);
                ui.add(Label::new(RichText::new(line).size(12.0).color(line_color)).truncate());
            });
        });
    });

    ui.add_space(8.0);
    ui.scope(|ui| {
        ui.set_opacity(fade);
        match &t.status {
            TaskStatus::Completed => {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if primary_button(ui, p, swoop_i18n::tr!("打开")).clicked() {
                        action = Some(CardAction::Open(t.id, t.path.clone(), FileAction::Open));
                    }
                    if ghost_button(ui, p, swoop_i18n::tr!("打开文件夹")).clicked() {
                        action = Some(CardAction::Open(t.id, t.path.clone(), FileAction::Reveal));
                    }
                });
            }
            TaskStatus::Failed(_) => {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, p, swoop_i18n::tr!("继续")).clicked() {
                        action = Some(CardAction::Resume(t.id));
                    }
                });
            }
            _ => {
                chart(ui, p, t, history, color);
                ui.add_space(6.0);
                bar(ui, p, t, color);
            }
        }
    });
    action
}

fn tile(ui: &mut Ui, p: &Palette, t: &TaskSnapshot) {
    let (glyph, color) = match &t.status {
        TaskStatus::Completed => (egui_phosphor::fill::CHECK_CIRCLE, p.success),
        TaskStatus::Failed(_) => (egui_phosphor::fill::WARNING_CIRCLE, p.danger),
        _ => return file_tile(ui, p, FileKind::from_name(&t.file_name), 34.0),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(R_TILE), p.soft(color));
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, crate::fonts::icon_fill(17.0), color);
}

/// 文件名下面那行：进度、速度、剩余时间，或当前状态
fn status_line(p: &Palette, t: &TaskSnapshot) -> (String, Color32) {
    let percent = t.progress().map(|f| format!("{:.0}%", f * 100.0));
    let text = match &t.status {
        TaskStatus::Downloading => {
            let mut parts = vec![percent.unwrap_or_else(|| format::bytes(t.downloaded)), format::speed(t.speed_bps)];
            if let Some(eta) = t.eta_secs() {
                parts.push(swoop_i18n::trf!("剩余 {}", format::duration(eta)));
            }
            parts.join(" · ")
        }
        TaskStatus::Queued => swoop_i18n::tr!("排队中").into(),
        TaskStatus::Probing => swoop_i18n::tr!("连接中").into(),
        TaskStatus::FileConflict => swoop_i18n::tr!("等待确认").into(),
        TaskStatus::Paused => match percent {
            Some(percent) => format!("{} · {percent}", swoop_i18n::tr!("已暂停")),
            None => swoop_i18n::tr!("已暂停").into(),
        },
        TaskStatus::Completed => {
            format!("{} · {}", swoop_i18n::tr!("下载完成"), format::bytes(t.total.unwrap_or(t.downloaded)))
        }
        TaskStatus::Failed(reason) => return (swoop_i18n::trf!("下载失败：{}", reason), p.danger),
    };
    (text, p.text_muted)
}

/// 最近一分钟的速度曲线；纵轴按这段的峰值留点余量，峰值变化时平滑过渡
fn chart(ui: &mut Ui, p: &Palette, t: &TaskSnapshot, history: &[f64], color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CHART_H), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    if history.len() < 2 {
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
        return;
    }
    let recent = &history[history.len().saturating_sub(CHART_SPAN)..];
    let peak = recent.iter().copied().fold(0.0, f64::max).max(64.0 * 1024.0);
    let ymax = ui.ctx().animate_value_with_time(Id::new(("card_ymax", t.id)), (peak * 1.15) as f32, anim_secs(0.4));
    let color = if t.status == TaskStatus::Downloading { color } else { p.text_muted };
    super::detail::speed_curve(ui.painter(), rect, history, CHART_SPAN, ymax, color);
}

/// 细进度条；大小未知时整条淡色，表示还在下
fn bar(ui: &mut Ui, p: &Palette, t: &TaskSnapshot, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
    let painter = ui.painter();
    let radius = CornerRadius::same(2);
    painter.rect_filled(rect, radius, p.track);
    match t.progress() {
        Some(done) => {
            let done = ui.ctx().animate_value_with_time(Id::new(("card_bar", t.id)), done, anim_secs(0.3));
            painter.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * done, rect.height())), radius, color);
        }
        None if t.status.is_active() => {
            painter.rect_filled(rect, radius, color.gamma_multiply(0.35));
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_popup_round_trips() {
        for popup in [ProgressPopup::Card, ProgressPopup::Window, ProgressPopup::Off] {
            assert_eq!(ProgressPopup::parse(popup.as_str()), Some(popup));
        }
        assert_eq!(ProgressPopup::parse("unknown"), None);
    }

    #[test]
    fn cards_stack_up_from_bottom_right_of_work_area() {
        let area = Rect::from_min_max(pos2(0.0, 25.0), pos2(1440.0, 820.0));
        let bottom = slot_pos(area, 0);
        assert_eq!(bottom, pos2(1440.0 - MARGIN - SIZE.x, 820.0 - MARGIN - SIZE.y));
        let above = slot_pos(area, 1);
        assert_eq!(above.x, bottom.x);
        assert_eq!(bottom.y - above.y, SIZE.y + GAP);
    }

    #[test]
    fn motion_waits_for_window_then_eases_into_place() {
        let mut motion = Motion { from: pos2(0.0, 0.0), to: pos2(100.0, 0.0), secs: 0.5, started: None };
        assert_eq!(motion.at(10.0), (pos2(0.0, 0.0), false), "窗口没建好前停在起点");
        motion.started = Some(10.0);
        let (mid, arrived) = motion.at(10.25);
        assert!(!arrived && mid.x > 50.0, "缓出：前半程走得多 {mid:?}");
        assert_eq!(motion.at(11.0), (pos2(100.0, 0.0), true));
    }

    #[test]
    fn only_first_cards_are_visible_and_closing_frees_a_slot() {
        let mut cards = Cards::default();
        for id in 1..=4 {
            cards.open(TaskId(id), None);
        }
        cards.open(TaskId(1), None);
        assert_eq!(cards.cards.len(), 4, "同一任务不重复开卡片");
        assert!(cards.visible(TaskId(3)) && !cards.visible(TaskId(4)));
        cards.close(TaskId(2));
        assert!(cards.visible(TaskId(4)));
        for id in [1, 3, 4] {
            cards.close(TaskId(id));
        }
        assert!(cards.is_empty() && cards.area.is_none());
    }

    #[test]
    fn card_flies_from_dialog_center() {
        let mut cards = Cards::default();
        let dialog = Rect::from_min_size(pos2(400.0, 200.0), vec2(580.0, 500.0));
        cards.open(TaskId(1), Some(dialog));
        assert_eq!(cards.cards[0].origin, Some(dialog.center() - SIZE / 2.0));
    }
}
