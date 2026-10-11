//! 独立弹窗：同进程的原生窗口（egui 立即模式多视口），像 IDM 那样不依赖主窗口。
//!
//! 主窗口隐藏到菜单栏、最小化或被完全遮挡时，eframe 只跑 `logic`，无法在其中新建窗口；但只要有
//! 子窗口可见，根视口又会跑完整的 `ui`。所以始终保留一个看不见的 1×1 宿主窗口：有弹窗要显示时
//! 在 `logic` 里把它显示出来换回 `ui` 帧，弹窗建好后再把它藏起来。

use egui::{Frame, Margin, Pos2, Ui, ViewportBuilder, ViewportCommand, ViewportId, vec2};

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
fn ensure_window(ctx: &egui::Context, id: ViewportId, builder: &ViewportBuilder, fill: egui::Color32) -> bool {
    if ctx.input(|i| i.raw.viewports.get(&id).is_some_and(|info| info.native_pixels_per_point.is_some())) {
        return true;
    }
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
    if !ensure_window(ctx, id, &builder, p.surface) {
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

#[derive(Default)]
pub struct PopupHost {
    /// 上一次 `logic` 之后是否跑过 `ui`；没跑说明主窗口隐藏且没有可见子窗口
    ui_ran: bool,
    shown: bool,
}

impl SwoopApp {
    pub(crate) fn wants_popup(&self) -> bool {
        self.add_dialog.open || !self.task_windows.is_empty() || !self.completion.dialogs.is_empty()
    }

    /// 所有独立弹窗；宿主窗口必须最后画，才能在弹窗建好后藏起来
    pub(crate) fn popups_ui(&mut self, ctx: &egui::Context) {
        self.add_dialog_ui(ctx);
        self.task_windows_ui(ctx);
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
