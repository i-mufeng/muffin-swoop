#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod demo;
mod fonts;
mod settings;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use swoop_core::{Config, Engine, Store};

use crate::settings::{AppSettings, default_download_dir};

/// 数据目录：macOS 为 ~/Library/Application Support/com.muffin.swoop，与 eframe 自身状态同目录
fn data_dir() -> PathBuf {
    dirs::data_dir().map(|d| d.join("com.muffin.swoop")).unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn".into()),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("启动 tokio runtime 失败");

    let db_path = data_dir().join("swoop.db");
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::error!(path = %db_path.display(), "打开数据库失败: {e}");
            std::process::exit(1);
        }
    };
    let settings = AppSettings::load(&store, default_download_dir());

    let config = Config {
        speed_limit: if settings.speed_limit_on { ui::format::mbps_to_bps(settings.speed_limit_mbps) } else { 0 },
        ..Config::default()
    };
    let engine = Arc::new(Engine::with_store(rt.handle().clone(), config, store.clone()));
    match engine.load(settings.auto_resume) {
        Ok(n) => tracing::info!(count = n, auto_resume = settings.auto_resume, "已恢复历史任务"),
        Err(e) => tracing::warn!("恢复历史任务失败: {e}"),
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Swoop")
        .with_app_id("com.muffin.swoop")
        .with_inner_size([1080.0, 700.0])
        .with_min_inner_size([760.0, 480.0]);
    if cfg!(target_os = "macos") {
        // 透明标题栏 + 内容延伸到顶部，红绿灯按钮浮在侧栏上方
        viewport = viewport
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_fullsize_content_view(true);
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    #[cfg(feature = "screenshot")]
    if std::env::var("SWOOP_RENDERER").as_deref() == Ok("glow") {
        options.renderer = eframe::Renderer::Glow;
    }

    eframe::run_native(
        "Swoop",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::SwoopApp::new(cc, engine, store, settings)))
        }),
    )
}
