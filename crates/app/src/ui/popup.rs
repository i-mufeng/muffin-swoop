//! 独立弹窗：同进程的原生窗口（egui 立即模式多视口），像 IDM 那样不依赖主窗口。
//!
//! 主窗口隐藏到菜单栏、最小化或被完全遮挡时，eframe 只跑 `logic`，无法在其中新建窗口；但只要有
//! 子窗口可见，根视口又会跑完整的 `ui`。所以始终保留一个看不见的 1×1 宿主窗口：有弹窗要显示时
//! 在 `logic` 里把它显示出来换回 `ui` 帧，弹窗建好后再把它藏起来。

use egui::{Color32, Frame, Margin, Pos2, Stroke, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId, vec2};

use crate::app::SwoopApp;
use crate::theme::Palette;

const HOST: &str = "swoop-popup-host";

pub fn host_id() -> ViewportId {
    ViewportId::from_hash_of(HOST)
}

/// 以主窗口所在屏幕居中；`cascade` 让多个同类窗口错开
pub fn centered(ctx: &egui::Context, size: egui::Vec2, cascade: usize) -> Option<Pos2> {
    let monitor = ctx.input_for(ViewportId::ROOT, |i| i.viewport().monitor_size)?;
    let offset = (cascade % 8) as f32 * 28.0;
    Some(((monitor - size) / 2.0).to_pos2() + vec2(offset, offset))
}

pub fn builder(title: &str, size: egui::Vec2, pos: Option<Pos2>) -> ViewportBuilder {
    let mut builder = ViewportBuilder::default()
        .with_title(title)
        .with_app_id("com.muffin.swoop")
        .with_icon(crate::brand::window_icon())
        .with_inner_size(size)
        .with_min_inner_size(size * 0.8);
    if let Some(pos) = pos {
        builder = builder.with_position(pos);
    }
    builder
}

/// eframe 0.36 只有在带事件循环的回调里才能为立即视口建窗口；定时重绘帧（winit `new_events`）
/// 里建不出来，egui 会因回调未被调用而 panic。所以新窗口先登记成延迟视口，由 eframe 在下一帧开头
/// （`initialized_all_windows`）建好窗口，之后再按立即视口画。返回 false 表示窗口还没建好。
fn ensure_window(ctx: &egui::Context, id: ViewportId, builder: &ViewportBuilder, p: &Palette) -> bool {
    ensure_window_filled(ctx, id, builder, p, p.surface)
}

fn ensure_window_filled(
    ctx: &egui::Context,
    id: ViewportId,
    builder: &ViewportBuilder,
    p: &Palette,
    fill: Color32,
) -> bool {
    // 窗口建好后把原生标题栏切到应用主题；窗口每次重建都会先经过未就绪的帧，届时清掉标记
    let themed = id.0.with("native_theme");
    if ctx.input(|i| i.raw.viewports.get(&id).is_some_and(|info| info.native_pixels_per_point.is_some())) {
        if ctx.data(|d| d.get_temp::<crate::theme::Mode>(themed)) != Some(p.mode) {
            ctx.send_viewport_cmd_to(id, ViewportCommand::SetTheme(p.system_theme()));
            ctx.data_mut(|d| d.insert_temp(themed, p.mode));
        }
        return true;
    }
    ctx.data_mut(|d| d.remove::<crate::theme::Mode>(themed));
    ctx.show_viewport_deferred(id, builder.clone(), move |ui, _| {
        egui::CentralPanel::default().frame(Frame::new().fill(fill)).show(ui, |_| {});
    });
    ctx.request_repaint();
    false
}

/// 画一个独立窗口；返回 true 表示用户点了关闭按钮或按了 Esc
pub fn show(
    ctx: &egui::Context,
    id: ViewportId,
    builder: ViewportBuilder,
    p: &Palette,
    add_contents: impl FnOnce(&mut Ui),
) -> bool {
    #[cfg(feature = "screenshot")]
    if crate::shot::active() {
        show_inline(ctx, id, &builder, p, add_contents);
        return false;
    }
    if !ensure_window(ctx, id, &builder, p) {
        return false;
    }
    let mut add_contents = Some(add_contents);
    ctx.show_viewport_immediate(id, builder, |ui, _class| {
        egui::CentralPanel::default().frame(Frame::new().fill(p.surface).inner_margin(Margin::same(20))).show(
            ui,
            |ui| {
                if let Some(add) = add_contents.take() {
                    add(ui);
                }
            },
        );
        ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
    })
}

