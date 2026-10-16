use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use swoop_core::Store;
use swoop_updater::{Candidate, UpdateConfig};
use tokio_util::sync::CancellationToken;

enum Event {
    Checked(Result<Option<Candidate>, String>),
    Downloaded(Result<PathBuf, String>),
    Prepared(Result<(PathBuf, PathBuf, String), String>),
    Ready(Result<std::process::Child, String>),
}

pub struct Updates {
    pub config: UpdateConfig,
    pub candidate: Option<Candidate>,
    pub archive: Option<PathBuf>,
    pub error: Option<String>,
    pub message: String,
    handle: tokio::runtime::Handle,
    store: Arc<Store>,
    receiver: Option<mpsc::Receiver<Event>>,
    cancel: CancellationToken,
    last_attempt: u64,
    installing: bool,
}
impl Updates {
    pub fn load(store: Arc<Store>, handle: tokio::runtime::Handle) -> Self {
        // 只读取自动检查开关；更新源/公钥/通道/间隔一律使用内置官方值，
        // 历史版本存过的自定义配置在加载时清除并规范化存储。
        let automatic_check = store
            .get_setting("updates")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str::<UpdateConfig>(&value).ok())
            .map(|config| config.automatic_check)
            .unwrap_or(false);
        let config = UpdateConfig { automatic_check, ..UpdateConfig::default() };
        let _ = store.set_setting("updates", &Self::stored_config(automatic_check));
        let last_attempt =
            store.get_setting("update_last_check").ok().flatten().and_then(|value| value.parse().ok()).unwrap_or(0);
        Self {
            config,
            candidate: None,
            archive: None,
            error: None,
            message: String::new(),
            handle,
            store,
            receiver: None,
            cancel: CancellationToken::new(),
            last_attempt,
            installing: false,
        }
    }
    fn stored_config(automatic_check: bool) -> String {
        serde_json::json!({ "automatic_check": automatic_check }).to_string()
    }
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn installing(&self) -> bool {
        self.installing
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    /// 自动检查开关即时保存；持久化失败时回滚并保留生效值。
    pub fn set_automatic_check(&mut self, enabled: bool) {
        if self.config.automatic_check == enabled {
            return;
        }
        self.config.automatic_check = enabled;
        if let Err(error) = self
            .store
            .set_setting("updates", &Self::stored_config(enabled))
            .map_err(|error| error.to_string())
        {
            self.config.automatic_check = !enabled;
            self.error = Some(error);
            return;
        }
        self.error = None;
        self.message = if enabled {
            swoop_i18n::tr!("已开启自动检查更新").into()
        } else {
            swoop_i18n::tr!("已关闭自动检查更新").into()
        };
    }
    pub fn check(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if let Err(error) = self.store.set_setting("update_last_check", &now.to_string()) {
            self.error = Some(error.to_string());
            return;
        }
        self.last_attempt = now;
        self.error = None;
        self.candidate = None;
        self.archive = None;
        self.message = swoop_i18n::tr!("正在检查更新…").into();
        self.cancel = CancellationToken::new();
        let cancel = self.cancel.clone();
        let config = self.config.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.handle.spawn(async move {
            let result = swoop_updater::check(&config, env!("CARGO_PKG_VERSION"), &cancel).await;
            let _ = tx.send(Event::Checked(result));
            ctx.request_repaint();
        });
    }
    pub fn download(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(candidate) = self.candidate.clone() else { return };
        self.error = None;
        self.message = swoop_i18n::tr!("正在下载并验证更新…").into();
        self.cancel = CancellationToken::new();
        let cancel = self.cancel.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.handle.spawn(async move {
            let result =
                swoop_updater::download(&candidate, &crate::instance::data_dir().join("updates"), &cancel).await;
            let _ = tx.send(Event::Downloaded(result));
            ctx.request_repaint();
        });
    }
    pub fn prepare_install(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(candidate), Some(archive)) = (self.candidate.clone(), self.archive.clone()) else { return };
        self.error = None;
        self.installing = true;
        self.message = swoop_i18n::tr!("正在准备更新…").into();
        self.cancel = CancellationToken::new();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        std::thread::spawn(move || {
            let result = std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| swoop_updater::prepare(&candidate, &archive, &exe, &crate::instance::data_dir()));
            let _ = tx.send(Event::Prepared(result));
            ctx.request_repaint();
        });
    }
    /// Returns true only after the user-approved installer has started waiting for app exit.
    pub fn poll(&mut self, ctx: &egui::Context, idle: bool, quitting: bool) -> bool {
        let event = self.receiver.as_ref().map(|rx| rx.try_recv());
        match event {
            Some(Ok(event)) => {
                self.receiver = None;
                match event {
                    Event::Checked(Ok(candidate)) => {
                        self.message = if candidate.is_some() {
                            swoop_i18n::tr!("发现新版本")
                        } else {
                            swoop_i18n::tr!("当前已是最新版本")
                        }
                        .into();
                        self.candidate = candidate;
                    }
                    Event::Downloaded(Ok(path)) => {
                        self.archive = Some(path);
                        self.message = swoop_i18n::tr!("更新包已验证，可安装并重启").into();
                    }
                    Event::Prepared(Ok((plan, helper, token))) if idle && !quitting && !self.cancel.is_cancelled() => {
                        let (tx, rx) = mpsc::channel();
                        self.receiver = Some(rx);
                        let cancel = self.cancel.clone();
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let result = launch_ready(&plan, &helper, &token, &cancel);
                            if let Err(mpsc::SendError(Event::Ready(Ok(mut child)))) = tx.send(Event::Ready(result)) {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            ctx.request_repaint();
                        });
                    }
                    Event::Ready(Ok(mut child)) if idle && !quitting && !self.cancel.is_cancelled() => {
                        // The helper has validated its inputs and is waiting on our instance lock.
                        if matches!(child.try_wait(), Ok(None)) {
                            drop(child);
                            self.installing = false;
                            return true;
                        }
                        let (tx, rx) = mpsc::channel();
                        self.receiver = Some(rx);
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ =
                                tx.send(Event::Ready(Err(swoop_i18n::tr!("更新助手已退出，请重新准备安装").into())));
                            ctx.request_repaint();
                        });
                    }
                    Event::Ready(Ok(mut child)) => {
                        // Keep the controller busy until the rejected helper has been reaped.
                        let (tx, rx) = mpsc::channel();
                        self.receiver = Some(rx);
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = tx.send(Event::Ready(Err(swoop_i18n::tr!("更新已取消").into())));
                            ctx.request_repaint();
                        });
                    }
                    Event::Prepared(Ok(_)) => {
                        self.installing = false;
                        self.error = Some(swoop_i18n::tr!("任务状态已改变，请结束下载后重新点击安装").into())
                    }
                    Event::Checked(Err(error))
                    | Event::Downloaded(Err(error))
                    | Event::Prepared(Err(error))
                    | Event::Ready(Err(error)) => {
                        self.installing = false;
                        self.error = Some(error);
                        self.message.clear();
                    }
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.receiver = None;
                self.installing = false;
                self.error = Some(swoop_i18n::tr!("更新操作意外结束").into());
            }
            _ => {}
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if !quitting
            && !self.busy()
            && self.config.automatic_check
            && now.saturating_sub(self.last_attempt)
                >= Duration::from_secs(u64::from(self.config.interval_hours) * 3600).as_secs()
        {
            self.check(ctx);
        }
        false
    }
}

