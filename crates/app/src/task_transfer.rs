//! 任务选择、稳定排序与后台批量操作；移除始终保留文件。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use serde::{Deserialize, Serialize};
use swoop_core::{Engine, Store, TaskId, TaskSnapshot, TaskStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListPreferences {
    pub compact: bool,
    pub progress_bar: bool,
    pub size: bool,
    pub percent: bool,
    pub connections: bool,
    pub remaining_time: bool,
    pub directory: bool,
    pub host: bool,
    pub speed: bool,
    pub toolbar_actions: bool,
}
impl Default for ListPreferences {
    fn default() -> Self {
        Self {
            compact: false,
            progress_bar: true,
            size: true,
            percent: true,
            connections: true,
            remaining_time: true,
            directory: true,
            host: false,
            speed: true,
            toolbar_actions: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortField {
    Name,
    #[default]
    Created,
    Size,
    Status,
}

impl SortField {
    pub const ALL: [Self; 4] = [Self::Name, Self::Created, Self::Size, Self::Status];
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => swoop_i18n::tr!("名称"),
            Self::Created => swoop_i18n::tr!("添加时间"),
            Self::Size => swoop_i18n::tr!("大小"),
            Self::Status => swoop_i18n::tr!("状态"),
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Created => "created",
            Self::Size => "size",
            Self::Status => "status",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.key() == value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortOrder {
    pub field: SortField,
    pub descending: bool,
}
impl Default for SortOrder {
    fn default() -> Self {
        Self { field: SortField::Created, descending: true }
    }
}

impl SortOrder {
    pub fn sort(self, tasks: &mut [TaskSnapshot]) {
        tasks.sort_by(|a, b| {
            let order = match self.field {
                SortField::Name => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                SortField::Created => a.created_at.cmp(&b.created_at),
                SortField::Size => match (a.total, b.total) {
                    (None, Some(_)) => return std::cmp::Ordering::Greater,
                    (Some(_), None) => return std::cmp::Ordering::Less,
                    _ => a.total.cmp(&b.total),
                },
                SortField::Status => status_rank(&a.status).cmp(&status_rank(&b.status)),
            };
            (if self.descending { order.reverse() } else { order }).then(a.id.cmp(&b.id))
        });
    }
}

fn status_rank(status: &TaskStatus) -> u8 {
    match status {
        TaskStatus::Downloading => 0,
        TaskStatus::Probing => 1,
        TaskStatus::Queued => 2,
        TaskStatus::FileConflict => 3,
        TaskStatus::Paused => 4,
        TaskStatus::Failed(_) => 5,
        TaskStatus::Completed => 6,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkAction {
    Pause,
    Resume,
    Remove,
}
impl BulkAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pause => swoop_i18n::tr!("暂停"),
            Self::Resume => swoop_i18n::tr!("继续"),
            Self::Remove => swoop_i18n::tr!("移除记录"),
        }
    }
    pub fn accepts(self, status: &TaskStatus) -> bool {
        match self {
            Self::Pause => status.is_active(),
            Self::Resume => matches!(status, TaskStatus::Paused | TaskStatus::Failed(_)),
            Self::Remove => true,
        }
    }
}

#[derive(Debug, Default)]
pub struct BulkReport {
    pub succeeded: Vec<TaskId>,
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub enum TransferEvent {
    Bulk(BulkAction, BulkReport),
    Exported { ids: Vec<TaskId>, path: PathBuf, count: usize },
    Cancelled,
    Error(String),
}

pub struct TaskTransfer {
    pub selected: HashSet<TaskId>,
    pub sort: SortOrder,
    pub view: ListPreferences,
    pub layout: crate::list_layout::LayoutPreferences,
    pub table_revision: Option<u64>,
    pending: Option<mpsc::Receiver<TransferEvent>>,
}

impl TaskTransfer {
    pub fn load(store: &Store) -> Self {
        let sort = store
            .get_setting("task_sort")
            .ok()
            .flatten()
            .and_then(|value| {
                let (field, direction) = value.split_once(':')?;
                Some(SortOrder {
                    field: SortField::parse(field)?,
                    descending: match direction {
                        "desc" => true,
                        "asc" => false,
                        _ => return None,
                    },
                })
            })
            .unwrap_or_default();
        let view = store
            .get_setting("task_list_view")
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        Self {
            selected: HashSet::new(),
            sort,
            view,
            layout: crate::list_layout::LayoutPreferences::load(store, view),
            table_revision: None,
            pending: None,
        }
    }

    /// 保存失败时界面保留旧排序，避免把未持久化状态当作已保存。
    pub fn set_sort(&mut self, store: &Store, sort: SortOrder) -> Result<(), String> {
        store
            .set_setting("task_sort", &format!("{}:{}", sort.field.key(), if sort.descending { "desc" } else { "asc" }))
            .map_err(|e| e.to_string())?;
        self.sort = sort;
        Ok(())
    }

    pub fn set_view(&mut self, store: &Store, view: ListPreferences) -> Result<(), String> {
        store
            .set_setting("task_list_view", &serde_json::to_string(&view).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        self.view = view;
        Ok(())
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn select_visible(&mut self, tasks: &[TaskSnapshot], selected: bool) {
        for task in tasks {
            if selected {
                self.selected.insert(task.id);
            } else {
                self.selected.remove(&task.id);
            }
        }
    }
    pub fn retain_visible(&mut self, tasks: &[TaskSnapshot]) {
        let visible: HashSet<_> = tasks.iter().map(|task| task.id).collect();
        self.selected.retain(|id| visible.contains(id));
    }
    pub fn chosen(&self, tasks: &[TaskSnapshot]) -> Vec<TaskSnapshot> {
        let mut chosen: Vec<_> = tasks.iter().filter(|task| self.selected.contains(&task.id)).cloned().collect();
        self.sort.sort(&mut chosen);
        chosen
    }

    fn spawn(
        &mut self,
        ctx: egui::Context,
        work: impl FnOnce() -> TransferEvent + Send + 'static,
    ) -> Result<(), String> {
        if self.busy() {
            return Err(swoop_i18n::tr!("请等待当前批量操作完成").into());
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("swoop-task-transfer".into())
            .spawn(move || {
                let _ = tx.send(work());
                ctx.request_repaint();
            })
            .map_err(|error| error.to_string())?;
        self.pending = Some(rx);
        Ok(())
    }

    pub fn start_bulk(
        &mut self,
        ctx: egui::Context,
        engine: Arc<Engine>,
        action: BulkAction,
        ids: Vec<TaskId>,
    ) -> Result<(), String> {
        self.spawn(ctx, move || TransferEvent::Bulk(action, apply_bulk(&engine, action, &ids)))
    }

    pub fn start_export(
        &mut self,
        ctx: egui::Context,
        tasks: Vec<TaskSnapshot>,
        protected: Vec<PathBuf>,
    ) -> Result<(), String> {
        let ids = tasks.iter().map(|task| task.id).collect();
        let text = urls_text(&tasks);
        let count = tasks.len();
        self.spawn(ctx, move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => return TransferEvent::Error(error.to_string()),
            };
            let file = runtime.block_on(
                rfd::AsyncFileDialog::new()
                    .set_title(swoop_i18n::tr!("导出所选下载链接"))
                    .add_filter(swoop_i18n::tr!("链接文本"), &["txt"])
                    .set_file_name("swoop-links.txt")
                    .save_file(),
            );
            let Some(file) = file else {
                return TransferEvent::Cancelled;
            };
            match write_export(file.path(), &text, &protected) {
                Ok(()) => TransferEvent::Exported { ids, path: file.path().into(), count },
                Err(error) => TransferEvent::Error(error),
            }
        })
    }

    pub fn poll(&mut self) -> Option<TransferEvent> {
        let event = match self.pending.as_ref()?.try_recv() {
            Ok(event) => event,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                TransferEvent::Error(swoop_i18n::tr!("后台任务意外中断，请检查当前状态后重试").into())
            }
        };
        self.pending = None;
        Some(event)
    }
}

pub fn urls_text(tasks: &[TaskSnapshot]) -> String {
    let mut text = tasks.iter().map(|task| task.url.as_str()).collect::<Vec<_>>().join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

fn apply_bulk(engine: &Engine, action: BulkAction, ids: &[TaskId]) -> BulkReport {
    let mut report = BulkReport::default();
    let mut seen = HashSet::new();
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        let Some(task) = engine.snapshot_one(id) else {
            report.errors.push(swoop_i18n::trf!("任务 {} 已不存在", id.0));
            continue;
        };
        if !action.accepts(&task.status) {
            report.skipped += 1;
            continue;
        }
        let result = match action {
            BulkAction::Pause => engine.pause(id),
            BulkAction::Resume => engine.resume(id),
            BulkAction::Remove => engine.remove(id, !crate::ui::task_list::keeps_file(&task)),
        };
        match result {
            Ok(()) => report.succeeded.push(id),
            Err(error) => report.errors.push(format!("{}：{error}", task.file_name)),
        }
    }
    report
}

fn path_identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.into());
        path.parent()
            .and_then(|parent| parent.canonicalize().ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or(path)
    })
}

fn write_export(path: &Path, text: &str, protected: &[PathBuf]) -> Result<(), String> {
    use std::io::Write;
    let identity = path_identity(path);
    if protected.iter().any(|path| path_identity(path) == identity) {
        return Err(swoop_i18n::tr!("该路径属于下载任务，请选择其他文件名").into());
    }
    // 不覆盖任何现有文件，即使用户在保存对话框中确认替换。
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            swoop_i18n::tr!("目标文件已存在，请使用新文件名").into()
        } else {
            error.to_string()
        }
    })?;
    file.write_all(text.as_bytes()).and_then(|_| file.sync_all()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};
    fn task(id: u64, name: &str, total: Option<u64>, status: TaskStatus) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId(id),
            url: format!("https://example.com/{id}"),
            file_name: name.into(),
            path: name.into(),
            total,
            downloaded: 0,
            speed_bps: 0.0,
            status,
            resumable: false,
            segments: vec![],
            active_connections: 0,
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(id),
            completed_at: None,
        }
    }
    #[test]
    fn select_all_is_limited_to_visible_and_filter_changes_drop_hidden_choices() {
        let store = Store::in_memory().unwrap();
        let mut state = TaskTransfer::load(&store);
        let tasks = [task(1, "a", None, TaskStatus::Paused), task(2, "b", None, TaskStatus::Completed)];
        state.select_visible(&tasks, true);
        state.retain_visible(&tasks[1..]);
        assert_eq!(state.selected, HashSet::from([TaskId(2)]));
        state.select_visible(&tasks[1..], false);
        assert!(state.selected.is_empty());
    }
    #[test]
    fn sort_is_deterministic_and_unknown_sizes_stay_last_both_directions() {
        let tasks = vec![
            task(3, "beta", None, TaskStatus::Completed),
            task(2, "Alpha", Some(20), TaskStatus::Paused),
            task(1, "alpha", Some(10), TaskStatus::Downloading),
        ];
        let mut sorted = tasks.clone();
        SortOrder { field: SortField::Name, descending: false }.sort(&mut sorted);
        assert_eq!(sorted.iter().map(|t| t.id.0).collect::<Vec<_>>(), [1, 2, 3]);
        SortOrder { field: SortField::Size, descending: true }.sort(&mut sorted);
        assert_eq!(sorted.iter().map(|t| t.id.0).collect::<Vec<_>>(), [2, 1, 3]);
        SortOrder { field: SortField::Size, descending: false }.sort(&mut sorted);
        assert_eq!(sorted.iter().map(|t| t.id.0).collect::<Vec<_>>(), [1, 2, 3]);
        SortOrder { field: SortField::Status, descending: false }.sort(&mut sorted);
        assert_eq!(sorted.iter().map(|t| t.id.0).collect::<Vec<_>>(), [1, 2, 3]);
        SortOrder::default().sort(&mut sorted);
        assert_eq!(sorted.iter().map(|t| t.id.0).collect::<Vec<_>>(), [3, 2, 1]);
    }
    #[test]
    fn sort_preferences_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.db");
        let store = Store::open(&path).unwrap();
        let sort = SortOrder { field: SortField::Size, descending: false };
        let mut transfer = TaskTransfer::load(&store);
        assert_eq!(transfer.view, ListPreferences::default());
        transfer.set_sort(&store, sort).unwrap();
        let view = ListPreferences {
            compact: true,
            host: true,
            directory: false,
            toolbar_actions: false,
            ..Default::default()
        };
        transfer.set_view(&store, view).unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let restored = TaskTransfer::load(&reopened);
        assert_eq!(restored.sort, sort);
        assert_eq!(restored.view, view);
        reopened.set_setting("task_list_view", r#"{"compact":true}"#).unwrap();
        assert_eq!(TaskTransfer::load(&reopened).view, ListPreferences { compact: true, ..Default::default() });
    }
    #[test]
    fn exports_only_selected_urls_and_never_overwrites_downloads_or_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("links.txt");
        let text = urls_text(&[task(1, "secret-name", None, TaskStatus::Paused)]);
        assert_eq!(text, "https://example.com/1\n");
        write_export(&target, &text, &[]).unwrap();
        assert!(write_export(&target, "overwrite", &[]).unwrap_err().contains("已存在"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
        let download = dir.path().join("pending.zip");
        assert!(
            write_export(&download, "overwrite", std::slice::from_ref(&download)).unwrap_err().contains("下载任务")
        );
        assert!(!download.exists());
    }
    #[test]
    fn bulk_remove_preserves_file_and_reports_missing_task() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let engine = Engine::with_store(rt.handle().clone(), swoop_core::Config::default(), store);
        engine.stop_queue(swoop_core::MAIN_QUEUE_ID).unwrap();
        let id = engine
            .add(swoop_core::AddOptions {
                url: "http://127.0.0.1:9/file.zip".into(),
                dir: dir.path().into(),
                file_name: Some("file.zip".into()),
                headers: vec![],
            })
            .unwrap();
        engine.pause(id).unwrap();
        let path = engine.snapshot_one(id).unwrap().path;
        std::fs::write(&path, b"keep bytes").unwrap();
        let report = apply_bulk(&engine, BulkAction::Remove, &[id, id, TaskId(u64::MAX)]);
        assert_eq!(report.succeeded, [id]);
        assert_eq!(report.errors.len(), 1);
        assert!(engine.snapshot_one(id).is_none());
        assert_eq!(std::fs::read(path).unwrap(), b"keep bytes");
    }
    #[test]
    fn batch_pause_resume_reports_only_actual_transitions() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(rt.handle().clone(), swoop_core::Config::default());
        engine.stop_queue(swoop_core::MAIN_QUEUE_ID).unwrap();
        let id = engine
            .add(swoop_core::AddOptions {
                url: "http://127.0.0.1:9/file.zip".into(),
                dir: dir.path().into(),
                file_name: None,
                headers: vec![],
            })
            .unwrap();
        assert_eq!(apply_bulk(&engine, BulkAction::Pause, &[id]).succeeded, [id]);
        assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Paused);
        let repeat = apply_bulk(&engine, BulkAction::Pause, &[id]);
        assert_eq!(repeat.skipped, 1);
        assert!(repeat.succeeded.is_empty());
        assert_eq!(apply_bulk(&engine, BulkAction::Resume, &[id]).succeeded, [id]);
        assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Queued);
    }
}