/// 右下角下载卡片这类小浮窗：置顶、不抢焦点、不进任务栏。圆角和阴影交给系统——eframe 只有主窗口
/// 透明时子窗口才能透明，没法自己画圆角：macOS 用隐藏了标题栏的普通窗口（系统画圆角），
/// 其它平台用无边框窗口，自己描一圈边。
/// 返回内容闭包的结果和用户是否要求关窗（⌘W 等）；窗口还没建好时返回 None。`slot` 只在截图模式下用来错开位置。
pub fn show_card<R>(
    ctx: &egui::Context,
    id: ViewportId,
    size: Vec2,
    pos: Pos2,
    slot: usize,
    p: &Palette,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<(R, bool)> {
    #[cfg(feature = "screenshot")]
    if crate::shot::active() {
        return Some((show_card_inline(ctx, id, size, slot, p, add_contents), false));
    }
    let _ = slot;
    let builder = ViewportBuilder::default()
        .with_title("Swoop")
        .with_app_id("com.muffin.swoop")
        .with_inner_size(size)
        .with_position(pos)
        .with_resizable(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_taskbar(false)
        .with_active(false);
    let builder = if cfg!(target_os = "macos") {
        builder
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(false)
            .with_fullsize_content_view(true)
    } else {
        builder.with_decorations(false)
    };
    if !ensure_window_filled(ctx, id, &builder, p, p.surface) {
        return None;
    }
    let mut add_contents = Some(add_contents);
    ctx.show_viewport_immediate(id, builder, |ui, _class| {
        let inner = egui::CentralPanel::default()
            .frame(card_frame(p))
            .show(ui, |ui| add_contents.take().map(|add| add(ui)))
            .inner;
        inner.map(|r| (r, ui.input(|i| i.viewport().close_requested())))
    })
}

fn card_frame(p: &Palette) -> Frame {
    let frame = Frame::new().fill(p.surface).inner_margin(Margin::same(12));
    if cfg!(target_os = "macos") { frame } else { frame.stroke(Stroke::new(1.0, p.border)) }
}

/// 截图模式：卡片画在主窗口右下角，按 `slot` 往上叠
#[cfg(feature = "screenshot")]
fn show_card_inline<R>(
    ctx: &egui::Context,
    id: ViewportId,
    size: Vec2,
    slot: usize,
    p: &Palette,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let offset = vec2(-16.0, -16.0 - slot as f32 * (size.y + 10.0));
    egui::Area::new(id.0)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, offset)
        .show(ctx, |ui| {
            card_frame(p)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(egui::CornerRadius::same(crate::theme::R_CARD))
                .show(ui, |ui| {
                    ui.set_width(size.x - 24.0);
                    ui.set_height(size.y - 24.0);
                    add_contents(ui)
                })
                .inner
        })
        .inner
}

/// 截图模式：把独立窗口的内容画在主窗口中央，模拟同尺寸的原生窗口，好与主窗口一起截下来
#[cfg(feature = "screenshot")]
fn show_inline(
    ctx: &egui::Context,
    id: ViewportId,
    builder: &ViewportBuilder,
    p: &Palette,
    add_contents: impl FnOnce(&mut Ui),
) {
    let size = builder.inner_size.unwrap_or(vec2(440.0, 300.0));
    egui::Area::new(id.0).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            crate::ui::dialog::frame(p).inner_margin(Margin::same(20)).show(ui, |ui| {
                ui.set_width(size.x - 40.0);
                ui.set_height(size.y - 40.0);
                add_contents(ui);
            });
        },
    );
}

#[derive(Default)]
pub struct PopupHost {
    /// 上一次 `logic` 之后是否跑过 `ui`；没跑说明主窗口隐藏且没有可见子窗口
    ui_ran: bool,
    shown: bool,
}

impl SwoopApp {
    pub(crate) fn wants_popup(&self) -> bool {
        self.add_dialog.open
            || !self.task_windows.is_empty()
            || !self.cards.is_empty()
            || !self.completion.dialogs.is_empty()
    }

    /// 所有独立弹窗；宿主窗口必须最后画，才能在弹窗建好后藏起来
    pub(crate) fn popups_ui(&mut self, ctx: &egui::Context) {
        self.add_dialog_ui(ctx);
        self.task_windows_ui(ctx);
        self.cards_ui(ctx);
        self.completed_window_ui(ctx);
        self.popup_host_ui(ctx);
    }

    /// 每帧 `logic`：有弹窗待显示却没有 `ui` 帧时（主窗口隐藏、最小化或被完全遮挡），显示宿主窗口唤醒界面
    pub(crate) fn popup_logic(&mut self, ctx: &egui::Context) {
        let ui_ran = std::mem::take(&mut self.popup_host.ui_ran);
        if ui_ran || !self.wants_popup() {
            return;
        }
        let host = host_id();
        if ctx.input(|i| i.raw.viewports.contains_key(&host)) {
            ctx.send_viewport_cmd_to(host, ViewportCommand::Visible(true));
            self.popup_host.shown = true;
        } else {
            // 宿主不存在（例如还没跑过 ui 帧）时只能退回到显示主窗口
            self.show_window(ctx);
        }
        ctx.request_repaint();
    }

    /// 每帧 `ui` 的最后：始终保留宿主窗口；弹窗都画完后把宿主藏起来。
    /// 宿主用延迟视口：隐藏时不重绘，不给主窗口的每一帧增加开销。
    pub(crate) fn popup_host_ui(&mut self, ctx: &egui::Context) {
        self.popup_host.ui_ran = true;
        let host = host_id();
        let builder = ViewportBuilder::default()
            .with_title("Swoop")
            .with_inner_size([1.0, 1.0])
            .with_position([-10_000.0, -10_000.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_taskbar(false)
            .with_mouse_passthrough(true)
            .with_active(false)
            .with_visible(false);
        ctx.show_viewport_deferred(host, builder, |_, _| {});
        if self.popup_host.shown {
            ctx.send_viewport_cmd_to(host, ViewportCommand::Visible(false));
            self.popup_host.shown = false;
        }
    }
}
