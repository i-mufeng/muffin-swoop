//! Native drag-and-drop. Incoming content only becomes a confirmation draft.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use swoop_core::{Store, TaskSnapshot, TaskStatus};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DragConfig {
    pub allow_drop_in: bool,
    pub allow_drag_out: bool,
}
impl Default for DragConfig {
    fn default() -> Self {
        Self { allow_drop_in: true, allow_drag_out: true }
    }
}
#[derive(Debug)]
pub enum DropPayload {
    Urls(String),
    Files(Vec<PathBuf>),
}
struct Shared {
    enabled: AtomicBool,
    drops: Mutex<VecDeque<DropPayload>>,
    context: egui::Context,
}
impl Shared {
    fn push(&self, payload: DropPayload) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut drops) = self.drops.lock()
            && drops.len() < 16
        {
            drops.push_back(payload);
        }
        self.context.request_repaint();
    }
}
pub struct NativeDrag {
    pub config: DragConfig,
    pending: Option<Vec<PathBuf>>,
    shared: Option<Arc<Shared>>,
    initialized: bool,
    error: Option<String>,
    #[cfg(target_os = "macos")]
    guard: Option<macos::Guard>,
    #[cfg(windows)]
    guard: Option<windows::Guard>,
}
impl NativeDrag {
    pub fn load(store: &Store) -> Self {
        let config = store
            .get_setting("native_drag_config")
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            config,
            pending: None,
            shared: None,
            initialized: false,
            error: None,
            #[cfg(any(target_os = "macos", windows))]
            guard: None,
        }
    }
    pub fn settings_ui(&mut self, ui: &mut egui::Ui, store: &Store) {
        let mut config = self.config.clone();
        crate::ui::checkbox(ui, &mut config.allow_drop_in, swoop_i18n::tr!("拖入链接和链接文件"));
        crate::ui::checkbox(ui, &mut config.allow_drag_out, swoop_i18n::tr!("将已完成文件拖到其他应用"));
        if config != self.config {
            match serde_json::to_string(&config)
                .map_err(|error| error.to_string())
                .and_then(|text| store.set_setting("native_drag_config", &text).map_err(|error| error.to_string()))
            {
                Ok(()) => self.config = config,
                Err(error) => self.error = Some(swoop_i18n::trf!("拖放设置未保存：{error}")),
            }
        }
    }
    pub fn queue_files(&mut self, tasks: &[TaskSnapshot]) -> Result<(), String> {
        if !self.config.allow_drag_out {
            return Err(swoop_i18n::tr!("拖出文件已关闭").into());
        }
        self.pending = Some(completed_files(tasks)?);
        if let Some(shared) = &self.shared {
            shared.context.request_repaint();
        }
        Ok(())
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
    pub fn update(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> Vec<DropPayload> {
        if !self.initialized {
            self.initialized = true;
            let shared = Arc::new(Shared {
                enabled: AtomicBool::new(self.config.allow_drop_in),
                drops: Mutex::new(VecDeque::new()),
                context: ctx.clone(),
            });
            #[cfg(target_os = "macos")]
            match macos::install(frame, shared.clone()) {
                Ok(guard) => self.guard = Some(guard),
                Err(error) => self.error = Some(swoop_i18n::trf!("原生链接拖入不可用：{error}")),
            }
            #[cfg(windows)]
            match windows::install(frame, shared.clone()) {
                Ok(guard) => self.guard = Some(guard),
                Err(error) => self.error = Some(swoop_i18n::trf!("原生拖入不可用：{error}")),
            }
            self.shared = Some(shared);
        }
        let shared = self.shared.as_ref().expect("initialized drag state");
        shared.enabled.store(self.config.allow_drop_in, Ordering::Release);
        if let Some(files) = self.pending.take()
            && self.config.allow_drag_out
        {
            // Revalidate just before handing paths to the OS; sources are never removed.
            let valid = files.iter().all(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
            });
            if !valid {
                self.error = Some(swoop_i18n::tr!("拖出文件已不存在或不再是普通文件").into());
            } else {
                #[cfg(any(target_os = "macos", windows))]
                if let Err(error) = drag::start_drag(
                    frame,
                    drag::DragItem::Files(files),
                    drag::Image::Raw(include_bytes!("../../../extension/public/icons/icon-32.png").to_vec()),
                    |_, _| {},
                    drag::Options { mode: drag::DragMode::Copy, ..Default::default() },
                ) {
                    self.error = Some(swoop_i18n::trf!("拖出失败：{error}"));
                }
                #[cfg(not(any(target_os = "macos", windows)))]
                {
                    let _ = (frame, files);
                    self.error = Some(swoop_i18n::tr!("当前平台不支持原生拖出").into());
                }
            }
        }
        let mut result = shared.drops.lock().map(|mut queue| queue.drain(..).collect::<Vec<_>>()).unwrap_or_default();
        let files =
            ctx.input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect::<Vec<_>>());
        if !files.is_empty() {
            result.push(DropPayload::Files(files));
        }
        if !self.config.allow_drop_in {
            result.clear();
        }
        result
    }
}
fn completed_files(tasks: &[TaskSnapshot]) -> Result<Vec<PathBuf>, String> {
    if tasks.is_empty() || tasks.len() > 1000 {
        return Err(swoop_i18n::tr!("请选择 1–1000 个已完成文件").into());
    }
    let mut paths = Vec::new();
    for task in tasks {
        if task.status != TaskStatus::Completed {
            return Err(swoop_i18n::tr!("只能拖出已完成文件，请调整选择").into());
        }
        let metadata = std::fs::symlink_metadata(&task.path).map_err(|_| swoop_i18n::tr!("文件已不存在").to_owned())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(swoop_i18n::tr!("只能拖出普通文件").into());
        }
        let path = task.path.canonicalize().map_err(|error| error.to_string())?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}
