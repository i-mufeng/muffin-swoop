#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod demo;
mod fonts;
mod theme;
mod ui;

use std::sync::Arc;

use swoop_core::{Config, Engine};

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
    let engine = Arc::new(Engine::new(rt.handle().clone(), Config::default()));

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
            Ok(Box::new(app::SwoopApp::new(cc, engine)))
        }),
    )
}