fn launch_ready(
    plan: &std::path::Path,
    helper: &std::path::Path,
    token: &str,
    cancel: &CancellationToken,
) -> Result<std::process::Child, String> {
    let log_path = plan.with_file_name("installer.log");
    let ready = plan.with_file_name("helper-ready.json");
    if ready.try_exists().map_err(|e| e.to_string())? {
        return Err(swoop_i18n::tr!("更新助手确认文件已存在，请重新准备安装").into());
    }
    let log = std::fs::OpenOptions::new().write(true).create_new(true).open(&log_path).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(helper);
    command
        .arg("--swoop-install-update")
        .arg(plan)
        .arg(token)
        .arg(&ready)
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let result = (|| {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            if cancel.is_cancelled() {
                return Err(swoop_i18n::tr!("更新已取消").into());
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err(swoop_i18n::trf!("更新助手启动失败，请查看 {}", log_path.display()));
            }
            if let Ok(bytes) = std::fs::read(&ready)
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && value["token"].as_str() == Some(token)
            {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(swoop_i18n::trf!("等待更新助手超时，请查看 {}", log_path.display()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    match result {
        Ok(()) => Ok(child),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn helper_must_acknowledge_before_app_exit_and_failed_start_stays_visible() {
        use std::os::unix::fs::PermissionsExt;
        for acknowledges in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let helper = directory.path().join("test-helper");
            let script = if acknowledges {
                "#!/bin/sh\nprintf '{\"token\":\"%s\"}' \"$3\" > \"$4\"\nexec /bin/sleep 20\n"
            } else {
                "#!/bin/sh\nexit 7\n"
            };
            std::fs::write(&helper, script).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result =
                launch_ready(&directory.path().join("plan.json"), &helper, "fixture-token", &CancellationToken::new());
            if acknowledges {
                let mut child = result.unwrap();
                assert!(child.try_wait().unwrap().is_none());
                child.kill().unwrap();
                child.wait().unwrap();
            } else {
                assert!(result.is_err());
                assert!(directory.path().join("installer.log").exists());
            }
        }
    }
    #[test]
    #[cfg(unix)]
    fn delayed_ready_event_from_an_exited_helper_never_closes_the_app() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut updates = Updates::load(Arc::new(Store::in_memory().unwrap()), rt.handle().clone());
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        child.wait().unwrap();
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        updates.installing = true;
        tx.send(Event::Ready(Ok(child))).unwrap();
        assert!(!updates.poll(&egui::Context::default(), true, false));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while updates.busy() {
            assert!(std::time::Instant::now() < deadline);
            assert!(!updates.poll(&egui::Context::default(), true, false));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(updates.error.is_some());
        assert!(!updates.installing());
    }
    #[test]
    fn failed_persistence_keeps_active_config_and_never_starts_a_check() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        let store = Arc::new(Store::open(&path).unwrap());
        let mut updates = Updates::load(store.clone(), rt.handle().clone());
        rusqlite::Connection::open(&path).unwrap().execute_batch(
            "CREATE TRIGGER reject_updates BEFORE INSERT ON settings WHEN NEW.key IN ('updates', 'update_last_check') BEGIN SELECT RAISE(FAIL, 'test update setting failure'); END;"
        ).unwrap();
        updates.set_automatic_check(true);
        assert!(!updates.config.automatic_check);
        assert!(updates.error.is_some());
        updates.check(&egui::Context::default());
        assert!(!updates.busy());
        assert!(updates.error.is_some());
        assert_eq!(updates.last_attempt, 0);
        assert!(!Updates::load(store, rt.handle().clone()).config.automatic_check);
    }

    #[test]
    fn prepared_update_never_launches_after_cancel_new_work_or_quit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        for (idle, quitting, cancelled) in [(false, false, false), (true, true, false), (true, false, true)] {
            let mut updates = Updates::load(store.clone(), rt.handle().clone());
            let (tx, rx) = mpsc::channel();
            updates.receiver = Some(rx);
            if cancelled {
                updates.cancel();
            }
            tx.send(Event::Prepared(Ok((
                PathBuf::from("unused-plan"),
                PathBuf::from("unused-helper"),
                "unused-token".into(),
            ))))
            .unwrap();
            assert!(!updates.poll(&egui::Context::default(), idle, quitting));
            assert!(!updates.busy());
            assert!(updates.error.is_some());
        }
    }
    #[test]
    fn defaults_disable_unconfigured_network_and_toggle_persists() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let mut updates = Updates::load(store.clone(), rt.handle().clone());
        assert!(!updates.config.automatic_check);
        assert_eq!(updates.config.feed_url, swoop_updater::DEFAULT_FEED_URL);
        updates.set_automatic_check(true);
        assert!(Updates::load(store.clone(), rt.handle().clone()).config.automatic_check);
        // 历史版本存过的自定义源/公钥/通道/间隔在加载时清除，只保留开关。
        store
            .set_setting(
                "updates",
                r#"{"feed_url":"https://mirror.example/feed.json","public_key":"aa","channel":"Preview","automatic_check":false,"interval_hours":1}"#,
            )
            .unwrap();
        let migrated = Updates::load(store, rt.handle().clone());
        assert!(!migrated.config.automatic_check);
        assert_eq!(migrated.config.feed_url, swoop_updater::DEFAULT_FEED_URL);
        assert_eq!(migrated.config.public_key, swoop_updater::OFFICIAL_PUBLIC_KEY);
        assert_eq!(migrated.config.interval_hours, 168);
    }
}