fn drop_urls(text: &str) -> Option<String> {
    if text.len() > crate::link_import::MAX_IMPORT_BYTES {
        return None;
    }
    // Native URL drops are literal URLs, not the batch range expansion language.
    let mut urls = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        if urls.len() >= 1000 {
            return None;
        }
        let url = url::Url::parse(line).ok()?;
        if !matches!(url.scheme(), "http" | "https" | "ftp" | "ftps")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let literal = url.to_string().replace('{', "%7B").replace('}', "%7D");
        if !urls.contains(&literal) {
            urls.push(literal);
        }
    }
    (!urls.is_empty()).then(|| urls.join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn task(path: &std::path::Path, status: TaskStatus) -> TaskSnapshot {
        TaskSnapshot {
            id: swoop_core::TaskId(1),
            url: "https://example.test/file".into(),
            file_name: "file".into(),
            path: path.into(),
            total: Some(1),
            downloaded: 1,
            speed_bps: 0.0,
            status,
            resumable: false,
            segments: vec![],
            active_connections: 0,
            created_at: std::time::SystemTime::UNIX_EPOCH,
            completed_at: None,
        }
    }
    #[test]
    fn drag_out_requires_completed_regular_files_and_never_removes_sources() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("complete.bin");
        std::fs::write(&file, b"original").unwrap();
        let completed = task(&file, TaskStatus::Completed);
        assert_eq!(completed_files(&[completed.clone(), completed]).unwrap(), vec![file.canonicalize().unwrap()]);
        assert!(completed_files(&[task(&file, TaskStatus::Paused)]).is_err());
        assert!(completed_files(&[task(root.path(), TaskStatus::Completed)]).is_err());
        assert!(completed_files(&[task(&root.path().join("missing"), TaskStatus::Completed)]).is_err());
        #[cfg(unix)]
        {
            let link = root.path().join("alias");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(completed_files(&[task(&link, TaskStatus::Completed)]).is_err());
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
    }
    #[test]
    fn native_urls_are_literal_bounded_and_do_not_import_credentials() {
        assert_eq!(
            drop_urls("# comment\r\nhttps://example.test/file\r\nftp://example.test/archive.zip"),
            Some("https://example.test/file\nftp://example.test/archive.zip".into())
        );
        for bad in ["file:///tmp/a", "javascript:alert(1)", "https://u:password@example.test/file", "not a URL"] {
            assert!(drop_urls(bad).is_none());
        }
        assert!(drop_urls(&"x".repeat(crate::link_import::MAX_IMPORT_BYTES + 1)).is_none());
    }
}
