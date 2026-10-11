//! 任务调度与分段下载 worker。

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header;
use reqwest::{Client, StatusCode};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::{Error, Result};
use crate::limiter::Limiter;
use crate::probe::probe;
use crate::segment::{SegmentMap, SegmentState};
use crate::store::{Store, TaskRecord};
use crate::task::{SegmentSnapshot, TaskId, TaskSnapshot, TaskStatus};
use crate::util::{unique_file_name, write_at};

/// 每个任务保留的速度采样数（每秒一个），即速度曲线覆盖的秒数
pub const SPEED_HISTORY_LEN: usize = 120;

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

#[derive(Clone, Debug)]
pub struct AddOptions {
    pub url: String,
    pub dir: PathBuf,
    /// 不填则由探测结果决定
    pub file_name: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Changed(TaskId),
    Completed(TaskId),
    Failed(TaskId, String),
    Removed(TaskId),
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    handle: Handle,
    client: Client,
    tasks: Mutex<BTreeMap<TaskId, Entry>>,
    next_id: AtomicU64,
    events: broadcast::Sender<Event>,
    limiter: Arc<Limiter>,
    config: Config,
    store: Option<Arc<Store>>,
}

struct Entry {
    shared: Arc<TaskShared>,
    cancel: CancellationToken,
}

