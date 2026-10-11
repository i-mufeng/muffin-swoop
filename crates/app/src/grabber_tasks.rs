use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use swoop_core::Store;
use swoop_grabber::{Grabber, GrabberConfig, ProjectSnapshot};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
pub enum ProjectAction {
    Create,
    CreateOnly,
    Open,
    Run,
    ImportSnapshot,
}

pub struct ProjectOperation {
    pub id: u64,
    pub kind: &'static str,
    pub state: &'static str,
    pub directory: PathBuf,
}

pub struct GrabberTasks {
    pub config: GrabberConfig,
    pub snapshot: Option<ProjectSnapshot>,
    pub recent: Vec<PathBuf>,
    pub error: Option<String>,
    pub domains_text: String,
    pub extensions_text: String,
    pub exclusions_text: String,
    pub snapshot_url: String,
    pub open: bool,
    pub operation: Option<ProjectOperation>,
    next_operation: u64,
    store: Arc<Store>,
    handle: Handle,
    pending: Option<mpsc::Receiver<Result<ProjectSnapshot, String>>>,
    latest: Arc<Mutex<Option<ProjectSnapshot>>>,
    cancel: CancellationToken,
}

impl GrabberTasks {
    pub fn load(store: Arc<Store>, handle: Handle) -> Self {
        let config: GrabberConfig = store
            .get_setting("grabber_config")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let recent = store
            .get_setting("grabber_recent")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            domains_text: config.allowed_domains.join(", "),
            extensions_text: config.resource_extensions.join(", "),
            exclusions_text: config.excluded_url_patterns.join("\n"),
            snapshot_url: config.start_url.clone(),
            config,
            snapshot: None,
            recent,
            error: None,
            open: false,
            operation: None,
            next_operation: 1,
            store,
            handle,
            pending: None,
            latest: Default::default(),
            cancel: CancellationToken::new(),
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn stop(&self) {
        self.cancel.cancel();
    }
    pub fn start(&mut self, action: ProjectAction, ctx: &egui::Context) {
        if let Err(error) = self.try_start(action, ctx) {
            self.error = Some(error);
        }
    }
    pub fn try_start(&mut self, action: ProjectAction, ctx: &egui::Context) -> Result<u64, String> {
        if self.busy() {
            return Err(swoop_i18n::tr!("已有项目操作正在运行").into());
        }
        let id = self.next_operation;
        let next = id.checked_add(1).ok_or(swoop_i18n::tr!("项目操作编号已满"))?;
        self.config.allowed_domains = split_list(&self.domains_text);
        self.config.resource_extensions = split_list(&self.extensions_text);
        self.config.excluded_url_patterns =
            self.exclusions_text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect();
        let snapshot_url = self.snapshot_url.clone();
        let config = self.config.clone();
        let directory = config.output_dir.clone();
        let cancel = CancellationToken::new();
        self.cancel = cancel.clone();
        let handle = self.handle.clone();
        let ctx = ctx.clone();
        let latest = self.latest.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("swoop-site-project".into()).spawn(move || {
            let work = || -> Result<ProjectSnapshot, String> {
                let mut grabber = match action {
                    ProjectAction::Create | ProjectAction::CreateOnly => Grabber::create(config.clone()),
                    ProjectAction::Open | ProjectAction::Run | ProjectAction::ImportSnapshot => Grabber::open(&config.output_dir),
                }.map_err(|e| e.to_string())?;
                if matches!(action, ProjectAction::Open | ProjectAction::CreateOnly) { return Ok(grabber.snapshot()); }
                if matches!(action, ProjectAction::Run | ProjectAction::ImportSnapshot) { grabber.update_config(config).map_err(|e| e.to_string())?; }
                if matches!(action, ProjectAction::ImportSnapshot) {
                    use std::io::Read;
                    let file = handle.block_on(rfd::AsyncFileDialog::new().set_title(swoop_i18n::tr!("导入已渲染 HTML")).add_filter(swoop_i18n::tr!("HTML 快照"), &["html", "htm"]).pick_file());
                    let Some(file) = file else { return Ok(grabber.snapshot()); };
                    if cancel.is_cancelled() { return Ok(grabber.snapshot()); }
                    let max = grabber.snapshot().config.max_file_bytes;
                    let mut bytes = Vec::new();
                    std::fs::File::open(file.path()).map_err(|error| error.to_string())?.take(max + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
                    if bytes.len() as u64 > max { return Err(swoop_i18n::tr!("HTML 快照超过单文件上限").into()); }
                    let html = String::from_utf8(bytes).map_err(|_| swoop_i18n::tr!("请导入 UTF-8 编码的 HTML 快照").to_owned())?;
                    grabber.import_rendered_html(&snapshot_url, &html).map_err(|error| error.to_string())?;
                }
                let mut updates = grabber.subscribe();
                *latest.lock().unwrap() = Some(grabber.snapshot());
                handle.block_on(async {
                    let task = grabber.run(cancel);
                    tokio::pin!(task);
                    loop {
                        tokio::select! {
                            result = &mut task => return result.map_err(|e| e.to_string()),
                            changed = updates.changed() => {
                                if changed.is_ok() { *latest.lock().unwrap() = Some(updates.borrow_and_update().clone()); ctx.request_repaint(); }
                            }
                        }
                    }
                })
            };
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => {
                self.pending = Some(rx);
                self.error = None;
                self.next_operation = next;
                self.operation = Some(ProjectOperation {
                    id,
                    kind: match action {
                        ProjectAction::Create => "create-and-run",
                        ProjectAction::CreateOnly => "create",
                        ProjectAction::Open => "open",
                        ProjectAction::Run => "run",
                        ProjectAction::ImportSnapshot => "import-dom",
                    },
                    state: "running",
                    directory,
                });
                if !matches!(action, ProjectAction::Run | ProjectAction::ImportSnapshot) {
                    self.snapshot = None;
                }
                Ok(id)
            }
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn control_status(&self) -> serde_json::Value {
        use serde_json::json;
        let project = self.snapshot.as_ref().map(|s| json!({
            "directory": s.config.output_dir, "status": match s.status {
                swoop_grabber::ProjectStatus::Ready => "ready", swoop_grabber::ProjectStatus::Running => "running",
                swoop_grabber::ProjectStatus::Complete => "complete", swoop_grabber::ProjectStatus::Cancelled => "cancelled",
                swoop_grabber::ProjectStatus::Limited => "limited",
            }, "pages": s.pages.len(), "downloaded": s.downloaded(), "failed": s.failed(),
            "received_bytes": s.received_bytes, "rendered_network_bytes": s.rendered_network_bytes, "revision": s.revision,
            "entry_path": s.entry_path, "mirror_dir": s.mirror_dir,
        }));
        let operation =
            self.operation.as_ref().map(|o| json!({"id":o.id,"kind":o.kind,"status":o.state,"directory":o.directory}));
        json!({"busy":self.busy(),"error":self.error.is_some(),"operation":operation,"project":project})
    }
    pub fn poll(&mut self) {
        if let Some(snapshot) = self.latest.lock().unwrap().take() {
            self.snapshot = Some(snapshot);
        }
        let result = self.pending.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(swoop_i18n::tr!("站点抓取线程中断").into())),
        });
        if let Some(result) = result {
            self.pending = None;
            // 完成通知之后后台不再写进度；清掉可能在本帧交错到达的旧 Running 快照。
            self.latest.lock().unwrap().take();
            match result {
                Ok(snapshot) => {
                    self.config = snapshot.config.clone();
                    self.domains_text = self.config.allowed_domains.join(", ");
                    self.extensions_text = self.config.resource_extensions.join(", ");
                    self.exclusions_text = self.config.excluded_url_patterns.join("\n");
                    if !snapshot.pages.iter().any(|page| page.url == self.snapshot_url) {
                        self.snapshot_url = self.config.start_url.clone();
                    }
                    self.recent.retain(|path| path != &self.config.output_dir);
                    self.recent.insert(0, self.config.output_dir.clone());
                    self.recent.truncate(20);
                    self.snapshot = Some(snapshot);
                    let save = || -> Result<(), String> {
                        self.store
                            .set_settings(&[
                                ("grabber_config", serde_json::to_string(&self.config).map_err(|e| e.to_string())?),
                                ("grabber_recent", serde_json::to_string(&self.recent).map_err(|e| e.to_string())?),
                            ])
                            .map_err(|e| e.to_string())
                    };
                    if let Err(error) = save() {
                        self.error = Some(swoop_i18n::trf!("保存项目设置失败：{error}"));
                    }
                }
                Err(error) => {
                    if let Some(snapshot) = &mut self.snapshot {
                        snapshot.status = swoop_grabber::ProjectStatus::Ready;
                    }
                    self.error = Some(error);
                }
            }
            if let Some(operation) = &mut self.operation {
                operation.state = if self.error.is_some() {
                    "failed"
                } else if matches!(operation.kind, "run" | "create-and-run" | "import-dom")
                    && self.snapshot.as_ref().is_some_and(|s| s.status == swoop_grabber::ProjectStatus::Cancelled)
                {
                    "cancelled"
                } else {
                    "succeeded"
                };
            }
        }
    }
}
impl Drop for GrabberTasks {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split([',', ';', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}
