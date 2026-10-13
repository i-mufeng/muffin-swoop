//! 媒体任务、限速与持久化；重试可复用经验证的完整或部分分片。
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use swoop_core::{FileConflictPolicy, Store, TargetLease, TargetRegistry, TaskStatus};
use swoop_media::{
    AudioSelection, MediaConfig, MediaDownloader, MediaError, MediaEvent, MediaInfo, MediaNetworkConfig, MediaRequest,
    MediaSelection, MediaSessionAuth, MediaSource, VideoSelection,
};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaPreferences {
    pub ffmpeg: PathBuf,
    pub concurrency: usize,
    pub timeout_secs: u64,
    pub max_manifest_kib: usize,
    pub max_playlists: usize,
    pub default_max_height: Option<u32>,
    pub preferred_audio_language: String,
    pub network_mode: MediaNetworkMode,
    pub network: MediaNetworkConfig,
    pub mms_transport: swoop_media::MmsTransport,
    pub live_finalize_grace_secs: u64,
    pub default_recording_secs: u64,
    pub default_speed_limit_bps: u64,
    pub subtitles: swoop_media::SubtitleConfig,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaNetworkMode {
    #[default]
    Direct,
    Global,
    Manual,
}
impl Default for MediaPreferences {
    fn default() -> Self {
        #[allow(unused_mut)]
        let mut config = MediaConfig::default();
        #[cfg(target_os = "macos")]
        if let Some(path) = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"]
            .into_iter()
            .find(|p| std::path::Path::new(p).is_file())
        {
            config.ffmpeg_path = path.into();
        }
        Self {
            ffmpeg: config.ffmpeg_path,
            concurrency: config.max_concurrent,
            timeout_secs: config.network_timeout.as_secs(),
            max_manifest_kib: config.max_manifest_bytes / 1024,
            max_playlists: config.max_playlists,
            default_max_height: None,
            preferred_audio_language: String::new(),
            network_mode: MediaNetworkMode::Direct,
            network: MediaNetworkConfig::default(),
            mms_transport: swoop_media::MmsTransport::default(),
            live_finalize_grace_secs: 3,
            default_recording_secs: 3600,
            default_speed_limit_bps: 0,
            subtitles: Default::default(),
        }
    }
}
impl MediaPreferences {
    pub fn selection(&self) -> MediaSelection {
        MediaSelection {
            video: self.default_max_height.map(VideoSelection::MaxHeight).unwrap_or_default(),
            audio: if self.preferred_audio_language.trim().is_empty() {
                AudioSelection::Auto
            } else {
                AudioSelection::PreferredLanguage(self.preferred_audio_language.trim().to_owned())
            },
            subtitle: Default::default(),
        }
    }
    fn config(&self) -> MediaConfig {
        MediaConfig {
            ffmpeg_path: self.ffmpeg.clone(),
            max_concurrent: self.concurrency,
            network_timeout: Duration::from_secs(self.timeout_secs),
            max_manifest_bytes: self.max_manifest_kib.saturating_mul(1024),
            max_playlists: self.max_playlists,
            mms_transport: self.mms_transport,
            live_finalize_grace_secs: self.live_finalize_grace_secs,
            subtitles: self.subtitles.clone(),
        }
    }
    fn network_config(&self, global: &swoop_core::NetworkPolicy) -> Result<Option<MediaNetworkConfig>, String> {
        if !(1..=86400).contains(&self.default_recording_secs) {
            return Err(swoop_i18n::tr!("默认录制时长须在 1–86400 秒之间").into());
        }
        self.network.validate().map_err(|error| error.to_string())?;
        let network = match self.network_mode {
            MediaNetworkMode::Direct => {
                MediaNetworkConfig { proxy: None, proxy_auth: None, use_system_proxy: false, ..self.network.clone() }
            }
            MediaNetworkMode::Manual => {
                if self.network.proxy.is_none() {
                    return Err(swoop_i18n::tr!("请填写媒体代理地址").into());
                }
                let mut network = self.network.clone();
                network.use_system_proxy = false;
                network
            }
            MediaNetworkMode::Global => MediaNetworkConfig {
                proxy: global.proxy.as_ref().map(|proxy| swoop_media::MediaProxyConfig {
                    url: proxy.url.clone(),
                    bypass: proxy.bypass.clone(),
                }),
                proxy_auth: global.proxy_auth.clone(),
                use_system_proxy: global.use_system_proxy,
                connect_timeout_secs: global.connect_timeout_secs,
                read_timeout_secs: global.read_timeout_secs,
                ..self.network.clone()
            },
        };
        network.validate().map_err(|error| error.to_string())?;
        Ok(Some(network))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaStatus {
    Waiting,
    FileConflict,
    NeedsAuth,
    Downloading,
    Cancelling,
    Paused,
    Completed,
    Failed(String),
}
impl MediaStatus {
    pub fn active(&self) -> bool {
        matches!(self, Self::Waiting | Self::Downloading | Self::Cancelling)
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Waiting => swoop_i18n::tr!("等待"),
            Self::FileConflict => swoop_i18n::tr!("等待确认"),
            Self::NeedsAuth => swoop_i18n::tr!("需要重新登录"),
            Self::Downloading => swoop_i18n::tr!("下载中"),
            Self::Cancelling => swoop_i18n::tr!("停止中"),
            Self::Paused => swoop_i18n::tr!("已停止"),
            Self::Completed => swoop_i18n::tr!("已完成"),
            Self::Failed(_) => swoop_i18n::tr!("失败"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaJob {
    pub id: u64,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub destination: PathBuf,
    pub duration_secs: Option<u64>,
    pub status: MediaStatus,
    pub bytes: u64,
    #[serde(default)]
    pub media_seconds: u64,
    #[serde(default)]
    pub selection: MediaSelection,
    #[serde(default)]
    pub auth_required: bool,
    #[serde(default)]
    pub speed_limit_bps: u64,
}
#[derive(Serialize, Deserialize, Default)]
struct History {
    next_id: u64,
    jobs: Vec<MediaJob>,
}

enum Update {
    Progress(u64, MediaEvent),
    Finished(u64, Result<u64, MediaError>, bool),
}
struct DownloadPlan {
    destination: PathBuf,
    status: MediaStatus,
    overwrite: bool,
    lease: Option<TargetLease>,
}
impl DownloadPlan {
    fn new(destination: PathBuf, status: MediaStatus, overwrite: bool, lease: Option<TargetLease>) -> Self {
        Self { destination, status, overwrite, lease }
    }
}
pub struct MediaTasks {
    pub preferences: MediaPreferences,
    pub jobs: Vec<MediaJob>,
    pub persistence_error: bool,
    pub file_conflict_policy: FileConflictPolicy,
    http_targets: Vec<PathBuf>,
    registry: TargetRegistry,
    next_id: u64,
    handle: Handle,
    store: Arc<Store>,
    downloader: MediaDownloader,
    network_snapshot: Option<MediaNetworkConfig>,
    network_ready: bool,
    preferences_load_error: Option<String>,
    global_network: swoop_core::NetworkPolicy,
    global_proxy_credentials: Option<swoop_core::ProxyCredentials>,
    manual_proxy_credentials: Option<swoop_core::ProxyCredentials>,
    proxy_snapshot_scope: Option<String>,
    pub network_error: Option<String>,
    session_auth: std::collections::HashMap<u64, MediaSessionAuth>,
    running: std::collections::HashMap<u64, CancellationToken>,
    running_downloaders: std::collections::HashMap<u64, MediaDownloader>,
    tx: mpsc::Sender<Update>,
    rx: mpsc::Receiver<Update>,
}

impl MediaTasks {
    pub fn load(handle: Handle, store: Arc<Store>, registry: TargetRegistry) -> Self {
        let global_network = crate::network_settings::load(&store);
        let (preferences, preferences_load_error): (MediaPreferences, Option<String>) = match store
            .get_setting("media_preferences")
        {
            Ok(Some(raw)) => match serde_json::from_str(&raw) {
                Ok(preferences) => (preferences, None),
                Err(_) => (Default::default(), Some(swoop_i18n::tr!("媒体设置无法读取，请重新保存媒体设置").into())),
            },
            Ok(None) => (Default::default(), None),
            Err(_) => (Default::default(), Some(swoop_i18n::tr!("媒体设置读取失败，请重新保存媒体设置").into())),
        };
        let mut history: History = store
            .get_setting("media_history")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        for job in &mut history.jobs {
            if job.auth_required && !matches!(job.status, MediaStatus::Completed | MediaStatus::Failed(_)) {
                job.status = MediaStatus::NeedsAuth;
            } else if job.status.active() {
                job.status = MediaStatus::Paused;
            }
        }
        let next_id =
            history.next_id.max(history.jobs.iter().map(|j| j.id).max().unwrap_or(0).saturating_add(1)).max(1);
        let file_conflict_policy = store
            .get_setting("file_conflict_policy")
            .ok()
            .flatten()
            .as_deref()
            .and_then(FileConflictPolicy::parse)
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let (downloader, network_snapshot, network_error) =
            match Self::configured_downloader(&preferences, &global_network, None) {
                Ok((downloader, network)) => (downloader, network, None),
                Err(error) => (
                    // Inert fallback only satisfies struct initialization;
                    // network_ready/error block every inspect/add/restart.
                    MediaDownloader::new(MediaConfig::default()).expect("default media config"),
                    None,
                    Some(error),
                ),
            };
        let network_error = preferences_load_error.clone().or(network_error);
        Self {
            downloader,
            network_ready: network_error.is_none(),
            preferences_load_error,
            network_snapshot,
            global_network,
            global_proxy_credentials: None,
            manual_proxy_credentials: None,
            proxy_snapshot_scope: None,
            network_error,
            session_auth: Default::default(),
            preferences,
            jobs: history.jobs,
            persistence_error: false,
            file_conflict_policy,
            http_targets: Vec::new(),
            registry,
            next_id,
            handle,
            store,
            running: Default::default(),
            running_downloaders: Default::default(),
            tx,
            rx,
        }
    }
    fn configured_downloader(
        preferences: &MediaPreferences,
        global: &swoop_core::NetworkPolicy,
        credentials: Option<swoop_core::ProxyCredentials>,
    ) -> Result<(MediaDownloader, Option<MediaNetworkConfig>), String> {
        let network = preferences.network_config(global)?;
        let downloader = match &network {
            Some(network) => {
                MediaDownloader::new_with_network_and_proxy(preferences.config(), network.clone(), credentials)
            }
            None => MediaDownloader::new(preferences.config()),
        }
        .map_err(|error| error.to_string())?;
        Ok((downloader, network))
    }
    fn persist(&self) -> Result<(), String> {
        let json = serde_json::to_string(&History { next_id: self.next_id, jobs: self.jobs.clone() })
            .map_err(|e| e.to_string())?;
        self.store.set_setting("media_history", &json).map_err(|e| e.to_string())
    }
    pub fn active(&self) -> bool {
        !self.running.is_empty()
    }
    pub fn set_preferences(&mut self, preferences: MediaPreferences) -> Result<(), String> {
        self.set_preferences_with_proxy(preferences, None)
    }
    pub fn set_preferences_with_proxy(
        &mut self,
        mut preferences: MediaPreferences,
        supplied_credentials: Option<swoop_core::ProxyCredentials>,
    ) -> Result<(), String> {
        if self.active() {
            return Err(swoop_i18n::tr!("请先停止媒体任务再修改设置").into());
        }
        if preferences.network_mode != MediaNetworkMode::Manual
            && preferences.network.proxy.as_ref().is_some_and(|proxy| proxy.url.trim().is_empty())
        {
            preferences.network.proxy = None;
            preferences.network.proxy_auth = None;
        }
        if preferences.default_max_height.is_some_and(|height| !(1..=8640).contains(&height))
            || preferences.preferred_audio_language.len() > 64
        {
            return Err(swoop_i18n::tr!("默认高度需为 1–8640；音轨语言长度不能超过 64 字符").into());
        }
        let manual_credentials = supplied_credentials
            .or_else(|| self.manual_proxy_credentials.clone())
            .filter(|credentials| preferences.network.proxy_auth.as_ref() == Some(credentials.rule()));
        let credentials = match preferences.network_mode {
            MediaNetworkMode::Direct => None,
            MediaNetworkMode::Global => self.global_proxy_credentials.clone(),
            MediaNetworkMode::Manual => manual_credentials.clone(),
        };
        let scope = credentials.as_ref().map(|credentials| credentials.cache_scope().to_owned());
        let (downloader, network_snapshot) =
            Self::configured_downloader(&preferences, &self.global_network, credentials)?;
        let json = serde_json::to_string(&preferences).map_err(|e| e.to_string())?;
        self.store.set_setting("media_preferences", &json).map_err(|e| e.to_string())?;
        self.downloader = downloader;
        self.network_ready = true;
        self.preferences_load_error = None;
        self.network_snapshot = network_snapshot;
        self.manual_proxy_credentials = manual_credentials;
        self.proxy_snapshot_scope = scope;
        self.network_error = None;
        self.preferences = preferences;
        Ok(())
    }
    #[cfg(test)]
    pub fn add(
        &mut self,
        source: MediaSource,
        destination: PathBuf,
        duration_secs: Option<u64>,
        selection: MediaSelection,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.add_with_auth(source, destination, duration_secs, selection, MediaSessionAuth::default(), ctx)
    }
    #[cfg(test)]
    pub fn add_with_auth(
        &mut self,
        source: MediaSource,
        destination: PathBuf,
        duration_secs: Option<u64>,
        selection: MediaSelection,
        auth: MediaSessionAuth,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.add_with_auth_and_limit(
            source,
            destination,
            duration_secs,
            selection,
            auth,
            self.preferences.default_speed_limit_bps,
            ctx,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn add_with_auth_and_limit(
        &mut self,
        source: MediaSource,
        destination: PathBuf,
        duration_secs: Option<u64>,
        selection: MediaSelection,
        auth: MediaSessionAuth,
        speed_limit_bps: u64,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if let Some(error) = &self.network_error {
            return Err(error.clone());
        }
        source.validate().map_err(|e| e.to_string())?;
        if self.jobs.len() >= 1000 {
            return Err(swoop_i18n::tr!("媒体记录已满，请先移除旧记录").into());
        }
        if !destination
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("mp4") || s.eq_ignore_ascii_case("mkv"))
        {
            return Err(swoop_i18n::tr!("输出文件需要使用 .mp4 或 .mkv 扩展名").into());
        }
        let DownloadPlan { destination, status, overwrite, lease } =
            self.destination_plan(destination, self.file_conflict_policy, None)?;
        if duration_secs == Some(0) {
            return Err(swoop_i18n::tr!("录制时长必须大于零").into());
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(swoop_i18n::tr!("任务编号已满"))?;
        self.jobs.push(MediaJob {
            id,
            url: source.url,
            headers: source.headers,
            destination,
            duration_secs,
            status: status.clone(),
            bytes: 0,
            media_seconds: 0,
            selection,
            auth_required: !auth.is_empty(),
            speed_limit_bps,
        });
        if let Err(error) = self.persist() {
            self.jobs.pop();
            return Err(error);
        }
        if !auth.is_empty() {
            self.session_auth.insert(id, auth);
        }
        self.persistence_error = false;
        if status == MediaStatus::Waiting {
            self.spawn(id, overwrite, lease.expect("waiting destination reserved"), ctx);
        }
        Ok(())
    }
    pub fn add_control(
        &mut self,
        source: MediaSource,
        destination: PathBuf,
        duration_secs: Option<u64>,
        selection: MediaSelection,
        speed_limit_bps: Option<u64>,
        ctx: &egui::Context,
    ) -> Result<u64, String> {
        let id = self.next_id;
        self.add_with_auth_and_limit(
            source,
            destination,
            duration_secs,
            selection,
            MediaSessionAuth::default(),
            speed_limit_bps.unwrap_or(self.preferences.default_speed_limit_bps),
            ctx,
        )?;
        self.persistence_error = false;
        Ok(id)
    }
    pub fn set_speed_limit(&mut self, id: u64, speed_limit_bps: u64) -> Result<(), String> {
        let index = self.jobs.iter().position(|job| job.id == id).ok_or(swoop_i18n::tr!("媒体记录不存在"))?;
        let old = self.jobs[index].speed_limit_bps;
        self.jobs[index].speed_limit_bps = speed_limit_bps;
        if let Err(error) = self.persist() {
            self.jobs[index].speed_limit_bps = old;
            return Err(error);
        }
        if let Some(downloader) = self.running_downloaders.get(&id) {
            downloader.set_task_speed_limit(speed_limit_bps);
        }
        self.persistence_error = false;
        Ok(())
    }
    pub fn control_status(&self, id: Option<u64>) -> Result<serde_json::Value, String> {
        if id.is_some_and(|id| !self.jobs.iter().any(|j| j.id == id)) {
            return Err(swoop_i18n::tr!("媒体记录不存在").into());
        }
        let jobs: Vec<_> = self.jobs.iter().filter(|j| id.is_none_or(|id| j.id == id)).map(|j| serde_json::json!({
            "id":j.id, "path":j.destination, "bytes":j.bytes, "media_seconds":j.media_seconds,
            "speed_limit_bps":j.speed_limit_bps,
            "status":match j.status { MediaStatus::Waiting=>"waiting", MediaStatus::FileConflict=>"waiting_for_confirmation", MediaStatus::NeedsAuth=>"authentication_required", MediaStatus::Downloading=>"downloading", MediaStatus::Cancelling=>"cancelling", MediaStatus::Paused=>"paused", MediaStatus::Completed=>"completed", MediaStatus::Failed(_)=>"failed" },
        })).collect();
        Ok(serde_json::json!({"tasks":jobs,"persistence_error":self.persistence_error}))
    }
    fn spawn(&mut self, id: u64, overwrite: bool, lease: TargetLease, ctx: &egui::Context) {
        let Some(job) = self.jobs.iter().find(|j| j.id == id).cloned() else {
            return;
        };
        let token = CancellationToken::new();
        let auth = self.session_auth.get(&id).cloned().unwrap_or_default();
        if job.auth_required && auth.is_empty() {
            if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                job.status = MediaStatus::NeedsAuth;
            }
            if self.persist().is_err() {
                self.persistence_error = true;
            }
            return;
        }
        let downloader = match self.downloader.for_task(job.speed_limit_bps) {
            Ok(downloader) => downloader,
            Err(error) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.status = MediaStatus::Failed(error.to_string());
                }
                self.persistence_error = self.persist().is_err();
                return;
            }
        };
        self.running.insert(id, token.clone());
        self.running_downloaders.insert(id, downloader.clone());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = async {
                if let Some(parent) = job.destination.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(MediaError::from)?;
                }
                downloader
                    .download_with_auth(
                        MediaRequest {
                            source: MediaSource { url: job.url, headers: job.headers },
                            destination: job.destination,
                            overwrite,
                            duration_limit: job.duration_secs.map(Duration::from_secs),
                            selection: job.selection,
                        },
                        auth,
                        token.clone(),
                        |event| {
                            let _ = tx.send(Update::Progress(id, event));
                            ctx.request_repaint();
                        },
                    )
                    .await
                    .map(|r| r.bytes)
            }
            .await;
            drop(lease);
            let _ = tx.send(Update::Finished(id, result, token.is_cancelled()));
            ctx.request_repaint();
        });
    }
    pub fn inspect(
        &self,
        source: MediaSource,
        cancel: CancellationToken,
        ctx: &egui::Context,
    ) -> mpsc::Receiver<Result<MediaInfo, String>> {
        self.inspect_with_auth(source, MediaSessionAuth::default(), cancel, ctx)
    }
    pub fn inspect_with_auth(
        &self,
        source: MediaSource,
        auth: MediaSessionAuth,
        cancel: CancellationToken,
        ctx: &egui::Context,
    ) -> mpsc::Receiver<Result<MediaInfo, String>> {
        let downloader = self.downloader.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        if let Some(error) = &self.network_error {
            let _ = tx.send(Err(error.clone()));
            return rx;
        }
        self.handle.spawn(async move {
            let result = downloader.inspect_with_auth(&source, &auth, &cancel).await.map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        rx
    }
    pub fn update_file_policy(&mut self, engine: &swoop_core::Engine) {
        self.global_network = engine.network_policy();
        self.global_proxy_credentials = engine.proxy_credentials_snapshot();
        let credentials = match self.preferences.network_mode {
            MediaNetworkMode::Direct => None,
            MediaNetworkMode::Global => self.global_proxy_credentials.clone(),
            MediaNetworkMode::Manual => self.manual_proxy_credentials.clone(),
        };
        let scope = credentials.as_ref().map(|credentials| credentials.cache_scope().to_owned());
        let candidate = match &self.preferences_load_error {
            Some(error) => Err(error.clone()),
            None => self.preferences.network_config(&self.global_network),
        };
        match candidate {
            Ok(network)
                if self.network_ready && network == self.network_snapshot && scope == self.proxy_snapshot_scope =>
            {
                self.network_error = None
            }
            Ok(_) if self.active() => {
                self.network_error = Some(swoop_i18n::tr!("全局网络设置已变化；停止现有媒体任务后生效").into())
            }
            Ok(_) => match Self::configured_downloader(&self.preferences, &self.global_network, credentials) {
                Ok((downloader, network)) => {
                    self.downloader = downloader;
                    self.network_ready = true;
                    self.network_snapshot = network;
                    self.proxy_snapshot_scope = scope;
                    self.network_error = None;
                }
                Err(error) => self.network_error = Some(error),
            },
            Err(error) => self.network_error = Some(error),
        }
        self.file_conflict_policy = engine.file_conflict_policy();
        self.http_targets = engine
            .snapshot()
            .into_iter()
            .filter(|task| {
                task.status != TaskStatus::Completed && (task.status.is_active() || !task.segments.is_empty())
            })
            .map(|task| task.path)
            .collect();
    }
    fn reserve_destination(&self, path: &std::path::Path) -> std::io::Result<TargetLease> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        self.registry.reserve(path)
    }
    fn destination_plan(
        &self,
        destination: PathBuf,
        policy: FileConflictPolicy,
        id: Option<u64>,
    ) -> Result<DownloadPlan, String> {
        let busy = |path: &PathBuf| {
            self.jobs.iter().any(|j| Some(j.id) != id && j.status.active() && same_target(&j.destination, path))
                || self.http_targets.iter().any(|target| same_target(target, path))
        };
        let occupied = |path: &PathBuf| path.symlink_metadata().is_ok() || busy(path);
        if !occupied(&destination) {
            match self.reserve_destination(&destination) {
                Ok(lease) => {
                    return Ok(DownloadPlan::new(destination, MediaStatus::Waiting, false, Some(lease)));
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && matches!(
                            policy,
                            FileConflictPolicy::Ask | FileConflictPolicy::Rename | FileConflictPolicy::Skip
                        ) => {}
                Err(error) => return Err(swoop_i18n::trf!("目标文件无法使用：{error}")),
            }
        }
        match policy {
            FileConflictPolicy::Ask => Ok(DownloadPlan::new(destination, MediaStatus::FileConflict, false, None)),
            FileConflictPolicy::Skip => Ok(DownloadPlan::new(destination, MediaStatus::Paused, false, None)),
            FileConflictPolicy::Overwrite => {
                if busy(&destination) {
                    return Err(swoop_i18n::tr!("目标由其他下载任务占用，请先移除相关任务或选择重命名").into());
                }
                let meta = destination.symlink_metadata().map_err(|e| e.to_string())?;
                if !meta.is_file() || meta.file_type().is_symlink() {
                    return Err(swoop_i18n::tr!("覆盖目标必须为普通文件，不能是符号链接或目录").into());
                }
                let lease =
                    self.reserve_destination(&destination).map_err(|e| swoop_i18n::trf!("目标文件被占用：{e}"))?;
                Ok(DownloadPlan::new(destination, MediaStatus::Waiting, true, Some(lease)))
            }
            FileConflictPolicy::Rename => {
                let stem = destination.file_stem().ok_or(swoop_i18n::tr!("目标文件名无效"))?;
                for index in 1..=10_000 {
                    let mut name = stem.to_os_string();
                    name.push(format!(" ({index})"));
                    if let Some(ext) = destination.extension() {
                        name.push(".");
                        name.push(ext);
                    }
                    let candidate = destination.with_file_name(name);
                    if occupied(&candidate) {
                        continue;
                    }
                    match self.reserve_destination(&candidate) {
                        Ok(lease) => {
                            return Ok(DownloadPlan::new(candidate, MediaStatus::Waiting, false, Some(lease)));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(error) => return Err(error.to_string()),
                    }
                }
                Err(swoop_i18n::tr!("无法找到可用的媒体文件名").into())
            }
        }
    }
    fn save_start(&mut self, index: usize, plan: DownloadPlan, ctx: &egui::Context) -> Result<(), String> {
        if let Some(error) = &self.network_error {
            return Err(error.clone());
        }
        if self.jobs[index].auth_required && !self.session_auth.contains_key(&self.jobs[index].id) {
            self.jobs[index].status = MediaStatus::NeedsAuth;
            self.persist()?;
            return Err(swoop_i18n::tr!("请重新输入本次登录凭据").into());
        }
        let DownloadPlan { destination, status, overwrite, lease } = plan;
        let previous = self.jobs[index].clone();
        let id = previous.id;
        self.jobs[index].destination = destination;
        self.jobs[index].status = status.clone();
        self.jobs[index].bytes = 0;
        self.jobs[index].media_seconds = 0;
        if let Err(error) = self.persist() {
            self.jobs[index] = previous;
            return Err(error);
        }
        self.persistence_error = false;
        if status == MediaStatus::Waiting {
            self.spawn(id, overwrite, lease.expect("waiting destination reserved"), ctx);
        }
        Ok(())
    }
    pub fn restart(&mut self, id: u64, ctx: &egui::Context) -> Result<(), String> {
        let index = self.jobs.iter().position(|j| j.id == id).ok_or(swoop_i18n::tr!("媒体记录不存在"))?;
        if !matches!(self.jobs[index].status, MediaStatus::Paused | MediaStatus::Failed(_) | MediaStatus::NeedsAuth) {
            return Err(swoop_i18n::tr!("该任务不能重新开始").into());
        }
        let plan = self.destination_plan(self.jobs[index].destination.clone(), self.file_conflict_policy, Some(id))?;
        self.save_start(index, plan, ctx)
    }
    pub fn reauthenticate(&mut self, id: u64, auth: MediaSessionAuth, ctx: &egui::Context) -> Result<(), String> {
        if auth.is_empty() {
            return Err(swoop_i18n::tr!("请输入本次登录凭据").into());
        }
        let job = self.jobs.iter().find(|job| job.id == id).ok_or(swoop_i18n::tr!("媒体记录不存在"))?;
        if !matches!(job.status, MediaStatus::Paused | MediaStatus::Failed(_) | MediaStatus::NeedsAuth)
            || !job.auth_required
        {
            return Err(swoop_i18n::tr!("该任务当前不需要重新登录").into());
        }
        self.session_auth.insert(id, auth);
        self.restart(id, ctx)
    }
    pub fn resolve_file_conflict(
        &mut self,
        id: u64,
        policy: FileConflictPolicy,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let index = self.jobs.iter().position(|j| j.id == id).ok_or(swoop_i18n::tr!("媒体记录不存在"))?;
        if self.jobs[index].status != MediaStatus::FileConflict {
            return Err(swoop_i18n::tr!("媒体任务没有等待处理的同名文件").into());
        }
        if policy == FileConflictPolicy::Ask {
            return Err(swoop_i18n::tr!("请选择覆盖、重命名或取消").into());
        }
        let destination = self.jobs[index].destination.clone();
        let plan = if policy == FileConflictPolicy::Skip {
            DownloadPlan::new(destination, MediaStatus::Paused, false, None)
        } else {
            self.destination_plan(destination, policy, Some(id))?
        };
        if plan.status == MediaStatus::FileConflict {
            return Err(swoop_i18n::tr!("同一目标仍有媒体任务运行，请等待其结束或选择重命名").into());
        }
        self.save_start(index, plan, ctx)
    }
    pub fn cancel(&mut self, id: u64) {
        if let Some(token) = self.running.get(&id) {
            token.cancel();
            if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                job.status = MediaStatus::Cancelling;
            }
        }
    }
    pub fn cancel_all(&mut self) {
        let ids: Vec<_> = self.running.keys().copied().collect();
        for id in ids {
            self.cancel(id);
        }
    }
    pub fn remove(&mut self, id: u64) -> Result<(), String> {
        if self.running.contains_key(&id) {
            return Err(swoop_i18n::tr!("请等待媒体任务停止后移除记录").into());
        }
        let Some(index) = self.jobs.iter().position(|j| j.id == id) else {
            return Ok(());
        };
        let job = self.jobs.remove(index);
        if let Err(error) = self.persist() {
            self.jobs.insert(index, job);
            return Err(error);
        }
        self.persistence_error = false;
        self.session_auth.remove(&id);
        Ok(())
    }
    /// 完成或失败提示；返回路径名称，不将 URL 或请求头写入通知。
    pub fn poll(&mut self) -> Vec<(bool, String, String, Option<PathBuf>)> {
        let mut notices = Vec::new();
        let mut dirty = false;
        while let Ok(update) = self.rx.try_recv() {
            match update {
                Update::Progress(id, event) => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        match event {
                            MediaEvent::Started { .. } if job.status != MediaStatus::Cancelling => {
                                job.status = MediaStatus::Downloading
                            }
                            MediaEvent::Progress(progress) => {
                                job.bytes = progress.output_bytes;
                                job.media_seconds = progress.media_time.as_secs();
                            }
                            _ => {}
                        }
                    }
                }
                Update::Finished(id, result, cancelled) => {
                    self.running.remove(&id);
                    self.running_downloaders.remove(&id);
                    if result.is_ok() {
                        self.session_auth.remove(&id);
                    }
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        let name = job.destination.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        match result {
                            Ok(bytes) => {
                                job.bytes = bytes;
                                job.status = MediaStatus::Completed;
                                notices.push((true, name, String::new(), Some(job.destination.clone())));
                            }
                            // Recovery evidence must remain visible even if cancellation raced publication.
                            Err(error @ MediaError::RecoveryRequired(_)) => {
                                let error = error.to_string();
                                job.status = MediaStatus::Failed(error.clone());
                                notices.push((false, name, error, None));
                            }
                            Err(_) if cancelled => job.status = MediaStatus::Paused,
                            Err(MediaError::DestinationExists | MediaError::DestinationChanged) => {
                                job.status = MediaStatus::FileConflict
                            }
                            Err(error) => {
                                let error = error.to_string();
                                job.status = MediaStatus::Failed(error.clone());
                                notices.push((false, name, error, None));
                            }
                        }
                        dirty = true;
                    }
                }
            }
        }
        if dirty {
            match self.persist() {
                Ok(()) => self.persistence_error = false,
                Err(error) => {
                    self.persistence_error = true;
                    notices.push((false, swoop_i18n::tr!("媒体记录保存失败").into(), error, None));
                }
            }
        }
        notices
    }
}
impl Drop for MediaTasks {
    fn drop(&mut self) {
        for token in self.running.values() {
            token.cancel();
        }
    }
}

fn same_target(first: &std::path::Path, second: &std::path::Path) -> bool {
    let normalize = |path: &std::path::Path| {
        path.canonicalize().unwrap_or_else(|_| {
            path.parent()
                .and_then(|p| p.canonicalize().ok())
                .and_then(|parent| path.file_name().map(|name| parent.join(name)))
                .unwrap_or_else(|| path.to_owned())
        })
    };
    let (first, second) = (normalize(first), normalize(second));
    first.to_string_lossy().to_lowercase() == second.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_rate_survives_restart_and_database_failure_preserves_previous_limit() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("media.db");
        let store = Arc::new(Store::open(&database).unwrap());
        let mut tasks = MediaTasks::load(runtime.handle().clone(), store.clone(), TargetRegistry::default());
        let destination = directory.path().join("existing.mp4");
        std::fs::write(&destination, b"keep original").unwrap();
        tasks.file_conflict_policy = FileConflictPolicy::Skip;
        let id = tasks
            .add_control(
                MediaSource::new("https://media.test/a.m3u8"),
                destination.clone(),
                None,
                Default::default(),
                Some(123456),
                &egui::Context::default(),
            )
            .unwrap();
        assert_eq!(tasks.control_status(Some(id)).unwrap()["tasks"][0]["speed_limit_bps"], 123456);
        tasks.set_speed_limit(id, 65536).unwrap();
        let mut restored = MediaTasks::load(runtime.handle().clone(), store.clone(), TargetRegistry::default());
        assert_eq!(restored.jobs[0].speed_limit_bps, 65536);
        let sql = rusqlite::Connection::open(database).unwrap();
        sql.execute_batch("CREATE TRIGGER reject_media_rate BEFORE UPDATE ON settings WHEN NEW.key='media_history' BEGIN SELECT RAISE(FAIL, 'fixture refusal'); END;").unwrap();
        assert!(restored.set_speed_limit(id, 4096).is_err());
        assert_eq!(restored.jobs[0].speed_limit_bps, 65536);
        assert_eq!(
            MediaTasks::load(runtime.handle().clone(), store.clone(), TargetRegistry::default()).jobs[0]
                .speed_limit_bps,
            65536
        );
        assert_eq!(std::fs::read(destination).unwrap(), b"keep original");
        assert!(restored.set_speed_limit(99999, 0).is_err());
    }

    #[test]
    fn proxy_sessions_are_not_persisted_and_global_credentials_refresh_only_on_change() {
        use swoop_core::{AuthSecret, CredentialStorage, ProxyAuthRule, ProxyAuthScheme, ProxyCredentials};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let mut tasks = MediaTasks::load(runtime.handle().clone(), store.clone(), TargetRegistry::default());
        let endpoint = "http://127.0.0.1:17890";
        let rule =
            ProxyAuthRule::new(endpoint, ProxyAuthScheme::Basic, "fixture-user".into(), CredentialStorage::Session)
                .unwrap();
        let mut preferences = tasks.preferences.clone();
        preferences.network_mode = MediaNetworkMode::Manual;
        preferences.network.proxy = Some(swoop_media::MediaProxyConfig { url: endpoint.into(), bypass: vec![] });
        preferences.network.proxy_auth = Some(rule.clone());
        let credentials =
            ProxyCredentials::session(rule.clone(), AuthSecret::new("private-media-proxy-password".into()).unwrap())
                .unwrap();
        tasks.set_preferences_with_proxy(preferences.clone(), Some(credentials)).unwrap();
        assert!(tasks.network_error.is_none());
        assert!(!store.get_setting("media_preferences").unwrap().unwrap().contains("private-media-proxy-password"));
        let mut restored = MediaTasks::load(runtime.handle().clone(), store.clone(), TargetRegistry::default());
        assert!(restored.network_error.is_some(), "a restarted session must not silently connect anonymously");
        let engine = swoop_core::Engine::with_store(runtime.handle().clone(), Default::default(), store);
        engine
            .save_network_policy(
                swoop_core::NetworkPolicy {
                    proxy: Some(swoop_core::ProxyConfig { url: endpoint.into(), bypass: vec![] }),
                    proxy_auth: Some(rule),
                    ..Default::default()
                },
                Some(AuthSecret::new("global-fixture-password".into()).unwrap()),
            )
            .unwrap();
        restored.update_file_policy(&engine);
        preferences.network_mode = MediaNetworkMode::Global;
        restored.set_preferences(preferences).unwrap();
        let original_scope = restored.proxy_snapshot_scope.clone();
        assert!(original_scope.is_some());
        for _ in 0..3 {
            restored.update_file_policy(&engine);
            assert!(restored.network_error.is_none());
            assert_eq!(restored.proxy_snapshot_scope, original_scope);
        }
        engine
            .save_network_policy(
                engine.network_policy(),
                Some(AuthSecret::new("rotated-fixture-password".into()).unwrap()),
            )
            .unwrap();
        restored.update_file_policy(&engine);
        assert!(restored.network_error.is_none());
        assert_ne!(restored.proxy_snapshot_scope, original_scope);
    }
    #[test]
    fn authenticated_jobs_persist_only_requirement_and_block_anonymous_restart() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("private.mp4");
        std::fs::write(&destination, b"existing").unwrap();
        let registry = TargetRegistry::default();
        let mut tasks = MediaTasks::load(rt.handle().clone(), store.clone(), registry.clone());
        tasks.file_conflict_policy = FileConflictPolicy::Skip;
        let auth = MediaSessionAuth::new(vec![
            swoop_media::OriginCredentials::new(
                "https://private.test",
                Some(swoop_media::MediaAuthorization::Bearer("never-persist-this-token".into())),
                vec![swoop_media::ScopedCookie {
                    name: "session".into(),
                    value: "never-persist-this-cookie".into(),
                    path: "/".into(),
                    secure: true,
                    expires_at: None,
                }],
            )
            .unwrap(),
        ])
        .unwrap();
        tasks
            .add_with_auth(
                MediaSource::new("https://private.test/vod.m3u8"),
                destination,
                None,
                Default::default(),
                auth.clone(),
                &egui::Context::default(),
            )
            .unwrap();
        let id = tasks.jobs[0].id;
        assert!(tasks.jobs[0].auth_required);
        assert_eq!(tasks.jobs[0].status, MediaStatus::Paused);
        let json = store.get_setting("media_history").unwrap().unwrap();
        assert!(json.contains("\"auth_required\":true"));
        assert!(!json.contains("never-persist"));
        assert!(!json.contains("Bearer"));
        drop(tasks);
        let mut restored = MediaTasks::load(rt.handle().clone(), store, registry);
        assert_eq!(restored.jobs[0].status, MediaStatus::NeedsAuth);
        assert!(restored.session_auth.is_empty());
        assert!(restored.restart(id, &egui::Context::default()).is_err());
        assert!(!restored.active());
        assert_eq!(restored.control_status(Some(id)).unwrap()["tasks"][0]["status"], "authentication_required");
        restored.file_conflict_policy = FileConflictPolicy::Skip;
        restored.reauthenticate(id, auth, &egui::Context::default()).unwrap();
        assert_eq!(restored.jobs[0].status, MediaStatus::Paused);
        assert!(restored.session_auth.contains_key(&id));
    }

    #[test]
    fn proxy_modes_map_explicit_policy_and_never_persist_proxy_passwords() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let mut tasks = MediaTasks::load(rt.handle().clone(), store.clone(), TargetRegistry::default());
        let mut preferences = tasks.preferences.clone();
        preferences.network_mode = MediaNetworkMode::Global;
        let global = swoop_core::NetworkPolicy {
            proxy: Some(swoop_core::ProxyConfig {
                url: "socks5h://127.0.0.1:1080".into(),
                bypass: vec!["localhost".into()],
            }),
            ..Default::default()
        };
        let mapped = preferences.network_config(&global).unwrap().unwrap();
        assert_eq!(mapped.proxy.unwrap().url, "socks5h://127.0.0.1:1080");
        let authenticated_proxy = swoop_core::NetworkPolicy {
            proxy: Some(swoop_core::ProxyConfig { url: "http://127.0.0.1:7890".into(), bypass: vec![] }),
            proxy_auth: Some(
                swoop_core::ProxyAuthRule::new(
                    "http://127.0.0.1:7890",
                    swoop_core::ProxyAuthScheme::Basic,
                    "test-user".into(),
                    swoop_core::CredentialStorage::Session,
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        assert_eq!(
            preferences.network_config(&authenticated_proxy).unwrap().unwrap().proxy_auth,
            authenticated_proxy.proxy_auth,
            "global proxy authentication must be retained"
        );
        preferences.network_mode = MediaNetworkMode::Direct;
        assert!(preferences.network_config(&authenticated_proxy).unwrap().unwrap().proxy.is_none());
        preferences.network.proxy = Some(swoop_media::MediaProxyConfig {
            url: "http://user:never-save-password@127.0.0.1:8080".into(),
            bypass: vec![],
        });
        assert!(tasks.set_preferences(preferences.clone()).is_err());
        assert!(store.get_setting("media_preferences").unwrap().is_none());
        // A damaged/old persisted manual configuration must block work, not
        // silently construct an effective direct-network downloader.
        preferences.network_mode = MediaNetworkMode::Manual;
        store.set_setting("media_preferences", &serde_json::to_string(&preferences).unwrap()).unwrap();
        let mut restored = MediaTasks::load(rt.handle().clone(), store.clone(), TargetRegistry::default());
        assert!(restored.network_error.is_some());
        let dir = tempfile::tempdir().unwrap();
        assert!(
            restored
                .add(
                    MediaSource::new("https://private.test/vod.m3u8"),
                    dir.path().join("movie.mp4"),
                    None,
                    Default::default(),
                    &egui::Context::default()
                )
                .is_err()
        );
        assert!(restored.jobs.is_empty());
        assert!(!restored.active());
        preferences.network.proxy.as_mut().unwrap().url = "http://127.0.0.1:8123".into();
        preferences.concurrency = 0;
        store.set_setting("media_preferences", &serde_json::to_string(&preferences).unwrap()).unwrap();
        let mut restored = MediaTasks::load(rt.handle().clone(), store.clone(), TargetRegistry::default());
        assert!(restored.network_error.is_some());
        assert_eq!(restored.preferences.network_mode, MediaNetworkMode::Manual);
        let engine = swoop_core::Engine::new(rt.handle().clone(), Default::default());
        restored.update_file_policy(&engine);
        assert!(restored.network_error.is_some());
        store.set_setting("media_preferences", "{invalid").unwrap();
        let mut restored = MediaTasks::load(rt.handle().clone(), store, TargetRegistry::default());
        restored.update_file_policy(&engine);
        assert!(restored.network_error.is_some());
        assert!(!restored.network_ready);
    }
    #[test]
    fn recovery_required_survives_cancellation_and_history_reload() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let recovery = dir.path().join(".swoop-recovery-原件 空格.mp4");
        std::fs::write(&recovery, b"external saved bytes").unwrap();
        for cancelled in [false, true] {
            let store = Arc::new(Store::in_memory().unwrap());
            let mut tasks = MediaTasks::load(rt.handle().clone(), store.clone(), TargetRegistry::default());
            tasks.jobs.push(MediaJob {
                id: 1,
                url: "https://example.test/a.m3u8?token=private".into(),
                headers: vec![],
                destination: dir.path().join("video.mp4"),
                duration_secs: None,
                status: if cancelled { MediaStatus::Cancelling } else { MediaStatus::Downloading },
                bytes: 0,
                media_seconds: 0,
                selection: Default::default(),
                auth_required: true,
                speed_limit_bps: 0,
            });
            let token = CancellationToken::new();
            if cancelled {
                token.cancel();
            }
            tasks.running.insert(1, token);
            tasks.tx.send(Update::Finished(1, Err(MediaError::RecoveryRequired(recovery.clone())), cancelled)).unwrap();
            let notices = tasks.poll();
            assert!(!tasks.active());
            let MediaStatus::Failed(message) = &tasks.jobs[0].status else {
                panic!("recovery was hidden as {:?}", tasks.jobs[0].status);
            };
            assert!(message.contains(recovery.to_str().unwrap()));
            assert_eq!(notices.len(), 1);
            assert!(!notices[0].0);
            assert_eq!(&notices[0].2, message);
            assert!(notices[0].3.is_none(), "recovery must never dispatch a success/open action");
            assert!(!notices[0].2.contains("token=private"));
            assert_eq!(tasks.control_status(Some(1)).unwrap()["tasks"][0]["status"], "failed");
            let restored = MediaTasks::load(rt.handle().clone(), store, TargetRegistry::default());
            assert_eq!(restored.jobs[0].status, tasks.jobs[0].status);
            assert!(!tasks.persistence_error);
            assert_eq!(std::fs::read(&recovery).unwrap(), b"external saved bytes");
        }
    }

    #[test]
    fn shared_registry_blocks_both_engines_and_paused_zero_byte_partial() {
        use swoop_core::{AddOptions, Config, Engine};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
        let registry = engine.target_registry();
        let mut media = MediaTasks::load(rt.handle().clone(), store.clone(), registry.clone());
        let ctx = egui::Context::default();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else { break };
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut byte = [0];
                        while !request.ends_with(b"\r\n\r\n") {
                            if socket.read_exact(&mut byte).await.is_err() { return; }
                            request.push(byte[0]);
                            if request.len() > 16384 { return; }
                        }
                        let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                        if text.contains("/pending.m3u8") {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        } else if text.contains("range: bytes=0-0\r\n") {
                            let _ = socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/8192\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\nx").await;
                        } else {
                            let _ = socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 8192\r\nContent-Range: bytes 0-8191/8192\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n").await;
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                    });
                }
            });
            let source = || MediaSource { url: format!("{base}/pending.m3u8"), headers: vec![] };
            let add_http = |name: &str| engine.add(AddOptions { url: format!("{base}/file.mp4"), dir: dir.path().into(), file_name: Some(name.into()), ..Default::default() }).unwrap();
            // A media worker reserves its final path even before any final file exists.
            let path = dir.path().join("media-first.mp4");
            let media_id = media.add_control(source(), path.clone(), None, Default::default(), None, &ctx).unwrap();
            assert!(registry.is_reserved(&path).unwrap());
            let http_id = add_http("media-first.mp4");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                let task = engine.snapshot().into_iter().find(|t| t.id == http_id).unwrap();
                if task.status == TaskStatus::FileConflict { break; }
                assert!(std::time::Instant::now() < deadline, "{:?}", task.status);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(engine.resolve_file_conflict(http_id, FileConflictPolicy::Overwrite).is_err());
            assert!(!path.exists());
            engine.resolve_file_conflict(http_id, FileConflictPolicy::Skip).unwrap();
            media.cancel(media_id);
            while media.active() { media.poll(); assert!(std::time::Instant::now() < deadline); tokio::time::sleep(Duration::from_millis(10)).await; }
            assert!(!registry.is_reserved(&path).unwrap());
            // A preallocated HTTP download owns the target even when received bytes remain zero.
            let path = dir.path().join("http-first.mp4");
            let http_id = add_http("http-first.mp4");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                let task = engine.snapshot().into_iter().find(|t| t.id == http_id).unwrap();
                if task.status == TaskStatus::Downloading && !task.segments.is_empty() { assert_eq!(task.downloaded, 0); break; }
                assert!(std::time::Instant::now() < deadline, "{:?}", task.status);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            media.update_file_policy(&engine);
            let id = media.add_control(source(), path.clone(), None, Default::default(), None, &ctx).unwrap();
            assert_eq!(media.jobs.last().unwrap().status, MediaStatus::FileConflict);
            assert!(media.resolve_file_conflict(id, FileConflictPolicy::Overwrite, &ctx).is_err());
            engine.pause(http_id).unwrap();
            while registry.is_reserved(&path).unwrap() { assert!(std::time::Instant::now() < deadline); tokio::time::sleep(Duration::from_millis(10)).await; }
            media.update_file_policy(&engine);
            assert!(media.resolve_file_conflict(id, FileConflictPolicy::Overwrite, &ctx).is_err());
            assert_eq!(MediaTasks::load(rt.handle().clone(), store.clone(), registry.clone()).jobs.last().unwrap().status, MediaStatus::FileConflict);
            media.resolve_file_conflict(id, FileConflictPolicy::Rename, &ctx).unwrap();
            let renamed = media.jobs.last().unwrap().destination.clone();
            assert_ne!(renamed, path);
            assert!(registry.is_reserved(&renamed).unwrap());
            media.cancel(id);
            while media.active() { media.poll(); assert!(std::time::Instant::now() < deadline); tokio::time::sleep(Duration::from_millis(10)).await; }
            assert!(!renamed.exists());
            // HTTP 的半成品写在 `.swoop` 临时文件里，目标名直到完成才出现。
            assert!(!path.exists());
            assert_eq!(std::fs::metadata(dir.path().join("http-first.mp4.swoop")).unwrap().len(), 8192);
        });
    }

    #[test]
    fn restores_active_jobs_as_stopped_and_preserves_sequence_after_removal() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let history = History {
            next_id: 52,
            jobs: vec![MediaJob {
                id: 51,
                url: "https://example.test/a.m3u8".into(),
                headers: vec![],
                destination: "a.mp4".into(),
                duration_secs: None,
                status: MediaStatus::Downloading,
                bytes: 10,
                media_seconds: 1,
                auth_required: false,
                speed_limit_bps: 0,
                selection: MediaSelection {
                    video: VideoSelection::MaxHeight(720),
                    audio: AudioSelection::Track("saved-track".into()),
                    subtitle: swoop_media::SubtitleSelection::Track("saved-subtitle".into()),
                },
            }],
        };
        store.set_setting("media_history", &serde_json::to_string(&history).unwrap()).unwrap();
        let mut tasks = MediaTasks::load(rt.handle().clone(), store.clone(), TargetRegistry::default());
        assert_eq!(tasks.jobs[0].status, MediaStatus::Paused);
        assert_eq!(tasks.jobs[0].selection.video, VideoSelection::MaxHeight(720));
        assert_eq!(tasks.jobs[0].selection.audio, AudioSelection::Track("saved-track".into()));
        assert_eq!(tasks.jobs[0].selection.subtitle, swoop_media::SubtitleSelection::Track("saved-subtitle".into()));
        tasks.remove(51).unwrap();
        assert_eq!(MediaTasks::load(rt.handle().clone(), store, TargetRegistry::default()).next_id, 52);
    }
}
