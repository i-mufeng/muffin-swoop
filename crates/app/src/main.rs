#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod batch;
mod brand;
mod bridge;
mod categories;
mod cli;
mod clipboard;
mod completion;
mod demo;
mod fonts;
mod ftp_settings;
mod grabber_tasks;
mod instance;
mod link_import;
mod list_layout;
mod media_tasks;
mod native_drag;
mod network_settings;
mod notifications;
mod screen;
mod settings;
#[cfg(feature = "screenshot")]
mod shot;
mod startup;
mod task_transfer;
mod theme;
mod tray;
mod ui;
mod updates;
mod window_state;

use std::sync::{Arc, Mutex};

use swoop_core::{Config, Engine, Store};

use crate::settings::{AppSettings, default_download_dir};

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--swoop-install-update") {
        let result = if args.len() == 5 {
            swoop_updater::run_helper_ready(
                std::path::Path::new(&args[2]),
                &args[3].to_string_lossy(),
                std::path::Path::new(&args[4]),
            )
        } else if args.len() == 4 {
            swoop_updater::run_helper(std::path::Path::new(&args[2]), &args[3].to_string_lossy())
        } else {
            Err(swoop_i18n::tr!("更新安装参数无效").into())
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args_os().len() > 1 {
        cli::attach_parent_console();
        std::process::exit(cli::run(std::env::args_os().skip(1)) as i32);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn".into()),
        )
        .init();

    let data = instance::data_dir();
    let ownership = match instance::InstanceGuard::acquire(&data) {
        Ok(ownership) => ownership,
        Err(error) => {
            tracing::error!("无法取得实例锁：{error}");
            std::process::exit(1);
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect(swoop_i18n::tr!("启动 tokio runtime 失败"));

    let Some(ownership) = ownership else {
        match rt.block_on(instance::send(&data, &instance::Command::Show, true)) {
            Ok(result) if result.get("ok").and_then(serde_json::Value::as_bool) == Some(true) => {
                return Ok(());
            }
            _ => {
                tracing::error!("另一个实例持有数据目录，但未能唤醒其窗口；未打开数据库");
                std::process::exit(3);
            }
        }
    };
    // Retain ownership until after the runtime and all engine workers are gone.
    let ownership = Arc::new(ownership);

    let db_path = data.join("swoop.db");
    let fresh_install = !db_path.exists();
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::error!(path = %db_path.display(), "打开数据库失败: {e}");
            std::process::exit(1);
        }
    };
    let settings = AppSettings::load(&store, default_download_dir());
    if fresh_install {
        categories::Categories::init_fresh(&store, &settings.default_dir);
    }

    let config = Config {
        connections: settings.connections,
        speed_limit: if settings.speed_limit_on { ui::format::mbps_to_bps(settings.speed_limit_mbps) } else { 0 },
        ..Config::default()
    };
    let engine = Arc::new(Engine::with_store(rt.handle().clone(), config, store.clone()));
    if let Err(error) = engine.set_network_policy(network_settings::load(&store)) {
        tracing::warn!("网络设置未生效：{error}");
    }
    // 先订阅再自动续传，避免小文件在 GUI 初始化之前完成而丢失提醒。
    let events = engine.subscribe();
    match engine.load(settings.auto_resume) {
        Ok(n) => tracing::info!(count = n, auto_resume = settings.auto_resume, "已恢复历史任务"),
        Err(e) => tracing::warn!("恢复历史任务失败: {e}"),
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Swoop")
        .with_app_id("com.muffin.swoop")
        .with_icon(brand::window_icon())
        .with_inner_size(window_state::DEFAULT_SIZE)
        .with_min_inner_size(window_state::MIN_SIZE);
    if cfg!(target_os = "macos") {
        // 透明标题栏 + 内容延伸到顶部，红绿灯按钮浮在侧栏上方
        viewport = viewport.with_title_shown(false).with_titlebar_shown(false).with_fullsize_content_view(true);
    }
    #[cfg(windows)]
    {
        viewport = viewport.with_drag_and_drop(false);
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(feature = "screenshot")]
    if std::env::var("SWOOP_RENDERER").as_deref() == Ok("glow") {
        options.renderer = eframe::Renderer::Glow;
    }

    // 只把 handle 交给界面；rt 本身留在 main 栈上，活到窗口关闭
    let handle = rt.handle().clone();
    let control = Arc::new(Mutex::new(None));
    let control_slot = control.clone();
    let instance_for_gui = ownership.clone();
    let result = eframe::run_native(
        "Swoop",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let app = app::SwoopApp::new(cc, handle.clone(), engine.clone(), store, settings, events);
            *control_slot.lock().unwrap() = Some(instance::ControlServer::start(
                &instance_for_gui,
                engine,
                &handle,
                app.bridge.activation_callback(),
                app.bridge.directory_callback(),
                app.bridge.control_callback(),
            )?);
            Ok(Box::new(app))
        }),
    );
    drop(control.lock().unwrap().take());
    drop(rt);
    drop(ownership);
    result
}
