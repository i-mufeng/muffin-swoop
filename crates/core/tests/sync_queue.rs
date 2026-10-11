//! 真正的可变 HTTP 服务：同步检查/暂存替换/故障保留/取消/重启/并发与额度。
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use swoop_core::{
    AddOptions, Config, Engine, MAIN_QUEUE_ID, QueueClock, QueueSchedule, QuotaPolicy, Store, TaskId, TaskStatus,
};

#[derive(Clone)]
struct Response {
    bytes: Vec<u8>,
    etag: Option<String>,
    modified: Option<String>,
    download_etag: Option<String>,
    fail: bool,
    short: bool,
    omit_length: bool,
    probe_size: Option<usize>,
    delay: Duration,
    probe_gate: Option<Arc<ProbeGate>>,
}
impl Response {
    fn version(byte: u8) -> Self {
        Self {
            bytes: vec![byte; 64 * 1024],
            etag: Some(format!("\"v{byte}\"")),
            modified: None,
            download_etag: None,
            fail: false,
            short: false,
            omit_length: false,
            probe_size: None,
            delay: Duration::ZERO,
            probe_gate: None,
        }
    }
}
#[derive(Default)]
struct ProbeGate {
    entered: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}
impl ProbeGate {
    fn wait(&self) -> std::io::Result<()> {
        let released = self.released.lock().unwrap();
        self.entered.store(true, Ordering::SeqCst);
        let (_released, timeout) =
            self.wake.wait_timeout_while(released, Duration::from_secs(5), |released| !*released).unwrap();
        if timeout.timed_out() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "probe gate was not released"));
        }
        Ok(())
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

struct Server {
    url: String,
    response: Arc<Mutex<Response>>,
    probes: Arc<AtomicUsize>,
    downloads: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}
impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let response = Arc::new(Mutex::new(Response::version(1)));
        let probes = Arc::new(AtomicUsize::new(0));
        let downloads = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (r, p, d, a, m) = (response.clone(), probes.clone(), downloads.clone(), active.clone(), peak.clone());
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(socket) = socket else { return };
                let (r, p, d, a, m) = (r.clone(), p.clone(), d.clone(), a.clone(), m.clone());
                std::thread::spawn(move || {
                    let mut socket = socket;
                    let _ = serve(&mut socket, &r, &p, &d, &a, &m);
                });
            }
        });
        Self { url, response, probes, downloads, active, peak }
    }
    fn set(&self, response: Response) {
        *self.response.lock().unwrap() = response;
    }
}
fn serve(
    socket: &mut TcpStream,
    state: &Mutex<Response>,
    probes: &AtomicUsize,
    downloads: &AtomicUsize,
    active: &AtomicUsize,
    peak: &AtomicUsize,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(socket.try_clone()?);
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            let (a, b) = value.trim().split_once('-').unwrap();
            range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().ok()));
        }
    }
    let response = state.lock().unwrap().clone();
    let is_probe = range == Some((0, Some(0)));
    if is_probe {
        probes.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &response.probe_gate {
            gate.wait()?;
        }
    } else {
        downloads.fetch_add(1, Ordering::SeqCst);
    }
    let mut headers = String::new();
    let etag =
        if is_probe { response.etag.as_ref() } else { response.download_etag.as_ref().or(response.etag.as_ref()) };
    if let Some(etag) = etag {
        headers += &format!("ETag: {etag}\r\n");
    }
    if let Some(modified) = &response.modified {
        headers += &format!("Last-Modified: {modified}\r\n");
    }
    if response.fail {
        write!(socket, "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
    let (start, end) = range
        .map(|(a, b)| (a, b.unwrap_or(response.bytes.len() - 1).min(response.bytes.len() - 1)))
        .unwrap_or((0, response.bytes.len() - 1));
    if range.is_some() {
        write!(
            socket,
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            response.probe_size.unwrap_or(response.bytes.len()),
            end - start + 1
        )?;
    } else {
        let length =
            if response.omit_length { String::new() } else { format!("Content-Length: {}\r\n", response.bytes.len()) };
        write!(socket, "HTTP/1.1 200 OK\r\n{headers}{length}Connection: close\r\n\r\n")?;
    }
    let bytes = &response.bytes[start..=end];
    let bytes = if response.short && !is_probe { &bytes[..bytes.len() / 2] } else { bytes };
    if is_probe {
        socket.write_all(bytes)?;
        return Ok(());
    }
    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
    peak.fetch_max(current, Ordering::SeqCst);
    let result = (|| {
        for chunk in bytes.chunks(4096) {
            if !response.delay.is_zero() {
                std::thread::sleep(response.delay);
            }
            socket.write_all(chunk)?;
        }
        Ok(())
    })();
    active.fetch_sub(1, Ordering::SeqCst);
    result
}

fn engine(store: Option<Arc<Store>>, clock: Arc<dyn QueueClock>) -> Engine {
    let engine = Engine::with_queue_clock(
        tokio::runtime::Handle::current(),
        Config { connections: 1, min_segment: 4096, retries: 0, ..Default::default() },
        store,
        clock,
    );
    let mut policy = engine.network_policy();
    policy.use_system_proxy = false;
    engine.set_network_policy(policy).unwrap();
    engine
}
struct Clock(AtomicI64);
impl QueueClock for Clock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.0.load(Ordering::SeqCst) as u64)
    }
}
fn clock() -> Arc<Clock> {
    Arc::new(Clock(AtomicI64::new(1000)))
}
async fn idle(engine: &Engine, id: TaskId) -> TaskStatus {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let status = engine.snapshot_one(id).unwrap().status;
            if !status.is_active() && engine.queues().iter().all(|q| q.active_count == 0) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("task did not finish")
}
async fn completed(engine: &Engine, id: TaskId) {
    assert_eq!(idle(engine, id).await, TaskStatus::Completed);
}
async fn downloaded(engine: &Engine, server: &Server, path: &Path) -> TaskId {
    let id = engine.add(AddOptions { url: server.url.clone(), dir: path.into(), ..Default::default() }).unwrap();
    completed(engine, id).await;
    id
}
async fn transferring(engine: &Engine, id: TaskId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = engine.snapshot_one(id).unwrap();
            if task.status == TaskStatus::Downloading && task.downloaded > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn assert_no_temps(path: &Path) {
    assert!(
        !std::fs::read_dir(path)
            .unwrap()
            .any(|e| { e.unwrap().file_name().to_string_lossy().starts_with(".swoop-sync-") })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_does_not_download_or_repeat_completed_event_and_change_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let before = server.downloads.load(Ordering::SeqCst);
    let mut events = engine.subscribe();
    let original_time = engine.snapshot_one(id).unwrap().completed_at;
    assert_eq!(engine.sync_queue(MAIN_QUEUE_ID).unwrap(), 1);
    completed(&engine, id).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), before);
    assert_eq!(engine.snapshot_one(id).unwrap().completed_at, original_time);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, swoop_core::Event::Completed(_)));
    }
    server.set(Response::version(2));
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![2; 65536]);
    assert_eq!(server.downloads.load(Ordering::SeqCst), before + 1);
    assert_no_temps(dir.path());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_and_retry_keep_old_file_until_verified_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let path = engine.snapshot_one(id).unwrap().path;
    let mut broken = Response::version(2);
    broken.short = true;
    server.set(broken);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(&path).unwrap(), vec![1; 65536]);
    assert_no_temps(dir.path());
    server.set(Response::version(2));
    engine.resume(id).unwrap();
    completed(&engine, id).await;
    assert_eq!(std::fs::read(path).unwrap(), vec![2; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_and_user_edit_during_download_preserve_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let path = engine.snapshot_one(id).unwrap().path;
    let mut changed = Response::version(2);
    changed.delay = Duration::from_millis(20);
    server.set(changed.clone());
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    transferring(&engine, id).await;
    engine.pause(id).unwrap();
    assert_eq!(idle(&engine, id).await, TaskStatus::Paused);
    assert_eq!(std::fs::read(&path).unwrap(), vec![1; 65536]);
    assert_no_temps(dir.path());
    assert_eq!(engine.sync_queue(MAIN_QUEUE_ID).unwrap(), 0, "manual per-task pause survives sync cycle");
    engine.resume(id).unwrap();
    transferring(&engine, id).await;
    std::fs::write(&path, vec![8; 65536]).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(&path).unwrap(), vec![8; 65536]);
    assert_no_temps(dir.path());
    let before = server.probes.load(Ordering::SeqCst);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert_eq!(server.probes.load(Ordering::SeqCst), before, "edited file rejected before network");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_destination_is_never_followed_or_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let path = engine.snapshot_one(id).unwrap().path;
    let other = dir.path().join("user.bin");
    std::fs::write(&other, vec![1; 65536]).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    server.set(Response::version(2));
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(other).unwrap(), vec![1; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cycle_and_restart_persist_sync_mode_and_owned_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let time = clock();
    let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
    let first = engine(Some(store.clone()), time.clone());
    let id = downloaded(&first, &server, dir.path()).await;
    first.set_queue_sync_enabled(MAIN_QUEUE_ID, true).unwrap();
    first
        .set_queue_schedule(
            MAIN_QUEUE_ID,
            Some(QueueSchedule { start_at: Some(1060), stop_at: None, repeat_every_secs: Some(60) }),
        )
        .unwrap();
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    let second = engine(Some(store.clone()), time.clone());
    second.load(true).unwrap();
    assert!(second.queues()[0].sync_enabled);
    assert!(store.load_tasks().unwrap()[0].sync_state.baseline.is_some());
    server.set(Response::version(3));
    time.0.store(1060, Ordering::SeqCst);
    second.tick_queues().unwrap();
    completed(&second, id).await;
    assert_eq!(std::fs::read(second.snapshot_one(id).unwrap().path).unwrap(), vec![3; 65536]);
    let mut bad = Response::version(4);
    bad.fail = true;
    server.set(bad);
    time.0.store(1120, Ordering::SeqCst);
    second.tick_queues().unwrap();
    assert!(matches!(idle(&second, id).await, TaskStatus::Failed(_)));
    server.set(Response::version(4));
    time.0.store(1180, Ordering::SeqCst);
    second.tick_queues().unwrap();
    completed(&second, id).await;
    assert_eq!(std::fs::read(second.snapshot_one(id).unwrap().path).unwrap(), vec![4; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_modified_without_etag_and_no_validator_fallback_are_safe() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let mut response = Response::version(1);
    response.etag = None;
    response.modified = Some("Sun, 11 Oct 2026 12:00:00 GMT".into());
    server.set(response.clone());
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let count = server.downloads.load(Ordering::SeqCst);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), count);
    response.bytes = vec![2; 65536];
    response.modified = Some("Sun, 11 Oct 2026 12:00:01 GMT".into());
    server.set(response.clone());
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![2; 65536]);
    response.modified = None;
    server.set(response);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), count + 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_larger_than_probe_with_no_content_length_is_not_truncated_to_success() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let mut response = Response::version(2);
    response.probe_size = Some(32000);
    response.omit_length = true;
    server.set(response);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![1; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_quota_holds_replacement_and_stop_keeps_waiting_members_parked() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let time = clock();
    let engine = engine(None, time.clone());
    engine.set_file_conflict_policy(swoop_core::FileConflictPolicy::Rename).unwrap();
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 1).unwrap();
    let a = downloaded(&engine, &server, dir.path()).await;
    let b = downloaded(&engine, &server, dir.path()).await;
    engine.set_quota_policy(QuotaPolicy { max_bytes: 2 * 65536 + 8192, window_secs: 60 }).unwrap();
    let mut response = Response::version(2);
    response.delay = Duration::from_millis(3);
    server.set(response);
    server.peak.store(0, Ordering::SeqCst);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    transferring(&engine, a).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.quota().used_bytes, 2 * 65536 + 8192);
    assert_eq!(engine.snapshot_one(b).unwrap().status, TaskStatus::Queued);
    assert_eq!(std::fs::read(engine.snapshot_one(a).unwrap().path).unwrap(), vec![1; 65536]);
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.queues()[0].active_count != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let requests = server.downloads.load(Ordering::SeqCst);
    time.0.store(1060, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), requests);
    assert_eq!(server.active.load(Ordering::SeqCst), 0);
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, a).await;
    completed(&engine, b).await;
    assert_eq!(std::fs::read(engine.snapshot_one(a).unwrap().path).unwrap(), vec![2; 65536]);
    assert_eq!(std::fs::read(engine.snapshot_one(b).unwrap().path).unwrap(), vec![2; 65536]);
    assert_eq!(server.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_completed_records_without_proof_are_not_adopted() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let store = Arc::new(Store::open(&dir.path().join("legacy.db")).unwrap());
    let first = engine(Some(store.clone()), clock());
    let id = downloaded(&first, &server, dir.path()).await;
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    let mut rec = store.load_tasks().unwrap().remove(0);
    rec.sync_state = Default::default();
    store.save_task(&rec).unwrap();
    let second = engine(Some(store), clock());
    second.load(true).unwrap();
    server.set(Response::version(2));
    second.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&second, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(second.snapshot_one(id).unwrap().path).unwrap(), vec![1; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_validator_change_between_probe_and_body_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let mut response = Response::version(2);
    response.download_etag = Some("\"v3\"".into());
    server.set(response);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    assert!(matches!(idle(&engine, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![1; 65536]);
    assert_no_temps(dir.path());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_sync_can_resume_safely_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
    let first = engine(Some(store.clone()), clock());
    let id = downloaded(&first, &server, dir.path()).await;
    let path = first.snapshot_one(id).unwrap().path;
    let mut response = Response::version(2);
    response.delay = Duration::from_millis(20);
    server.set(response);
    first.sync_queue(MAIN_QUEUE_ID).unwrap();
    transferring(&first, id).await;
    first.pause(id).unwrap();
    assert_eq!(idle(&first, id).await, TaskStatus::Paused);
    assert_eq!(std::fs::read(&path).unwrap(), vec![1; 65536]);
    assert!(store.load_tasks().unwrap()[0].sync_state.pending);
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    let second = engine(Some(store), clock());
    second.load(false).unwrap();
    assert_eq!(second.snapshot_one(id).unwrap().status, TaskStatus::Paused);
    server.set(Response::version(2));
    second.resume(id).unwrap();
    completed(&second, id).await;
    assert_eq!(std::fs::read(path).unwrap(), vec![2; 65536]);
    assert_no_temps(dir.path());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_response_mismatching_probe_is_not_a_trusted_remote_validator() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let mut response = Response::version(1);
    response.bytes = vec![2; 65536];
    response.download_etag = Some("\"v2\"".into());
    server.set(response);
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![2; 65536]);
    server.set(Response::version(1));
    let count = server.downloads.load(Ordering::SeqCst);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), count + 1);
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), vec![1; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_rejecting_publish_intent_keeps_old_file_and_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("intent.db");
    let server = Server::start();
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = engine(Some(store.clone()), clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let path = engine.snapshot_one(id).unwrap().path;
    let previous = store.load_tasks().unwrap()[0].sync_state.baseline.clone().unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_intent BEFORE UPDATE ON tasks WHEN json_extract(NEW.sync_state,'$.prepared') IS NOT NULL BEGIN SELECT RAISE(ABORT,'intent rejected'); END;").unwrap();
    server.set(Response::version(2));
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    let status = idle(&engine, id).await;
    assert!(matches!(status,TaskStatus::Failed(ref e) if e.contains("intent rejected")), "{status:?}");
    assert_eq!(std::fs::read(&path).unwrap(), vec![1; 65536]);
    assert_no_temps(dir.path());
    let state = store.load_tasks().unwrap()[0].sync_state.clone();
    assert_eq!(state.baseline.unwrap().sha256, previous.sha256);
    assert!(state.prepared.is_none());
    assert!(state.pending);
    connection.execute_batch("DROP TRIGGER reject_intent").unwrap();
    engine.resume(id).unwrap();
    completed(&engine, id).await;
    assert_eq!(std::fs::read(path).unwrap(), vec![2; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persisted_publish_intent_recovers_after_final_state_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("finish.db");
    let server = Server::start();
    let store = Arc::new(Store::open(&db).unwrap());
    let first = engine(Some(store.clone()), clock());
    let id = downloaded(&first, &server, dir.path()).await;
    let path = first.snapshot_one(id).unwrap().path;
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_finish BEFORE UPDATE ON tasks WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'finish rejected'); END;").unwrap();
    server.set(Response::version(2));
    first.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&first, id).await;
    assert_eq!(std::fs::read(&path).unwrap(), vec![2; 65536]);
    let state = store.load_tasks().unwrap()[0].sync_state.clone();
    assert!(state.pending);
    assert!(state.prepared.is_some());
    assert_ne!(state.baseline.unwrap().sha256, state.prepared.unwrap().sha256);
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    connection.execute_batch("DROP TRIGGER reject_finish").unwrap();
    let count = server.downloads.load(Ordering::SeqCst);
    let second = engine(Some(store.clone()), clock());
    second.load(true).unwrap();
    completed(&second, id).await;
    assert_eq!(
        server.downloads.load(Ordering::SeqCst),
        count,
        "published candidate must recover without another body download"
    );
    assert_eq!(std::fs::read(path).unwrap(), vec![2; 65536]);
    let state = store.load_tasks().unwrap()[0].sync_state.clone();
    assert!(!state.pending);
    assert!(state.prepared.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn intent_recovery_never_rolls_back_over_a_users_post_publish_edit() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("edited.db");
    let server = Server::start();
    let store = Arc::new(Store::open(&db).unwrap());
    let first = engine(Some(store.clone()), clock());
    let id = downloaded(&first, &server, dir.path()).await;
    let path = first.snapshot_one(id).unwrap().path;
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_finish BEFORE UPDATE ON tasks WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'finish rejected'); END;").unwrap();
    server.set(Response::version(2));
    first.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&first, id).await;
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    std::fs::write(&path, vec![9; 65536]).unwrap();
    connection.execute_batch("DROP TRIGGER reject_finish").unwrap();
    let second = engine(Some(store), clock());
    second.load(true).unwrap();
    assert!(matches!(idle(&second, id).await, TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(path).unwrap(), vec![9; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_but_unpublished_intent_recovers_from_old_owned_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let store = Arc::new(Store::open(&dir.path().join("prepared.db")).unwrap());
    let first = engine(Some(store.clone()), clock());
    let id = downloaded(&first, &server, dir.path()).await;
    let path = first.snapshot_one(id).unwrap().path;
    drop(first);
    tokio::time::sleep(Duration::from_millis(270)).await;
    let mut record = store.load_tasks().unwrap().remove(0);
    let mut candidate = record.sync_state.baseline.clone().unwrap();
    candidate.sha256 = "0".repeat(64);
    candidate.etag = Some("\"v2\"".into());
    record.sync_state.prepared = Some(candidate);
    record.sync_state.pending = true;
    record.status = TaskStatus::Queued;
    store.save_task(&record).unwrap();
    server.set(Response::version(2));
    let second = engine(Some(store), clock());
    second.load(true).unwrap();
    completed(&second, id).await;
    assert_eq!(std::fs::read(path).unwrap(), vec![2; 65536]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn moved_original_recovers_from_durable_journal_without_clobbering_late_user_save() {
    for user_save in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start();
        let store = Arc::new(Store::open(&dir.path().join("recovery.db")).unwrap());
        let first = engine(Some(store.clone()), clock());
        let id = downloaded(&first, &server, dir.path()).await;
        let path = first.snapshot_one(id).unwrap().path;
        tokio::time::timeout(Duration::from_secs(3), async {
            while first.target_registry().is_reserved(&path).unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let mut record = store.load_tasks().unwrap().remove(0);
        let mut candidate = record.sync_state.baseline.clone().unwrap();
        candidate.sha256 = "0".repeat(64);
        candidate.etag = Some("\"v2\"".into());
        let recovery = std::fs::canonicalize(dir.path()).unwrap().join(".swoop-recovery-interrupted");
        std::fs::rename(&path, &recovery).unwrap();
        record.sync_state.prepared = Some(candidate);
        record.sync_state.recovery_path = Some(recovery.clone());
        record.sync_state.pending = true;
        record.status = TaskStatus::Queued;
        store.save_task(&record).unwrap();
        drop(first);
        if user_save {
            std::fs::write(&path, vec![9; 65536]).unwrap();
        }
        server.set(Response::version(2));
        let second = engine(Some(store.clone()), clock());
        second.load(true).unwrap();
        if user_save {
            assert!(matches!(idle(&second, id).await, TaskStatus::Failed(_)));
            assert_eq!(std::fs::read(&path).unwrap(), vec![9; 65536]);
            assert_eq!(std::fs::read(&recovery).unwrap(), vec![1; 65536]);
            assert!(second.remove(id, false).unwrap_err().to_string().contains(recovery.to_str().unwrap()));
        } else {
            completed(&second, id).await;
            assert_eq!(std::fs::read(&path).unwrap(), vec![2; 65536]);
            assert!(!recovery.exists());
            assert!(store.load_tasks().unwrap()[0].sync_state.recovery_path.is_none());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_body_without_validators_does_not_replace_or_repeat_completion() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let mut response = Response::version(1);
    response.etag = None;
    server.set(response);
    let engine = engine(None, clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let original = engine.snapshot_one(id).unwrap();
    let metadata = std::fs::metadata(&original.path).unwrap();
    let mut events = engine.subscribe();
    let requests = server.downloads.load(Ordering::SeqCst);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, id).await;
    assert_eq!(server.downloads.load(Ordering::SeqCst), requests + 1);
    assert_eq!(engine.snapshot_one(id).unwrap().completed_at, original.completed_at);
    assert_eq!(std::fs::metadata(original.path).unwrap().modified().unwrap(), metadata.modified().unwrap());
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, swoop_core::Event::Completed(_)));
    }
    assert_no_temps(dir.path());
}

async fn local_change_while_unchanged_probe_waits(delete: bool) {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start();
    let store = Arc::new(Store::in_memory().unwrap());
    let engine = engine(Some(store.clone()), clock());
    let id = downloaded(&engine, &server, dir.path()).await;
    let path = engine.snapshot_one(id).unwrap().path;
    let original = store.load_tasks().unwrap()[0].sync_state.baseline.clone().unwrap();
    let downloads = server.downloads.load(Ordering::SeqCst);
    let mut events = engine.subscribe();
    let gate = Arc::new(ProbeGate::default());
    let mut response = Response::version(1);
    response.probe_gate = Some(gate.clone());
    server.set(response);
    engine.sync_queue(MAIN_QUEUE_ID).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !gate.entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("sync probe did not reach the gate");
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Probing);
    if delete {
        std::fs::remove_file(&path).unwrap();
    } else {
        std::fs::write(&path, vec![9; 65536]).unwrap();
    }
    gate.release();
    let status = idle(&engine, id).await;
    assert!(matches!(status, TaskStatus::Failed(_)), "local change during unchanged probe was accepted: {status:?}");
    if delete {
        assert!(!path.exists(), "deleted target must not be restored");
    } else {
        assert_eq!(std::fs::read(path).unwrap(), vec![9; 65536]);
    }
    assert_eq!(server.downloads.load(Ordering::SeqCst), downloads);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, swoop_core::Event::Completed(_)));
    }
    let record = store.load_tasks().unwrap().remove(0);
    assert!(matches!(record.status, TaskStatus::Failed(_)));
    assert!(record.sync_state.pending, "failed check must remain retryable");
    assert_eq!(record.sync_state.baseline.unwrap().sha256, original.sha256);
    assert!(record.sync_state.prepared.is_none());
    assert_no_temps(dir.path());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_probe_rejects_local_edits_during_network_wait() {
    local_change_while_unchanged_probe_waits(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_probe_rejects_local_deletion_during_network_wait() {
    local_change_while_unchanged_probe_waits(true).await;
}