struct TaskShared {
    id: TaskId,
    url: Url,
    dir: PathBuf,
    file_name: Mutex<String>,
    user_named: bool,
    status: Mutex<TaskStatus>,
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
    fn set_status(&self, s: TaskStatus) {
        *self.status.lock().unwrap() = s;
    }
    fn status(&self) -> TaskStatus {
        self.status.lock().unwrap().clone()
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
        let client = Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(config.connect_timeout)
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("构建 HTTP client 失败");
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                handle,
                client,
                tasks: Mutex::new(BTreeMap::new()),
                next_id: AtomicU64::new(1),
                events,
                limiter: Arc::new(Limiter::new(config.speed_limit)),
                config,
                store,
            }),
        }
    }

    pub fn store(&self) -> Option<&Arc<Store>> {
        self.inner.store.as_ref()
    }

    /// 从数据库恢复任务。`auto_resume` 为 true 时，上次仍在下载的任务立即继续；否则恢复为已暂停。
    /// 返回恢复的任务数。
    pub fn load(&self, auto_resume: bool) -> Result<usize> {
        let Some(store) = &self.inner.store else { return Ok(0) };
        let records = store.load_tasks()?;
        let mut count = 0;
        let mut to_resume = Vec::new();
        for rec in records {
            let Ok(url) = Url::parse(&rec.url) else {
                tracing::warn!(id = rec.id.0, "跳过无效 URL 的任务: {}", rec.url);
                continue;
            };
            let was_active = rec.status.is_active();
            let status = match rec.status {
                TaskStatus::Completed => TaskStatus::Completed,
                TaskStatus::Failed(e) => TaskStatus::Failed(e),
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
                dir: rec.dir,
                file_name: Mutex::new(rec.file_name),
                user_named: rec.user_named,
                status: Mutex::new(status),
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
            self.inner
                .tasks
                .lock()
                .unwrap()
                .insert(rec.id, Entry { shared, cancel: CancellationToken::new() });
            self.inner.next_id.fetch_max(rec.id.0 + 1, Ordering::Relaxed);
            if was_active && auto_resume {
                to_resume.push(rec.id);
            }
            count += 1;
        }
        for id in to_resume {
            let _ = self.resume(id);
        }
        Ok(count)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn speed_limit(&self) -> u64 {
        self.inner.limiter.rate()
    }

    pub fn set_speed_limit(&self, bps: u64) {
        self.inner.limiter.set_rate(bps);
    }

    pub fn add(&self, opts: AddOptions) -> Result<TaskId> {
        let url = Url::parse(opts.url.trim())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Internal(format!("暂不支持的协议: {}", url.scheme())));
        }
        let created_at = SystemTime::now();
        let user_named = opts.file_name.is_some();
        let file_name = opts.file_name.unwrap_or_default();

        let id = match &self.inner.store {
            Some(store) => store.insert_task(&TaskRecord {
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
            })?,
            None => TaskId(self.inner.next_id.fetch_add(1, Ordering::Relaxed)),
        };

        let shared = Arc::new(TaskShared {
            id,
            url,
            dir: opts.dir,
            user_named,
            file_name: Mutex::new(file_name),
            status: Mutex::new(TaskStatus::Queued),
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
        self.inner
            .tasks
            .lock()
            .unwrap()
            .insert(id, Entry { shared: shared.clone(), cancel: cancel.clone() });
        spawn_run(self.inner.clone(), shared, cancel);
        Ok(id)
    }

    pub fn pause(&self, id: TaskId) -> Result<()> {
        let shared = {
            let tasks = self.inner.tasks.lock().unwrap();
            let entry = tasks.get(&id).ok_or(Error::TaskNotFound)?;
            if !entry.shared.status().is_active() {
                return Ok(());
            }
            entry.shared.set_status(TaskStatus::Paused);
            entry.shared.set_speed(0.0);
            entry.cancel.cancel();
            entry.shared.clone()
        };
        persist(&self.inner, &shared);
        let _ = self.inner.events.send(Event::Changed(id));
        Ok(())
    }

    pub fn resume(&self, id: TaskId) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get_mut(&id).ok_or(Error::TaskNotFound)?;
        if matches!(entry.shared.status(), TaskStatus::Paused | TaskStatus::Failed(_)) {
            entry.cancel = CancellationToken::new();
            entry.shared.set_status(TaskStatus::Queued);
            spawn_run(self.inner.clone(), entry.shared.clone(), entry.cancel.clone());
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
        let entry = self.inner.tasks.lock().unwrap().remove(&id).ok_or(Error::TaskNotFound)?;
        entry.cancel.cancel();
        if delete_file {
            let path = entry.shared.path();
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
        }
        if let Some(store) = &self.inner.store
            && let Err(e) = store.delete_task(id)
        {
            tracing::warn!(task = id.0, "删除任务记录失败: {e}");
        }
        let _ = self.inner.events.send(Event::Removed(id));
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

fn spawn_run(inner: Arc<Inner>, shared: Arc<TaskShared>, cancel: CancellationToken) {
    let handle = inner.handle.clone();
    handle.spawn(async move {
        let id = shared.id;
        let result = run_task(&inner, &shared, cancel.clone()).await;
        shared.set_speed(0.0);
        match result {
            Ok(()) => {
                shared.set_status(TaskStatus::Completed);
                *shared.completed_at.lock().unwrap() = Some(SystemTime::now());
                persist(&inner, &shared);
                let _ = inner.events.send(Event::Completed(id));
            }
            Err(_) if shared.status() == TaskStatus::Paused => {
                // 用户主动暂停，状态已在 pause() 里设置
                shared.segments.lock().unwrap().reset_active();
                persist(&inner, &shared);
                let _ = inner.events.send(Event::Changed(id));
            }
            Err(e) => {
                tracing::warn!(task = id.0, "下载失败: {e}");
                shared.segments.lock().unwrap().reset_active();
                shared.set_status(TaskStatus::Failed(e.to_string()));
                persist(&inner, &shared);
                let _ = inner.events.send(Event::Failed(id, e.to_string()));
            }
        }
    });
}

async fn run_task(inner: &Arc<Inner>, shared: &Arc<TaskShared>, cancel: CancellationToken) -> Result<()> {
    let id = shared.id;
    shared.set_status(TaskStatus::Probing);
    let _ = inner.events.send(Event::Changed(id));

    let info = tokio::select! {
        r = probe(&inner.client, &shared.url) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    tracing::info!(task = id.0, size = ?info.size, resumable = info.resumable, name = %info.file_name, "探测完成");

    // 续传校验：ETag 或大小变了，说明远端文件已更新，丢弃旧进度
    {
        let mut map = shared.segments.lock().unwrap();
        if !map.segments.is_empty() {
            let old_etag = shared.etag.lock().unwrap().clone();
            let etag_changed = matches!((&old_etag, &info.etag), (Some(a), Some(b)) if a != b);
            let size_changed = info.size.is_some() && shared.total() != info.size;
            if etag_changed || size_changed {
                tracing::info!(task = id.0, "远端文件已变化，从头下载");
                *map = SegmentMap::default();
            }
        }
    }
    *shared.etag.lock().unwrap() = info.etag.clone();

    if let Some(size) = info.size {
        shared.total.store(size, Ordering::Relaxed);
    }
    shared.resumable.store(info.resumable, Ordering::Relaxed);

    std::fs::create_dir_all(&shared.dir)?;
    {
        let mut name = shared.file_name.lock().unwrap();
        if name.is_empty() {
            *name = if shared.user_named { info.file_name.clone() } else { unique_file_name(&shared.dir, &info.file_name) };
        }
    }

    {
        let mut map = shared.segments.lock().unwrap();
        if map.segments.is_empty() {
            *map = match (info.size, info.resumable) {
                (Some(size), true) => SegmentMap::plan(size, inner.config.connections, inner.config.min_segment),
                (Some(size), false) => SegmentMap::single_known(size),
                (None, _) => SegmentMap::single_unknown(),
            };
        } else {
            map.reset_active();
        }
        shared.downloaded.store(map.total_downloaded(), Ordering::Relaxed);
    }

    let path = shared.path();
    let file = OpenOptions::new().create(true).write(true).read(true).truncate(false).open(&path)?;
    if let Some(size) = info.size
        && file.metadata()?.len() != size
    {
        file.set_len(size)?;
    }
    let file = Arc::new(file);

    shared.set_status(TaskStatus::Downloading);
    persist(inner, shared);
    let _ = inner.events.send(Event::Changed(id));

    let workers = if info.resumable && info.size.is_some() { inner.config.connections.max(1) } else { 1 };
    let mut set = JoinSet::new();
    for _ in 0..workers {
        set.spawn(worker(inner.clone(), shared.clone(), file.clone(), cancel.clone()));
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

async fn worker(inner: Arc<Inner>, shared: Arc<TaskShared>, file: Arc<File>, cancel: CancellationToken) -> Result<()> {
    loop {
        let idx = shared.segments.lock().unwrap().next_work();
        let Some(idx) = idx else { return Ok(()) };

        let mut attempt = 0u32;
        loop {
            match download_segment(&inner, &shared, &file, idx, &cancel).await {
                Ok(()) => break,
                Err(_) if cancel.is_cancelled() => return Err(Error::Cancelled),
                Err(Error::RangeNotSupported) => return Err(Error::RangeNotSupported),
                Err(e) => {
                    attempt += 1;
                    if attempt > inner.config.retries {
                        return Err(e);
                    }
                    let backoff = Duration::from_millis(300 * 2u64.pow(attempt.min(5)));
                    tracing::debug!(task = shared.id.0, segment = idx, attempt, "区段出错，{backoff:?} 后重试: {e}");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = cancel.cancelled() => return Err(Error::Cancelled),
                    }
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
) -> Result<()> {
    let (start, end) = {
        let map = shared.segments.lock().unwrap();
        let s = map.segments[idx];
        (s.start + s.pos, s.end)
    };
    if end != u64::MAX && start >= end {
        shared.segments.lock().unwrap().segments[idx].state = SegmentState::Done;
        return Ok(());
    }

    let resumable = shared.resumable.load(Ordering::Relaxed);
    let mut req = inner.client.get(shared.url.clone());
    if resumable {
        let range = if end == u64::MAX { format!("bytes={start}-") } else { format!("bytes={start}-{}", end - 1) };
        req = req.header(header::RANGE, range);
    }

    let mut resp = tokio::select! {
        r = req.send() => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http(status.as_u16()));
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

        // 先在锁内"预订"本次写入范围，避免与窃取方重叠
        let take = {
            let mut map = shared.segments.lock().unwrap();
            let seg = &mut map.segments[idx];
            let cur_end = seg.end;
            let take = if cur_end == u64::MAX {
                chunk.len() as u64
            } else {
                (chunk.len() as u64).min(cur_end.saturating_sub(pos))
            };
            seg.pos += take;
            take
        };
        if take > 0 {
            inner.limiter.acquire(take as usize).await;
            if let Err(e) = write_at(file, &chunk[..take as usize], pos) {
                let mut map = shared.segments.lock().unwrap();
                map.segments[idx].pos -= take;
                return Err(e.into());
            }
            pos += take;
            shared.downloaded.fetch_add(take, Ordering::Relaxed);
        }
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

/// 每 250ms 计算一次速度；每 4 次（1 秒）记一个吞吐采样，并把区段表刷到数据库
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
        if tick % 4 != 0 {
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
