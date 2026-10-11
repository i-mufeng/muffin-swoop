//! 任务调度与分段下载 worker。

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::{self, HeaderMap};
use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::{Error, Result};
use crate::limiter::Limiter;
use crate::network::{NetworkPolicy, NetworkRuntime, retryable, wait_retry};
use crate::probe::{ProbeInfo, build_headers, probe_network};
use crate::queue::{MAIN_QUEUE_ID, QueueClock, QueueId, QueueRecord, SystemQueueClock};
use crate::quota::{Quota, QuotaPolicy, QuotaSnapshot};
use crate::segment::{SegmentMap, SegmentState};
use crate::store::{Store, TaskRecord};
use crate::task::{SegmentSnapshot, TaskId, TaskProperties, TaskSnapshot, TaskStatus};
use crate::util::{open_resume_file, sanitize_file_name, write_at};
use crate::{FileConflictPolicy, FtpCredentials, FtpTaskOptions};

mod auth;
mod ftp;
mod queues;
mod sync;

/// 每个任务保留的速度采样数（每秒一个），即速度曲线覆盖的秒数
pub const SPEED_HISTORY_LEN: usize = 120;
/// 每个任务允许的最大连接数。
pub const MAX_CONNECTIONS: usize = 32;

#[derive(Clone, Debug)]
pub struct Config {
    /// 每个任务的最大连接数
    pub connections: usize,
    /// 分段下限（字节）；小于 2 倍此值的区段不再被窃取
    pub min_segment: u64,
    /// 每个区段的最大重试次数
    pub retries: u32,
    pub user_agent: String,
    /// 全局限速（字节/秒），0 为不限
    pub speed_limit: u64,
    pub connect_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connections: 8,
            min_segment: 1024 * 1024,
            retries: 5,
            user_agent: format!("Swoop/{}", env!("CARGO_PKG_VERSION")),
            speed_limit: 0,
            connect_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AddOptions {
    pub url: String,
    pub dir: PathBuf,
    /// 不填则由探测结果决定
    pub file_name: Option<String>,
    /// 探测和每个区段请求都附加的头（如浏览器带来的 Cookie / Referer / User-Agent）；
    /// 其中的 User-Agent 覆盖 [`Config::user_agent`]，Range 等引擎自管的头会被忽略
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Changed(TaskId),
    Completed(TaskId),
    Failed(TaskId, String),
    Removed(TaskId),
    FileConflict(TaskId),
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

/// Prevents new work and scheduled starts while a caller performs an idle-only operation.
/// Dropping the guard resumes admission; commit keeps this engine quiescent until shutdown.
pub struct Quiescence {
    inner: Arc<Inner>,
    committed: bool,
}
impl Quiescence {
    pub fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for Quiescence {
    fn drop(&mut self) {
        if !self.committed {
            let _tasks = self.inner.tasks.lock().unwrap();
            self.inner.quiescent.store(false, Ordering::Release);
            self.inner.queue_wake.notify_one();
        }
    }
}

struct Inner {
    handle: Handle,
    network: Mutex<NetworkRuntime>,
    file_conflict_policy: Mutex<FileConflictPolicy>,
    ftp_defaults: Mutex<FtpTaskOptions>,
    target_registry: crate::TargetRegistry,
    tasks: Mutex<BTreeMap<TaskId, Entry>>,
    quiescent: AtomicBool,
    next_id: AtomicU64,
    events: broadcast::Sender<Event>,
    limiter: Arc<Limiter>,
    quota: Arc<Quota>,
    connections: AtomicUsize,
    config: Config,
    store: Option<Arc<Store>>,
    queues: Mutex<BTreeMap<QueueId, QueueRecord>>,
    queue_clock: Arc<dyn QueueClock>,
    queue_wake: Arc<Notify>,
    run_slots: Mutex<BTreeMap<TaskId, QueueId>>,
}

struct Entry {
    shared: Arc<TaskShared>,
    cancel: CancellationToken,
    running: bool,
    run_queue: Option<QueueId>,
    connections: usize,
}

struct RunStatus {
    status: TaskStatus,
    generation: u64,
}

struct TaskShared {
    id: TaskId,
    url: Url,
    /// 原样保留用于持久化；请求时用解析好的 `header_map`
    headers: Vec<(String, String)>,
    header_map: HeaderMap,
    description: Mutex<String>,
    limiter: Arc<Limiter>,
    source_changed: AtomicBool,
    sync_state: Mutex<crate::sync::SyncState>,
    /// 当次同步的验证器；不会持久化，取消后重新探测。
    sync_probe: Mutex<Option<ProbeInfo>>,
    ftp_options: Mutex<Option<FtpTaskOptions>>,
    ftp_credentials: Mutex<Option<FtpCredentials>>,
    conflict_resolution: Mutex<Option<FileConflictPolicy>>,
    conflict_target: Mutex<Option<std::fs::Metadata>>,
    dir: PathBuf,
    file_name: Mutex<String>,
    user_named: bool,
    status: Mutex<RunStatus>,
    /// 同一任务的旧运行清理完文件与区段后，新运行才能开始；UI 不等待这把异步锁。
    run_lock: tokio::sync::Mutex<()>,
    /// 0 表示未知
    total: AtomicU64,
    downloaded: AtomicU64,
    /// f64 bits
    speed: AtomicU64,
    resumable: AtomicBool,
    etag: Mutex<Option<String>>,
    segments: Mutex<SegmentMap>,
    created_at: SystemTime,
    completed_at: Mutex<Option<SystemTime>>,
    /// 最近的每秒吞吐（字节/秒），旧的在前；只在内存里，不落盘
    history: Mutex<VecDeque<f64>>,
}

impl TaskShared {
    fn mark_paused(&self) -> bool {
        let mut state = self.status.lock().unwrap();
        if !state.status.is_active() && state.status != TaskStatus::FileConflict {
            return false;
        }
        state.status = TaskStatus::Paused;
        true
    }
    fn status(&self) -> TaskStatus {
        self.status.lock().unwrap().status.clone()
    }
    fn begin_run(&self) -> u64 {
        let mut state = self.status.lock().unwrap();
        state.generation += 1;
        state.status = TaskStatus::Queued;
        state.generation
    }
    fn queue_again(&self) {
        let mut state = self.status.lock().unwrap();
        state.generation += 1;
        state.status = TaskStatus::Queued;
    }
    /// 暂停或更新的运行已经接管状态时，旧运行不得再写入状态。
    fn set_run_status(&self, generation: u64, status: TaskStatus) -> bool {
        let mut state = self.status.lock().unwrap();
        if state.generation != generation || state.status == TaskStatus::Paused {
            return false;
        }
        state.status = status;
        true
    }
    fn total(&self) -> Option<u64> {
        let t = self.total.load(Ordering::Relaxed);
        (t > 0).then_some(t)
    }
    fn speed(&self) -> f64 {
        f64::from_bits(self.speed.load(Ordering::Relaxed))
    }
    fn set_speed(&self, v: f64) {
        self.speed.store(v.to_bits(), Ordering::Relaxed);
    }
    fn path(&self) -> PathBuf {
        self.dir.join(&*self.file_name.lock().unwrap())
    }
    fn push_history(&self, bps: f64) {
        let mut h = self.history.lock().unwrap();
        if h.len() == SPEED_HISTORY_LEN {
            h.pop_front();
        }
        h.push_back(bps);
    }
}

fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn from_unix(secs: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64)
}

impl Engine {
    pub fn new(handle: Handle, config: Config) -> Self {
        Self::build(handle, config, None)
    }

    /// 带持久化的引擎；之后调用 [`Engine::load`] 恢复历史任务。
    pub fn with_store(handle: Handle, config: Config, store: Arc<Store>) -> Self {
        Self::build(handle, config, Some(store))
    }

    fn build(handle: Handle, config: Config, store: Option<Arc<Store>>) -> Self {
        Self::with_queue_clock(handle, config, store, Arc::new(SystemQueueClock))
    }

    /// 可注入调度时钟；生产调用通常使用 new/with_store。
    pub fn with_queue_clock(
        handle: Handle,
        mut config: Config,
        store: Option<Arc<Store>>,
        queue_clock: Arc<dyn QueueClock>,
    ) -> Self {
        config.connections = config.connections.clamp(1, MAX_CONNECTIONS);
        let network = NetworkRuntime::new(
            NetworkPolicy {
                connect_timeout_secs: config.connect_timeout.as_secs().clamp(1, 86400),
                retries: config.retries.min(100),
                ..NetworkPolicy::default()
            },
            &config.user_agent,
        )
        .expect("构建 HTTP client 失败");
        let (events, _) = broadcast::channel(256);
        let records = store
            .as_ref()
            .map(|s| s.load_queues())
            .transpose()
            .unwrap_or_else(|e| {
                tracing::error!("读取队列失败，等待 load 返回错误: {e}");
                None
            })
            .unwrap_or_else(|| vec![QueueRecord::main()]);
        let quota = Quota::new(store.clone(), queue_clock.clone());
        let conflict_policy = store
            .as_ref()
            .and_then(|store| store.get_setting("file_conflict_policy").ok().flatten())
            .as_deref()
            .and_then(FileConflictPolicy::parse)
            .unwrap_or_default();
        let ftp_defaults = store
            .as_ref()
            .and_then(|store| store.get_setting("ftp_defaults").ok().flatten())
            .and_then(|value| serde_json::from_str::<FtpTaskOptions>(&value).ok())
            .filter(|options| options.username.is_none() && options.validate("ftp://localhost/default").is_ok())
            .unwrap_or_default();
        let inner = Arc::new(Inner {
            handle,
            network: Mutex::new(network),
            file_conflict_policy: Mutex::new(conflict_policy),
            ftp_defaults: Mutex::new(ftp_defaults),
            target_registry: crate::TargetRegistry::default(),
            tasks: Mutex::new(BTreeMap::new()),
            quiescent: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            events,
            limiter: Arc::new(Limiter::new(config.speed_limit)),
            quota,
            connections: AtomicUsize::new(config.connections),
            config,
            store,
            queues: Mutex::new(records.into_iter().map(|q| (q.id, q)).collect()),
            queue_clock,
            queue_wake: Arc::new(Notify::new()),
            run_slots: Mutex::new(BTreeMap::new()),
        });
        queues::start_loop(&inner);
        Self { inner }
    }

    pub fn store(&self) -> Option<&Arc<Store>> {
        self.inner.store.as_ref()
    }

    pub fn try_quiesce(&self) -> Option<Quiescence> {
        let tasks = self.inner.tasks.lock().unwrap();
        if self.inner.quiescent.load(Ordering::Acquire)
            || !self.inner.run_slots.lock().unwrap().is_empty()
            || tasks.values().any(|entry| {
                entry.running || entry.shared.status().is_active() || entry.shared.status() == TaskStatus::FileConflict
            })
        {
            return None;
        }
        self.inner.quiescent.store(true, Ordering::Release);
        Some(Quiescence { inner: self.inner.clone(), committed: false })
    }

    // Call with tasks locked to serialize against try_quiesce and queue dispatch.
    fn require_admission(&self) -> Result<()> {
        if self.inner.quiescent.load(Ordering::Acquire) {
            Err(Error::InvalidTaskUpdate("应用正在准备退出，暂不接受新的下载操作".into()))
        } else {
            Ok(())
        }
    }

    /// 从数据库恢复任务。`auto_resume` 为 true 时，上次活动的任务回到各自队列等待调度；否则恢复为已暂停。
    /// 仅允许在尚未添加或恢复任务时调用，避免替换正在运行的任务及其文件锁。返回恢复的任务数。
    pub fn load(&self, auto_resume: bool) -> Result<usize> {
        let Some(store) = &self.inner.store else {
            return Ok(0);
        };
        let mut tasks = self.inner.tasks.lock().unwrap();
        let mut queues = self.inner.queues.lock().unwrap();
        if !tasks.is_empty() || !self.inner.run_slots.lock().unwrap().is_empty() {
            return Err(Error::Internal("恢复历史任务必须在添加或运行任务之前进行".into()));
        }
        self.load_site_auth()?;
        // 与调度器串行读取，避免读取过程中定时边界被消费、然后又被旧快照覆盖。
        let records = store.load_tasks()?;
        let loaded_queues = store.load_queues()?;
        let mut count = 0;
        *queues = loaded_queues.into_iter().map(|q| (q.id, q)).collect();
        for rec in records {
            let Ok(url) = parse_download_url(&rec.url) else {
                tracing::warn!(id = rec.id.0, "跳过无效 URL 的任务");
                continue;
            };
            let header_map = build_headers(&rec.headers).unwrap_or_else(|e| {
                tracing::warn!(id = rec.id.0, "忽略无效的请求头: {e}");
                HeaderMap::new()
            });
            let ftp_options = if is_ftp(&url) {
                Some(rec.ftp_options.unwrap_or(FtpTaskOptions::for_url(url.as_str())?))
            } else {
                None
            };
            if let Some(options) = &ftp_options {
                options.validate(url.as_str())?;
            }
            let status = match rec.status {
                status
                    if status != TaskStatus::Completed
                        && ftp_options.as_ref().is_some_and(|opts| opts.username.is_some()) =>
                {
                    TaskStatus::Failed(Error::FtpAuthenticationRequired.to_string())
                }
                TaskStatus::Completed => TaskStatus::Completed,
                TaskStatus::FileConflict => TaskStatus::FileConflict,
                TaskStatus::Failed(e) => TaskStatus::Failed(e),
                status if status.is_active() && auto_resume => TaskStatus::Queued,
                _ => TaskStatus::Paused,
            };
            let mut segments = rec.segments;
            segments.reset_active();
            let downloaded = if status == TaskStatus::Completed {
                rec.total.unwrap_or(rec.downloaded)
            } else {
                segments.total_downloaded()
            };
            let shared = Arc::new(TaskShared {
                id: rec.id,
                url,
                headers: rec.headers,
                header_map,
                description: Mutex::new(rec.description),
                limiter: Arc::new(Limiter::new(rec.speed_limit)),
                source_changed: AtomicBool::new(rec.source_changed),
                sync_state: Mutex::new(rec.sync_state),
                sync_probe: Mutex::new(None),
                ftp_options: Mutex::new(ftp_options),
                ftp_credentials: Mutex::new(None),
                conflict_resolution: Mutex::new(None),
                conflict_target: Mutex::new(if status == TaskStatus::FileConflict {
                    std::fs::symlink_metadata(rec.dir.join(&rec.file_name)).ok()
                } else {
                    None
                }),
                dir: rec.dir,
                file_name: Mutex::new(rec.file_name),
                user_named: rec.user_named,
                status: Mutex::new(RunStatus { status, generation: 0 }),
                run_lock: tokio::sync::Mutex::new(()),
                total: AtomicU64::new(rec.total.unwrap_or(0)),
                downloaded: AtomicU64::new(downloaded),
                speed: AtomicU64::new(0),
                resumable: AtomicBool::new(rec.resumable),
                etag: Mutex::new(rec.etag),
                segments: Mutex::new(segments),
                created_at: from_unix(rec.created_at),
                completed_at: Mutex::new(rec.completed_at.map(from_unix)),
                history: Mutex::default(),
            });
            tasks.insert(
                rec.id,
                Entry {
                    shared,
                    cancel: CancellationToken::new(),
                    running: false,
                    run_queue: None,
                    connections: self.connections(),
                },
            );
            self.inner.next_id.fetch_max(rec.id.0 + 1, Ordering::Relaxed);
            count += 1;
        }
        drop(queues);
        drop(tasks);
        self.inner.queue_wake.notify_one();
        Ok(count)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// 启动时的配置；当前默认连接数请用 [`Self::connections`] 读取。
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// 新建任务或下次继续任务时使用的最大连接数。
    pub fn connections(&self) -> usize {
        self.inner.connections.load(Ordering::Relaxed)
    }

    /// 更新默认连接数，限于 1..=MAX_CONNECTIONS；已经运行的任务不受影响。
    pub fn set_connections(&self, connections: usize) {
        self.inner.connections.store(connections.clamp(1, MAX_CONNECTIONS), Ordering::Relaxed);
    }

    pub fn quota(&self) -> QuotaSnapshot {
        self.inner.quota.snapshot()
    }

    /// 配额变更保留当前窗口用量，并立即唤醒等待中的下载；窗口和账本自动持久化。
    pub fn set_quota_policy(&self, policy: QuotaPolicy) -> Result<()> {
        self.inner.quota.set_policy(policy)
    }

    pub fn network_policy(&self) -> NetworkPolicy {
        self.inner.network.lock().unwrap().policy.clone()
    }

    /// 先验证并构建客户端，再原子替换默认策略；活动 run 保留原客户端和参数。
    pub fn set_network_policy(&self, policy: NetworkPolicy) -> Result<()> {
        let mut network = NetworkRuntime::new(policy, &self.inner.config.user_agent)?;
        let mut current = self.inner.network.lock().unwrap();
        network.inherit_auth(&current);
        *current = network;
        Ok(())
    }

    pub fn speed_limit(&self) -> u64 {
        self.inner.limiter.rate()
    }

    pub fn file_conflict_policy(&self) -> FileConflictPolicy {
        *self.inner.file_conflict_policy.lock().unwrap()
    }

    pub fn target_registry(&self) -> crate::TargetRegistry {
        self.inner.target_registry.clone()
    }

    pub fn set_file_conflict_policy(&self, policy: FileConflictPolicy) -> Result<()> {
        let mut current = self.inner.file_conflict_policy.lock().unwrap();
        if let Some(store) = &self.inner.store {
            store.set_setting("file_conflict_policy", policy.as_str())?;
        }
        *current = policy;
        Ok(())
    }

    /// 单次处理等待确认的任务；覆盖授权只作用于当时展示的目标，不改变全局策略。
    pub fn resolve_file_conflict(&self, id: TaskId, policy: FileConflictPolicy) -> Result<()> {
        if policy == FileConflictPolicy::Ask {
            return Err(Error::InvalidTaskUpdate("请选择覆盖、重命名或跳过".into()));
        }
        let tasks = self.inner.tasks.lock().unwrap();
        self.require_admission()?;
        let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        if entry.shared.status() != TaskStatus::FileConflict {
            return Err(Error::InvalidTaskUpdate("任务当前没有等待处理的同名文件".into()));
        }
        if policy == FileConflictPolicy::Overwrite && self.inner.target_registry.is_reserved(&entry.shared.path())? {
            return Err(Error::InvalidTaskUpdate("同名文件正由另一个任务使用，请重命名或稍后重试".into()));
        }
        if policy == FileConflictPolicy::Overwrite {
            sync::check_unfinished_owner(&tasks, &entry.shared, &entry.shared.path())?;
        }
        let mut record = record_of(&entry.shared);
        record.status = if policy == FileConflictPolicy::Skip { TaskStatus::Paused } else { TaskStatus::Queued };
        if let Some(store) = &self.inner.store {
            store.save_task(&record)?;
        }
        if policy == FileConflictPolicy::Skip {
            entry.shared.mark_paused();
            entry.cancel.cancel();
        } else {
            *entry.shared.conflict_resolution.lock().unwrap() = Some(policy);
            entry.shared.queue_again();
        }
        let _ = self.inner.events.send(Event::Changed(id));
        drop(tasks);
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    pub fn set_speed_limit(&self, bps: u64) {
        self.inner.limiter.set_rate(bps);
    }

    pub fn add(&self, opts: AddOptions) -> Result<TaskId> {
        self.enqueue(opts, MAIN_QUEUE_ID)
    }

    /// 加入指定队列末尾；队列停止或达到并发上限时保持等待。
    pub fn enqueue(&self, opts: AddOptions, queue: QueueId) -> Result<TaskId> {
        let url = parse_download_url(&opts.url)?;
        let ftp = is_ftp(&url).then(|| self.ftp_options_for_url(&url)).transpose()?;
        self.enqueue_transport(opts, queue, ftp, None)
    }

    /// FTP/FTPS 与普通下载共用队列。凭据只在本次会话内保留。
    pub fn enqueue_ftp(
        &self,
        opts: AddOptions,
        queue: QueueId,
        mut ftp: FtpTaskOptions,
        credentials: Option<FtpCredentials>,
    ) -> Result<TaskId> {
        if let Some(credentials) = &credentials {
            if ftp.username.as_deref().is_some_and(|username| username != credentials.username()) {
                return Err(Error::InvalidTaskUpdate("FTP 凭据用户名与任务配置不符".into()));
            }
            ftp.username = Some(credentials.username().to_owned());
        }
        ftp.validate(&opts.url)?;
        self.enqueue_transport(opts, queue, Some(ftp), credentials)
    }

    fn enqueue_transport(
        &self,
        opts: AddOptions,
        queue: QueueId,
        ftp_options: Option<FtpTaskOptions>,
        ftp_credentials: Option<FtpCredentials>,
    ) -> Result<TaskId> {
        let url = parse_download_url(&opts.url)?;
        if is_ftp(&url) && !opts.headers.is_empty() {
            return Err(Error::InvalidTaskUpdate("FTP 认证必须使用独立凭据，不能设置 HTTP 请求头".into()));
        }
        let header_map = build_headers(&opts.headers)?;
        let created_at = SystemTime::now();
        let user_named = opts.file_name.is_some();
        if let Some(name) = &opts.file_name
            && (name.contains(['/', '\\']) || name.trim_matches(|c: char| c == '.' || c.is_whitespace()).is_empty())
        {
            return Err(Error::InvalidTaskUpdate("文件名必须是单个非空名称，不能包含路径".into()));
        }
        let file_name = opts.file_name.map(|name| sanitize_file_name(&name)).unwrap_or_default();
        let mut tasks = self.inner.tasks.lock().unwrap();
        self.require_admission()?;
        let mut queues = self.inner.queues.lock().unwrap();
        let queue_record = queues.get_mut(&queue).ok_or(Error::QueueNotFound)?;

        let id = match &self.inner.store {
            Some(store) => store.insert_task_in_queue(
                &TaskRecord {
                    id: TaskId(0),
                    url: url.to_string(),
                    dir: opts.dir.clone(),
                    file_name: file_name.clone(),
                    user_named,
                    total: None,
                    resumable: false,
                    etag: None,
                    status: TaskStatus::Queued,
                    segments: SegmentMap::default(),
                    downloaded: 0,
                    created_at: unix_secs(created_at),
                    completed_at: None,
                    headers: opts.headers.clone(),
                    description: String::new(),
                    speed_limit: 0,
                    source_changed: false,
                    sync_state: Default::default(),
                    ftp_options: ftp_options.clone(),
                },
                queue,
            )?,
            None => TaskId(self.inner.next_id.fetch_add(1, Ordering::Relaxed)),
        };

        let shared = Arc::new(TaskShared {
            id,
            url,
            headers: opts.headers,
            header_map,
            description: Mutex::new(String::new()),
            limiter: Arc::new(Limiter::new(0)),
            source_changed: AtomicBool::new(false),
            sync_state: Mutex::default(),
            sync_probe: Mutex::new(None),
            ftp_options: Mutex::new(ftp_options),
            ftp_credentials: Mutex::new(ftp_credentials),
            conflict_resolution: Mutex::new(None),
            conflict_target: Mutex::new(None),
            dir: opts.dir,
            user_named,
            file_name: Mutex::new(file_name),
            status: Mutex::new(RunStatus { status: TaskStatus::Queued, generation: 0 }),
            run_lock: tokio::sync::Mutex::new(()),
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            speed: AtomicU64::new(0),
            resumable: AtomicBool::new(false),
            etag: Mutex::new(None),
            segments: Mutex::new(SegmentMap::default()),
            created_at,
            completed_at: Mutex::new(None),
            history: Mutex::default(),
        });
        let cancel = CancellationToken::new();
        tasks.insert(id, Entry { shared, cancel, running: false, run_queue: None, connections: self.connections() });
        queue_record.task_ids.push(id);
        drop(queues);
        drop(tasks);
        self.inner.queue_wake.notify_one();
        Ok(id)
    }

    /// 不建任务，只探测一次（文件名、大小、是否可续传）。供浏览器桥接在接管下载前确认链接可用。
    pub async fn probe_url(&self, url: &str, headers: &[(String, String)]) -> Result<ProbeInfo> {
        let url = parse_download_url(url)?;
        let network = self.inner.network.lock().unwrap().clone();
        if is_ftp(&url) {
            if !headers.is_empty() {
                return Err(Error::InvalidTaskUpdate("FTP 不接受 HTTP 请求头".into()));
            }
            let ftp = ftp::Runtime::new(&url, &self.ftp_options_for_url(&url)?, None, &network.policy)?;
            ftp.probe_with_retry(&network.policy, &CancellationToken::new()).await
        } else {
            probe_with_retry(&network, &url, &build_headers(headers)?, &CancellationToken::new()).await
        }
    }

    pub fn pause(&self, id: TaskId) -> Result<()> {
        let shared = {
            let tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
            if !entry.shared.mark_paused() {
                return Ok(());
            }
            entry.shared.set_speed(0.0);
            entry.cancel.cancel();
            entry.shared.clone()
        };
        persist(&self.inner, &shared);
        let _ = self.inner.events.send(Event::Changed(id));
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    pub fn resume(&self, id: TaskId) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        self.require_admission()?;
        let mut queues = self.inner.queues.lock().unwrap();
        let entry = tasks.get_mut(&id).ok_or(Error::TaskNotFound)?;
        if matches!(entry.shared.status(), TaskStatus::Paused | TaskStatus::Failed(_)) {
            if !queues.values().any(|q| q.task_ids.contains(&id)) {
                let mut updated = queues.clone();
                updated.get_mut(&MAIN_QUEUE_ID).ok_or(Error::QueueNotFound)?.task_ids.push(id);
                queues::persist_queues(&self.inner, &updated)?;
                *queues = updated;
            }
            entry.connections = self.connections();
            entry.shared.queue_again();
            persist(&self.inner, &entry.shared);
            self.inner.queue_wake.notify_one();
        }
        Ok(())
    }

    pub fn pause_all(&self) {
        let ids: Vec<TaskId> = self.inner.tasks.lock().unwrap().keys().copied().collect();
        for id in ids {
            let _ = self.pause(id);
        }
    }

    pub fn resume_all(&self) {
        let ids: Vec<TaskId> = self.inner.tasks.lock().unwrap().keys().copied().collect();
        for id in ids {
            let _ = self.resume(id);
        }
    }

    pub fn remove(&self, id: TaskId, delete_file: bool) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let mut queues = self.inner.queues.lock().unwrap();
        let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        if let Some(recovery) = &entry.shared.sync_state.lock().unwrap().recovery_path {
            match std::fs::symlink_metadata(recovery) {
                Ok(_) => {
                    return Err(Error::InvalidTaskUpdate(format!(
                        "发布恢复文件仍存在，请先恢复任务再移除：{}",
                        recovery.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        // 数据库事务失败时，不取消任务、不移除成员，保持 UI、运行状态和数据库一致。
        if let Some(store) = &self.inner.store {
            store.delete_task(id)?;
        }
        let entry = tasks.remove(&id).ok_or(Error::TaskNotFound)?;
        for queue in queues.values_mut() {
            queue.task_ids.retain(|member| *member != id);
        }
        drop(queues);
        drop(tasks);
        entry.cancel.cancel();
        self.inner.queue_wake.notify_one();
        // 等待同名处理的任务尚未拥有目标；覆盖下载的区段属于临时文件，
        // 发布成功之前也不能据此删除目标上的用户原件。
        let publication_unresolved = {
            let state = entry.shared.sync_state.lock().unwrap();
            state.overwrite || state.recovery_path.is_some()
        };
        let owns_target = entry.shared.status() != TaskStatus::FileConflict
            && !publication_unresolved
            && !entry.shared.segments.lock().unwrap().segments.is_empty();
        if delete_file && owns_target {
            let path = entry.shared.path();
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
        }
        let _ = self.inner.events.send(Event::Removed(id));
        Ok(())
    }

    pub fn task_properties(&self, id: TaskId) -> Result<TaskProperties> {
        let tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        Ok(TaskProperties {
            description: entry.shared.description.lock().unwrap().clone(),
            speed_limit: entry.shared.limiter.rate(),
        })
    }

    /// 描述和任务限速独立落盘；运行中的下载也立即采用新的任务限速。
    pub fn set_task_properties(&self, id: TaskId, properties: TaskProperties) -> Result<()> {
        if properties.description.chars().count() > 4096 || properties.speed_limit > i64::MAX as u64 {
            return Err(Error::InvalidTaskUpdate("描述不得超过 4096 字，限速数值超出范围".into()));
        }
        let tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        if let Some(store) = &self.inner.store {
            store.save_task_properties(id, &properties.description, properties.speed_limit)?;
        }
        *entry.shared.description.lock().unwrap() = properties.description;
        entry.shared.limiter.set_rate(properties.speed_limit);
        let _ = self.inner.events.send(Event::Changed(id));
        Ok(())
    }

    /// 仅等待或暂停/失败且旧运行完全退出的任务可换地址。改变 URL 默认移除所有旧请求头。
    /// 已有片段需在下次探测时通过强 ETag + 已知文件大小核验，否则从零开始。
    pub fn update_task_url(&self, id: TaskId, url: &str, headers: Option<Vec<(String, String)>>) -> Result<()> {
        let url = parse_download_url(url)?;
        let mut tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get_mut(&id).ok_or(Error::TaskNotFound)?;
        let status = entry.shared.status();
        if entry.running || !matches!(status, TaskStatus::Queued | TaskStatus::Paused | TaskStatus::Failed(_)) {
            return Err(Error::InvalidTaskUpdate("请先暂停任务并等待运行结束，再修改地址".into()));
        }
        let changed = entry.shared.url != url;
        let headers = headers.unwrap_or_else(|| if changed { Vec::new() } else { entry.shared.headers.clone() });
        if is_ftp(&url) && !headers.is_empty() {
            return Err(Error::InvalidTaskUpdate("FTP 不接受 HTTP 请求头".into()));
        }
        let header_map = build_headers(&headers)?;
        let mut rec = record_of(&entry.shared);
        rec.url = url.to_string();
        rec.headers = headers.clone();
        rec.ftp_options = if is_ftp(&url) {
            let mut options = rec.ftp_options.unwrap_or(FtpTaskOptions::for_url(url.as_str())?);
            if url.scheme() == "ftps" {
                options.tls = crate::FtpTlsMode::Implicit;
            }
            options.validate(url.as_str())?;
            Some(options)
        } else {
            None
        };
        let credentials = if changed { None } else { entry.shared.ftp_credentials.lock().unwrap().clone() };
        // 即使 URL 未变，新的认证/协商头也可能选择另一份实体。
        rec.source_changed |= changed || entry.shared.headers != headers;
        if let Some(store) = &self.inner.store {
            store.save_task(&rec)?;
        }
        let history = entry.shared.history.lock().unwrap().clone();
        entry.shared = Arc::new(TaskShared {
            id,
            url,
            headers,
            header_map,
            description: Mutex::new(rec.description),
            limiter: entry.shared.limiter.clone(),
            source_changed: AtomicBool::new(rec.source_changed),
            sync_state: Mutex::new(rec.sync_state),
            sync_probe: Mutex::new(None),
            ftp_options: Mutex::new(rec.ftp_options),
            ftp_credentials: Mutex::new(credentials),
            conflict_resolution: Mutex::new(None),
            conflict_target: Mutex::new(None),
            dir: rec.dir,
            file_name: Mutex::new(rec.file_name),
            user_named: rec.user_named,
            status: Mutex::new(RunStatus { status, generation: 0 }),
            run_lock: tokio::sync::Mutex::new(()),
            total: AtomicU64::new(rec.total.unwrap_or(0)),
            downloaded: AtomicU64::new(rec.downloaded),
            speed: AtomicU64::new(0),
            resumable: AtomicBool::new(rec.resumable),
            etag: Mutex::new(rec.etag),
            segments: Mutex::new(rec.segments),
            created_at: from_unix(rec.created_at),
            completed_at: Mutex::new(rec.completed_at.map(from_unix)),
            history: Mutex::new(history),
        });
        let _ = self.inner.events.send(Event::Changed(id));
        Ok(())
    }

    /// 全部任务快照，最新的在前
    pub fn snapshot(&self) -> Vec<TaskSnapshot> {
        let tasks = self.inner.tasks.lock().unwrap();
        tasks.values().rev().map(|e| snapshot_of(&e.shared)).collect()
    }

    pub fn snapshot_one(&self, id: TaskId) -> Option<TaskSnapshot> {
        let tasks = self.inner.tasks.lock().unwrap();
        tasks.get(&id).map(|e| snapshot_of(&e.shared))
    }

    /// 任务最近的每秒吞吐（字节/秒），旧的在前，最多 [`SPEED_HISTORY_LEN`] 个。
    /// 只记录下载中的时段；任务不存在时返回空。
    pub fn speed_history(&self, id: TaskId) -> Vec<f64> {
        let tasks = self.inner.tasks.lock().unwrap();
        tasks.get(&id).map(|e| e.shared.history.lock().unwrap().iter().copied().collect()).unwrap_or_default()
    }

    /// 所有活动任务速度之和（字节/秒）
    pub fn global_speed(&self) -> f64 {
        let tasks = self.inner.tasks.lock().unwrap();
        tasks.values().map(|e| e.shared.speed()).sum()
    }
}

fn is_ftp(url: &Url) -> bool {
    matches!(url.scheme(), "ftp" | "ftps")
}

fn parse_download_url(s: &str) -> Result<Url> {
    let url = Url::parse(s.trim())?;
    if is_ftp(&url) {
        swoop_ftp::FtpRequest::new(s.trim())?;
    } else if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::UnsupportedScheme(url.scheme().to_string()));
    }
    Ok(url)
}

fn snapshot_of(s: &TaskShared) -> TaskSnapshot {
    // 注意：不要在结构体字面量里直接 lock()，临时守卫会活到整个表达式结束，
    // 再调用 path() 去锁同一把锁就会自锁死。先取出来再组装。
    let file_name = s.file_name.lock().unwrap().clone();
    let path = s.dir.join(&file_name);
    let completed_at = *s.completed_at.lock().unwrap();
    let map = s.segments.lock().unwrap();
    TaskSnapshot {
        id: s.id,
        url: s.url.to_string(),
        file_name,
        path,
        total: s.total(),
        downloaded: s.downloaded.load(Ordering::Relaxed),
        speed_bps: s.speed(),
        status: s.status(),
        resumable: s.resumable.load(Ordering::Relaxed),
        segments: map
            .segments
            .iter()
            .map(|g| SegmentSnapshot { start: g.start, end: g.end, pos: g.pos, state: g.state })
            .collect(),
        active_connections: map.active_count(),
        created_at: s.created_at,
        completed_at,
    }
}

fn record_of(s: &TaskShared) -> TaskRecord {
    let file_name = s.file_name.lock().unwrap().clone();
    let etag = s.etag.lock().unwrap().clone();
    let completed_at = *s.completed_at.lock().unwrap();
    let segments = s.segments.lock().unwrap().clone();
    TaskRecord {
        id: s.id,
        url: s.url.to_string(),
        dir: s.dir.clone(),
        file_name,
        user_named: s.user_named,
        total: s.total(),
        resumable: s.resumable.load(Ordering::Relaxed),
        etag,
        status: s.status(),
        segments,
        downloaded: s.downloaded.load(Ordering::Relaxed),
        created_at: unix_secs(s.created_at),
        completed_at: completed_at.map(unix_secs),
        headers: s.headers.clone(),
        description: s.description.lock().unwrap().clone(),
        speed_limit: s.limiter.rate(),
        source_changed: s.source_changed.load(Ordering::Relaxed),
        sync_state: s.sync_state.lock().unwrap().clone(),
        ftp_options: s.ftp_options.lock().unwrap().clone(),
    }
}

/// 全量写入一次任务记录（有 store 时）
fn persist(inner: &Inner, shared: &TaskShared) {
    if let Some(store) = &inner.store
        && let Err(e) = store.save_task(&record_of(shared))
    {
        tracing::warn!(task = shared.id.0, "保存任务失败: {e}");
    }
}

async fn probe_with_retry(
    network: &NetworkRuntime,
    url: &Url,
    headers: &HeaderMap,
    cancel: &CancellationToken,
) -> Result<ProbeInfo> {
    let mut attempt = 0;
    loop {
        let result = tokio::select! {
            response = probe_network(network, url, headers) => response,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        match result {
            Ok(info) => return Ok(info),
            Err(error) if attempt < network.policy.retries && retryable(&error) => {
                attempt += 1;
                wait_retry(&network.policy, attempt, cancel).await?;
            }
            Err(error) => return Err(error),
        }
    }
}

struct RunNetwork {
    network: NetworkRuntime,
    ftp: Option<ftp::Runtime>,
}
impl std::ops::Deref for RunNetwork {
    type Target = NetworkRuntime;
    fn deref(&self) -> &Self::Target {
        &self.network
    }
}
impl RunNetwork {
    fn for_task(network: NetworkRuntime, shared: &TaskShared) -> Result<Self> {
        let ftp = shared
            .ftp_options
            .lock()
            .unwrap()
            .as_ref()
            .map(|options| {
                ftp::Runtime::new(&shared.url, options, shared.ftp_credentials.lock().unwrap().clone(), &network.policy)
            })
            .transpose()?;
        Ok(Self { network, ftp })
    }
}
async fn probe_task(network: &RunNetwork, shared: &TaskShared, cancel: &CancellationToken) -> Result<ProbeInfo> {
    if let Some(ftp) = &network.ftp {
        ftp.probe_with_retry(&network.policy, cancel).await
    } else {
        probe_with_retry(network, &shared.url, &shared.header_map, cancel).await
    }
}

fn spawn_run(inner: Arc<Inner>, shared: Arc<TaskShared>, cancel: CancellationToken, connections: usize) {
    let handle = inner.handle.clone();
    let generation = shared.begin_run();
    let network = Arc::new(inner.network.lock().unwrap().clone());
    handle.spawn(async move {
        let _run = shared.run_lock.lock().await;
        let id = shared.id;
        let result = match RunNetwork::for_task(network.as_ref().clone(), &shared) {
            Ok(network) => sync::run(&inner, &shared, cancel.clone(), connections, generation, Arc::new(network)).await,
            Err(error) => Err(error),
        };
        shared.set_speed(0.0);
        match result {
            Ok(changed) => {
                if shared.set_run_status(generation, TaskStatus::Completed) {
                    {
                        let mut state = shared.sync_state.lock().unwrap();
                        state.pending = false;
                        state.overwrite = false;
                    }
                    if changed {
                        *shared.completed_at.lock().unwrap() = Some(SystemTime::now());
                    }
                    persist(&inner, &shared);
                    let _ = inner.events.send(if changed { Event::Completed(id) } else { Event::Changed(id) });
                }
            }
            Err(Error::FileConflict) => {
                if shared.set_run_status(generation, TaskStatus::FileConflict) {
                    persist(&inner, &shared);
                    let _ = inner.events.send(Event::FileConflict(id));
                }
            }
            Err(Error::FileSkipped) => {
                if shared.set_run_status(generation, TaskStatus::Paused) {
                    persist(&inner, &shared);
                    let _ = inner.events.send(Event::Changed(id));
                }
            }
            Err(e) => {
                // run_lock 保证这里清理的还是旧运行的区段，新运行尚未创建 worker。
                shared.segments.lock().unwrap().reset_active();
                let failed = shared.set_run_status(generation, TaskStatus::Failed(e.to_string()));
                persist(&inner, &shared);
                if failed {
                    tracing::warn!(task = id.0, "下载失败: {e}");
                    let _ = inner.events.send(Event::Failed(id, e.to_string()));
                } else {
                    let _ = inner.events.send(Event::Changed(id));
                }
            }
        }
        let mut tasks = inner.tasks.lock().unwrap();
        if let Some(entry) = tasks.get_mut(&id) {
            entry.running = false;
            entry.run_queue = None;
        }
        inner.run_slots.lock().unwrap().remove(&id);
        drop(tasks);
        inner.queue_wake.notify_one();
    });
}

struct RunTarget {
    file: Option<File>,
    info: ProbeInfo,
    staging: bool,
}

async fn run_task(
    inner: &Arc<Inner>,
    shared: &Arc<TaskShared>,
    cancel: CancellationToken,
    connections: usize,
    generation: u64,
    network: Arc<RunNetwork>,
    target: RunTarget,
) -> Result<()> {
    let id = shared.id;
    let RunTarget { file: target, info, staging: sync_download } = target;
    if !shared.set_run_status(generation, TaskStatus::Probing) {
        return Err(Error::Cancelled);
    }
    let _ = inner.events.send(Event::Changed(id));

    tracing::info!(task = id.0, size = ?info.size, resumable = info.resumable, name = %info.file_name, "探测完成");

    // 在修改远端验证器或区段前确认目标。新任务原子占用文件名，已有进度绝不偷偷新建缺失文件。
    let file = match target {
        Some(file) => file,
        None => {
            std::fs::create_dir_all(&shared.dir)?;
            let map = shared.segments.lock().unwrap();
            let name = shared.file_name.lock().unwrap();
            if name.contains(['/', '\\']) {
                return Err(Error::InvalidTaskUpdate("文件名不能包含路径".into()));
            }
            {
                let file = open_resume_file(&shared.dir.join(&*name))?;
                let size = file.metadata()?.len();
                let required =
                    map.segments.iter().filter(|s| s.pos > 0).map(|s| s.start.saturating_add(s.pos)).max().unwrap_or(0);
                if size < required || shared.total().is_some_and(|total| size != total) {
                    return Err(Error::InvalidTaskUpdate("续传文件大小已变化，请重新添加下载".into()));
                }
                file
            }
        }
    };

    // 规划新实体会更新续传字段；写库失败时恢复它们，防止下一次重试将旧片段
    // 误认成新实体，或因失败规划的新大小而拒绝原本仍有效的本地文件。
    let previous = record_of(shared);
    // 续传校验：ETag 或大小变了，说明远端文件已更新，丢弃旧进度
    {
        let mut map = shared.segments.lock().unwrap();
        if !map.segments.is_empty() {
            let old_etag = shared.etag.lock().unwrap().clone();
            let etag_changed = matches!((&old_etag, &info.etag), (Some(a), Some(b)) if a != b);
            let size_changed = info.size.is_some() && shared.total() != info.size;
            let changed_source_unverified = shared.source_changed.load(Ordering::Relaxed)
                && !(info.size.is_some()
                    && shared.total() == info.size
                    && matches!((&old_etag, &info.etag), (Some(old), Some(new)) if old == new && old.starts_with('"') && old.ends_with('"')));
            if etag_changed || size_changed || changed_source_unverified {
                tracing::info!(task = id.0, "远端文件已变化，从头下载");
                *map = SegmentMap::default();
            }
        }
    }
    *shared.etag.lock().unwrap() = info.etag.clone();
    shared.source_changed.store(false, Ordering::Relaxed);
    shared.total.store(info.size.unwrap_or(0), Ordering::Relaxed);
    shared.resumable.store(info.resumable, Ordering::Relaxed);

    {
        let mut map = shared.segments.lock().unwrap();
        if map.segments.is_empty() || !info.resumable {
            // 非 Range 服务器无法从断点继续；每次新运行必须从头重建单段。
            *map = if sync_download || network.ftp.is_some() {
                SegmentMap::single_unknown()
            } else {
                match (info.size, info.resumable) {
                    (Some(size), true) => SegmentMap::plan(size, connections, inner.config.min_segment),
                    (Some(size), false) => SegmentMap::single_known(size),
                    (None, _) => SegmentMap::single_unknown(),
                }
            };
        } else {
            map.reset_active();
        }
        shared.downloaded.store(map.total_downloaded(), Ordering::Relaxed);
    }

    // 文件名、实体信息和初始区段是重启后判断文件归属的依据，不能只记录警告后
    // 继续写入。必须在扩容、截断及启动 worker 之前成功保存这份记录。
    let mut prepared_record = record_of(shared);
    prepared_record.status = TaskStatus::Downloading;
    if let Some(store) = &inner.store
        && let Err(error) = store.save_task(&prepared_record)
    {
        *shared.segments.lock().unwrap() = previous.segments;
        *shared.etag.lock().unwrap() = previous.etag;
        shared.total.store(previous.total.unwrap_or(0), Ordering::Relaxed);
        shared.downloaded.store(previous.downloaded, Ordering::Relaxed);
        shared.resumable.store(previous.resumable, Ordering::Relaxed);
        shared.source_changed.store(previous.source_changed, Ordering::Relaxed);
        return Err(error);
    }
    if !sync_download
        && let Some(size) = info.size
        && file.metadata()?.len() != size
    {
        file.set_len(size)?;
    } else if info.size.is_none() && shared.downloaded.load(Ordering::Relaxed) == 0 {
        // 新的未知大小实体可能比旧文件更短，不能在最终结果后残留旧尾部。
        file.set_len(0)?;
    }
    let file = Arc::new(file);

    if !shared.set_run_status(generation, TaskStatus::Downloading) || cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let _ = inner.events.send(Event::Changed(id));

    let workers = if info.resumable && info.size.is_some() { connections } else { 1 };
    let mut set = JoinSet::new();
    for _ in 0..workers {
        set.spawn(worker(inner.clone(), shared.clone(), file.clone(), cancel.clone(), network.clone()));
    }
    let ticker = inner.handle.spawn(speed_ticker(inner.clone(), shared.clone(), cancel.clone()));

    let mut result = Ok(());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if result.is_ok() {
                    result = Err(e);
                }
                cancel.cancel();
            }
            Err(e) => {
                if result.is_ok() {
                    result = Err(Error::Internal(format!("worker panic: {e}")));
                }
                cancel.cancel();
            }
        }
    }
    ticker.abort();

    result?;
    if !shared.segments.lock().unwrap().all_done() {
        return Err(Error::Incomplete);
    }
    file.sync_all()?;
    if let Some(total) = shared.total() {
        shared.downloaded.store(total, Ordering::Relaxed);
    }
    Ok(())
}

async fn worker(
    inner: Arc<Inner>,
    shared: Arc<TaskShared>,
    file: Arc<File>,
    cancel: CancellationToken,
    network: Arc<RunNetwork>,
) -> Result<()> {
    loop {
        let idx = shared.segments.lock().unwrap().next_work();
        let Some(idx) = idx else { return Ok(()) };

        let mut attempt = 0u32;
        loop {
            match download_segment(&inner, &shared, &file, idx, &cancel, &network).await {
                Ok(()) => break,
                Err(_) if cancel.is_cancelled() => return Err(Error::Cancelled),
                Err(Error::RangeNotSupported) => return Err(Error::RangeNotSupported),
                Err(e) => {
                    attempt += 1;
                    if attempt > network.policy.retries || !retryable(&e) {
                        return Err(e);
                    }
                    let backoff = network.policy.retry_delay(attempt);
                    tracing::debug!(task = shared.id.0, segment = idx, attempt, "区段出错，{backoff:?} 后重试: {e}");
                    wait_retry(&network.policy, attempt, &cancel).await?;
                }
            }
        }
    }
}

async fn download_segment(
    inner: &Inner,
    shared: &TaskShared,
    file: &File,
    idx: usize,
    cancel: &CancellationToken,
    network: &RunNetwork,
) -> Result<()> {
    if let Some(ftp) = &network.ftp {
        return ftp::download(inner, shared, file, idx, cancel, ftp).await;
    }
    let resumable = shared.resumable.load(Ordering::Relaxed);
    let (start, end) = {
        let mut map = shared.segments.lock().unwrap();
        let s = &mut map.segments[idx];
        if !resumable && s.pos > 0 {
            // 非 Range 重试得到的是完整响应，不能接到上次失败位置后面。
            s.pos = 0;
            shared.downloaded.store(0, Ordering::Relaxed);
        }
        (s.start + s.pos, s.end)
    };
    if end != u64::MAX && start >= end {
        shared.segments.lock().unwrap().segments[idx].state = SegmentState::Done;
        return Ok(());
    }

    let mut headers = shared.header_map.clone();
    let sync_probe = shared.sync_probe.lock().unwrap().clone();
    if let Some(info) = &sync_probe
        && shared.sync_state.lock().unwrap().pending
        && let Some(etag) = info.etag.as_ref().filter(|etag| crate::sync::strong_etag(etag))
    {
        headers.insert(
            header::IF_MATCH,
            header::HeaderValue::from_str(etag).map_err(|_| Error::UnsafeSync("远端 ETag 无效".into()))?,
        );
    }
    if resumable {
        let range = if end == u64::MAX { format!("bytes={start}-") } else { format!("bytes={start}-{}", end - 1) };
        headers.insert(header::RANGE, header::HeaderValue::from_str(&range).expect("引擎产生合法 Range"));
    }

    let mut resp = tokio::select! {
        r = network.get(&shared.url, &headers) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http(status.as_u16()));
    }
    if let Some(info) = &sync_probe {
        if shared.sync_state.lock().unwrap().pending {
            sync::validate_response(info, &resp, start, end)?;
        } else if !sync::validators_match(info, &resp)
            && let Some(probe) = shared.sync_probe.lock().unwrap().as_mut()
        {
            probe.etag = None;
            probe.last_modified = None;
        }
    }
    // 要求了 Range 却拿到 200：从 0 开始的区段可以当整段用，其他位置则无法续传
    if resumable && status != StatusCode::PARTIAL_CONTENT && start != 0 {
        return Err(Error::RangeNotSupported);
    }

    let mut pos = start;
    loop {
        let chunk = tokio::select! {
            c = resp.chunk() => c?,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let Some(chunk) = chunk else { break };

        write_chunk(inner, shared, file, idx, &chunk, &mut pos, cancel).await?;
        let reached_end = {
            let map = shared.segments.lock().unwrap();
            let seg = map.segments[idx];
            seg.end != u64::MAX && pos >= seg.end
        };
        if reached_end {
            break;
        }
    }

    let mut map = shared.segments.lock().unwrap();
    let seg = &mut map.segments[idx];
    if seg.end == u64::MAX {
        // 流式结束：此时才知道总大小
        seg.end = pos;
        shared.total.store(pos, Ordering::Relaxed);
    }
    if seg.is_done() {
        seg.state = SegmentState::Done;
        Ok(())
    } else {
        Err(Error::ShortRead)
    }
}

/// 协议共享的实际写入计费路径；等待期间不提前持久化尚未写入的字节。
async fn write_chunk(
    inner: &Inner,
    shared: &TaskShared,
    file: &File,
    idx: usize,
    chunk: &[u8],
    pos: &mut u64,
    cancel: &CancellationToken,
) -> Result<()> {
    // 先等待限速配额，再预订落盘范围。等待期间不推进区段位置，防止把尚未写入的
    // 字节持久化为断点；取消可以立即释放队列槽位，避免低速设置把停止操作卡住。
    let planned_take = {
        let map = shared.segments.lock().unwrap();
        let end = map.segments[idx].end;
        if end == u64::MAX { chunk.len() as u64 } else { (chunk.len() as u64).min(end.saturating_sub(*pos)) }
    };
    // 同一网络块可跨多个额度窗口写入，剩余字节必须保留，不能丢弃或重新下载。
    let mut offset = 0usize;
    while offset < planned_take as usize {
        let permit = inner.quota.acquire(planned_take as usize - offset, cancel).await?;
        if !shared.limiter.acquire_cancellable(permit.bytes(), cancel).await
            || !inner.limiter.acquire_cancellable(permit.bytes(), cancel).await
        {
            return Err(Error::Cancelled);
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut reached_segment_end = false;
        let result = permit.write(|allowed| {
            // 窗口等待期间可能发生窃取，在锁内重新核算并预订实际范围。
            let take = {
                let mut map = shared.segments.lock().unwrap();
                let seg = &mut map.segments[idx];
                let available = if seg.end == u64::MAX {
                    allowed as u64
                } else {
                    (allowed as u64).min(seg.end.saturating_sub(*pos))
                };
                seg.pos += available;
                available as usize
            };
            if take == 0 {
                reached_segment_end = true;
                return (0, Ok(()));
            }
            let (written, result) = write_at(file, &chunk[offset..offset + take], *pos);
            if written < take {
                shared.segments.lock().unwrap().segments[idx].pos -= (take - written) as u64;
            }
            *pos += written as u64;
            offset += written;
            shared.downloaded.fetch_add(written as u64, Ordering::Relaxed);
            (written, result)
        })?;
        if let Some(result) = result {
            result?;
        }
        if reached_segment_end {
            break;
        }
    }
    Ok(())
}

/// 每 250ms 计算一次速度；每 4 次记一个吞吐采样并刷新区段。
async fn speed_ticker(inner: Arc<Inner>, shared: Arc<TaskShared>, cancel: CancellationToken) {
    // 首个 tick 放在 250ms 后（interval 默认立即触发），让吞吐采样落在整秒上
    let period = Duration::from_millis(250);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    let mut last_bytes = shared.downloaded.load(Ordering::Relaxed);
    let mut last_t = Instant::now();
    // 上一个整秒采样点：曲线用真实吞吐而不是平滑后的 EWMA
    let (mut sec_bytes, mut sec_t) = (last_bytes, last_t);
    let mut ewma = 0.0f64;
    let mut tick = 0u32;
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = cancel.cancelled() => return,
        }
        let now = Instant::now();
        let bytes = shared.downloaded.load(Ordering::Relaxed);
        let dt = now.duration_since(last_t).as_secs_f64().max(1e-3);
        let inst = bytes.saturating_sub(last_bytes) as f64 / dt;
        ewma = if ewma == 0.0 { inst } else { ewma * 0.7 + inst * 0.3 };
        shared.set_speed(ewma);
        last_bytes = bytes;
        last_t = now;
        let _ = inner.events.send(Event::Changed(shared.id));

        tick += 1;
        if !tick.is_multiple_of(4) {
            continue;
        }
        let sec_dt = now.duration_since(sec_t).as_secs_f64().max(1e-3);
        shared.push_history(bytes.saturating_sub(sec_bytes) as f64 / sec_dt);
        (sec_bytes, sec_t) = (bytes, now);
        if let Some(store) = &inner.store {
            let segments = shared.segments.lock().unwrap().clone();
            if let Err(e) = store.save_progress(shared.id, &segments, bytes) {
                tracing::warn!(task = shared.id.0, "刷新进度失败: {e}");
            }
        }
    }
}
