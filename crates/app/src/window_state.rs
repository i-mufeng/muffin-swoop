//! 主窗口几何的持久化兜底。
//!
//! eframe 每 30 秒和退出时把根窗口的位置/尺寸写进存储（`app.ron` 的 `window` 键），下次启动直接按它建窗口。
//! 窗口最小化时（Windows 上读到 0×0，位置是 -32000）或尺寸异常时也会照写，eframe 只把尺寸夹到 64×64；
//! 而系统建窗时不会用 `min_inner_size` 纠正初始尺寸，于是下次启动就是一个很小的窗口。
//! 这里在 `App::save`（eframe 写完 `window` 之后才调用）里把不合格的记录换回上一次合格的值。
//!
//! 另外，Windows 上从安装器完成页（NSIS `Exec`）启动时，主窗口第一次显示可能只剩一截标题栏（最小化/尺寸异常），
//! 所以启动后的几秒内每帧检查一次，发现就恢复成正常窗口。

use std::time::{Duration, Instant};

use egui::{Pos2, Vec2, ViewportCommand, vec2};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SIZE: Vec2 = vec2(1080.0, 700.0);
/// 侧栏 208 + 列表行最窄约 500 + 详情面板 300
pub const MIN_SIZE: Vec2 = vec2(1010.0, 480.0);

/// eframe 存储里 `window` 键的键名
const KEY: &str = "window";

/// 与 `egui_winit::WindowSettings` 字段一致，只用来读写 eframe 存的那份记录
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    inner_position_pixels: Option<Pos2>,
    outer_position_pixels: Option<Pos2>,
    fullscreen: bool,
    maximized: bool,
    inner_size_points: Option<Vec2>,
}

impl WindowGeometry {
    pub fn load(storage: &dyn eframe::Storage) -> Option<Self> {
        eframe::get_value(storage, KEY)
    }

    /// 尺寸不小于最小窗口才算合格；最小化时读到的 0×0 也在这里被挡掉
    pub fn usable(&self) -> bool {
        self.inner_size_points.is_some_and(fits)
    }
}

/// 留 2pt 给缩放取整
fn fits(size: Vec2) -> bool {
    size.x >= MIN_SIZE.x - 2.0 && size.y >= MIN_SIZE.y - 2.0
}

/// 启动后检查主窗口的时长：覆盖首帧显示和安装器退出前后的抖动
const STARTUP_CHECK: Duration = Duration::from_secs(5);

/// 记住最近一次合格的窗口几何，eframe 写入不合格的值时换回它
pub struct WindowGuard {
    last_good: Option<WindowGeometry>,
    /// 启动检查截止时间；检查结束后为 None
    startup_until: Option<Instant>,
}

impl WindowGuard {
    pub fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        let last_good = storage.and_then(WindowGeometry::load).filter(WindowGeometry::usable);
        Self { last_good, startup_until: Some(Instant::now() + STARTUP_CHECK) }
    }

    /// 每帧 `logic` 调用（根视口）：启动后几秒内主窗口若是最小化或小于最小尺寸，恢复成正常窗口
    pub fn check_startup(&mut self, ctx: &egui::Context, hidden: bool) {
        let Some(until) = self.startup_until else { return };
        if hidden || Instant::now() >= until {
            self.startup_until = None;
            return;
        }
        let (minimized, size) = ctx.input(|i| (i.viewport().minimized, i.viewport().inner_rect.map(|r| r.size())));
        if minimized == Some(true) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        } else if size.is_some_and(|size| !fits(size)) {
            tracing::warn!(?size, "启动时主窗口尺寸异常，恢复默认尺寸");
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(DEFAULT_SIZE));
        }
        // 最小化时 eframe 不主动重绘，定时唤醒才能继续检查
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    /// 在 `App::save` 里调用：eframe 刚写完 `window` 键
    pub fn save(&mut self, storage: &mut dyn eframe::Storage) {
        match (WindowGeometry::load(storage), &self.last_good) {
            (Some(current), _) if current.usable() => self.last_good = Some(current),
            (_, Some(good)) => eframe::set_value(storage, KEY, good),
            // 没有合格记录就删掉，下次按默认尺寸居中
            (_, None) => storage.remove_string(KEY),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct MemStorage(HashMap<String, String>);

    impl eframe::Storage for MemStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    /// eframe 实际写入的格式
    const MINIMIZED: &str = "(inner_position_pixels:Some((x:-32000.0,y:-32000.0)),outer_position_pixels:Some((x:-32000.0,y:-32000.0)),fullscreen:false,maximized:false,inner_size_points:Some((x:0.0,y:0.0)))";
    const NORMAL: &str = "(inner_position_pixels:Some((x:342.0,y:124.0)),outer_position_pixels:Some((x:342.0,y:124.0)),fullscreen:false,maximized:false,inner_size_points:Some((x:1200.0,y:760.0)))";

    #[test]
    fn parses_eframe_format() {
        let mut storage = MemStorage::default();
        storage.0.insert(KEY.into(), NORMAL.into());
        let geometry = WindowGeometry::load(&storage).unwrap();
        assert_eq!(geometry.inner_size_points, Some(vec2(1200.0, 760.0)));
        assert!(geometry.usable());
    }

    #[test]
    fn replaces_minimized_size_with_last_good() {
        let mut storage = MemStorage::default();
        storage.0.insert(KEY.into(), NORMAL.into());
        let mut guard = WindowGuard::new(Some(&storage));

        storage.0.insert(KEY.into(), MINIMIZED.into());
        guard.save(&mut storage);
        assert_eq!(WindowGeometry::load(&storage).unwrap().inner_size_points, Some(vec2(1200.0, 760.0)));
    }

    #[test]
    fn falls_back_to_default_without_good_record() {
        let mut storage = MemStorage::default();
        storage.0.insert(KEY.into(), MINIMIZED.into());
        let mut guard = WindowGuard::new(Some(&storage));
        assert!(guard.last_good.is_none());

        guard.save(&mut storage);
        assert_eq!(WindowGeometry::load(&storage), None);
    }
}
