//! 统一 Engine 的真实 FTP/FTPS、SQLite、文件发布与取消回归。
//! fixture 与 swoop-ftp 共用，仅监听回环地址，缺少 Python 依赖时直接失败。
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use swoop_core::{
    AddOptions, Config, Engine, Event, FileConflictPolicy, FtpCredentials, FtpProxyPolicy,
    FtpTaskOptions, FtpTlsMode, MAIN_QUEUE_ID, NetworkPolicy, ProxyConfig, QuotaPolicy, Store,
    TaskId, TaskProperties, TaskStatus,
};

struct Server {
    child: Child,
    root: tempfile::TempDir,
    port: u16,
    proxy: u16,
}
impl Server {
    fn start(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let python = std::env::var_os("SWOOP_FTP_TEST_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest.join("../../target/ftp-fixture-venv/bin/python"));
        let mut child = Command::new(python)
            .arg(manifest.join("../ftp/tests/server.py"))
            .arg(root.path())
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Install target-local FTP fixture dependencies first");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: serde_json::Value =
            serde_json::from_str(&line).expect("FTP fixture startup failed");
        Self {
            child,
            root,
            port: ready["port"].as_u64().unwrap() as u16,
            proxy: ready["http_proxy"].as_u64().unwrap() as u16,
        }
    }
    fn add(&self, dir: &Path, name: &str) -> AddOptions {
        AddOptions {
            url: format!("ftp://127.0.0.1:{}/{name}", self.port),
            dir: dir.into(),
            file_name: Some(name.into()),
            ..Default::default()
        }
    }
    fn options(&self) -> FtpTaskOptions {
        FtpTaskOptions {
            proxy: FtpProxyPolicy::Direct,
            ..Default::default()
        }
    }
    fn events(&self) -> String {
        std::fs::read_to_string(self.root.path().join("events.jsonl")).unwrap_or_default()
    }
    fn commands(&self, name: &str) -> usize {
        self.events()
            .lines()
            .filter(|line| *line == format!("\"{name}\""))
            .count()
    }
    fn body(&self) -> Vec<u8> {
        std::fs::read(self.root.path().join("payload.bin")).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut input) = self.child.stdin.take() {
            let _ = input.write_all(b"stop\n");
        }
        for _ in 0..100 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn engine(rt: &tokio::runtime::Runtime) -> Engine {
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            retries: 0,
            ..Default::default()
        },
    );
    engine
        .set_ftp_defaults(FtpTaskOptions {
            proxy: FtpProxyPolicy::Direct,
            ..Default::default()
        })
        .unwrap();
    engine
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(Instant::now() < end, "condition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn terminal(engine: &Engine, id: TaskId) -> TaskStatus {
    wait_until(|| {
        matches!(
            engine.snapshot_one(id).unwrap().status,
            TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::FileConflict
        )
    });
    released(engine, id);
    engine.snapshot_one(id).unwrap().status
}
fn completed(engine: &Engine, id: TaskId) {
    assert_eq!(terminal(engine, id), TaskStatus::Completed);
}
fn released(engine: &Engine, id: TaskId) {
    let path = engine.snapshot_one(id).unwrap().path;
    wait_until(|| {
        !engine.target_registry().is_reserved(&path).unwrap()
            && engine.queues().iter().all(|queue| queue.active_count == 0)
    });
}
fn pause(engine: &Engine, id: TaskId) {
    engine.pause(id).unwrap();
    released(engine, id);
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Paused);
}

#[test]
fn anonymous_and_empty_downloads_require_full_ftp_finish_and_are_single_connection() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    for name in ["payload.bin", "empty.bin"] {
        let id = engine.add(server.add(dir.path(), name)).unwrap();
        completed(&engine, id);
        let snapshot = engine.snapshot_one(id).unwrap();
        assert!(!snapshot.resumable);
        assert_eq!(snapshot.segments.len(), 1);
        assert_eq!(
            std::fs::read(snapshot.path).unwrap(),
            std::fs::read(server.root.path().join(name)).unwrap()
        );
    }
    assert_eq!(
        server.commands("RETR"),
        2,
        "empty file must still complete RETR"
    );
    assert_eq!(server.commands("REST-nonzero"), 0);
}

#[test]
fn final_error_and_truncated_body_never_emit_completed_or_server_secrets() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    let mut events = engine.subscribe();
    for name in ["final-error.bin", "short.bin"] {
        let id = engine.add(server.add(dir.path(), name)).unwrap();
        let TaskStatus::Failed(message) = terminal(&engine, id) else {
            panic!("accepted unsuccessful FTP transfer")
        };
        assert!(!message.contains("server-secret-leak"));
        assert!(!message.contains("fixture-password"));
    }
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, Event::Completed(_)));
    }
}

#[test]
fn explicit_and_implicit_tls_require_trusted_ca() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for (mode, tls) in [
        ("explicit", FtpTlsMode::Explicit),
        ("implicit", FtpTlsMode::Implicit),
    ] {
        let server = Server::start(mode);
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(&rt);
        let mut options = server.options();
        options.tls = tls;
        let mut add = server.add(dir.path(), "payload.bin");
        if tls == FtpTlsMode::Implicit {
            add.url = add.url.replacen("ftp:", "ftps:", 1);
        }
        let id = engine
            .enqueue_ftp(add, MAIN_QUEUE_ID, options.clone(), None)
            .unwrap();
        assert!(matches!(terminal(&engine, id), TaskStatus::Failed(_)));
        assert_eq!(server.commands("RETR"), 0);
        options.config.ca_certificate = Some(server.root.path().join("ca.pem"));
        engine.configure_ftp_task(id, options, None).unwrap();
        engine.resume(id).unwrap();
        completed(&engine, id);
        assert_eq!(
            std::fs::read(dir.path().join("payload.bin")).unwrap(),
            server.body()
        );
    }
}

#[test]
fn pause_cancels_quota_blocked_worker_and_restarts_without_trusting_size_or_mdtm() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 32 * 1024,
            window_secs: 3600,
        })
        .unwrap();
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    wait_until(|| engine.quota().waiting);
    let start = Instant::now();
    pause(&engine, id);
    assert!(start.elapsed() < Duration::from_secs(2));
    let partial = std::fs::read(dir.path().join("payload.bin")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        std::fs::read(dir.path().join("payload.bin")).unwrap(),
        partial
    );
    assert_eq!(engine.snapshot_one(id).unwrap().downloaded, 32 * 1024);
    let remote = server.root.path().join("payload.bin");
    let modified = std::fs::metadata(&remote).unwrap().modified().unwrap();
    let new = vec![99; 1024 * 1024];
    std::fs::write(&remote, &new).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&remote)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    engine.set_quota_policy(QuotaPolicy::default()).unwrap();
    engine.resume(id).unwrap();
    completed(&engine, id);
    assert_eq!(std::fs::read(dir.path().join("payload.bin")).unwrap(), new);
    assert_eq!(server.commands("REST-nonzero"), 0);
    assert_eq!(engine.quota().used_bytes, 32 * 1024 + 1024 * 1024);
}

#[test]
fn credentials_are_session_only_and_restart_never_falls_back_to_anonymous() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(
        rt.handle().clone(),
        Config {
            retries: 0,
            ..Default::default()
        },
        store.clone(),
    );
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let id = engine
        .enqueue_ftp(
            server.add(dir.path(), "payload.bin"),
            MAIN_QUEUE_ID,
            server.options(),
            Some(FtpCredentials::new("fixture-user", "fixture-password").unwrap()),
        )
        .unwrap();
    assert!(!engine.ftp_auth_required(id).unwrap());
    assert_eq!(
        store.load_tasks().unwrap()[0]
            .ftp_options
            .as_ref()
            .unwrap()
            .username
            .as_deref(),
        Some("fixture-user")
    );
    drop(engine);
    let restored = Engine::with_store(
        rt.handle().clone(),
        Config {
            retries: 0,
            ..Default::default()
        },
        store.clone(),
    );
    restored.load(true).unwrap();
    assert!(restored.ftp_auth_required(id).unwrap());
    assert!(matches!(
        restored.snapshot_one(id).unwrap().status,
        TaskStatus::Failed(_)
    ));
    restored.start_queue(MAIN_QUEUE_ID).unwrap();
    restored.resume(id).unwrap();
    assert!(
        matches!(terminal(&restored, id), TaskStatus::Failed(message) if message.contains("认证"))
    );
    assert_eq!(
        server.commands("USER"),
        0,
        "missing credentials must not open anonymous session"
    );
    let mut options = restored.ftp_task_options(id).unwrap().unwrap();
    options.config.buffer_chunks = 1;
    restored
        .configure_ftp_task(
            id,
            options,
            Some(FtpCredentials::new("fixture-user", "wrong-password").unwrap()),
        )
        .unwrap();
    restored.resume(id).unwrap();
    assert!(
        matches!(terminal(&restored, id), TaskStatus::Failed(message) if !message.contains("server-secret-leak"))
    );
    restored
        .set_ftp_credentials(
            id,
            Some(FtpCredentials::new("fixture-user", "fixture-password").unwrap()),
        )
        .unwrap();
    restored.resume(id).unwrap();
    completed(&restored, id);
    let records = format!("{:?}", store.load_tasks().unwrap());
    assert!(!records.contains("fixture-password"));
    assert!(!records.contains("wrong-password"));
    for name in ["state.db", "state.db-wal"] {
        let content = std::fs::read(dir.path().join(name)).unwrap_or_default();
        assert!(
            !content
                .windows(b"fixture-password".len())
                .any(|part| part == b"fixture-password")
        );
        assert!(
            !content
                .windows(b"wrong-password".len())
                .any(|part| part == b"wrong-password")
        );
    }
}

#[test]
fn defaults_and_atomic_configuration_only_change_after_sqlite_success() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let options = server.options();
    engine.set_ftp_defaults(options.clone()).unwrap();
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    assert_eq!(engine.ftp_task_options(id).unwrap(), Some(options.clone()));
    let mut private = options.clone();
    private.username = Some("fixture-user".into());
    assert!(engine.set_ftp_defaults(private.clone()).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_ftp BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT, 'ftp update rejected'); END;
        CREATE TRIGGER reject_ftp_default BEFORE INSERT ON settings WHEN NEW.key = 'ftp_defaults' BEGIN SELECT RAISE(ABORT, 'ftp default rejected'); END;").unwrap();
    assert!(
        engine
            .configure_ftp_task(
                id,
                private,
                Some(FtpCredentials::new("fixture-user", "fixture-password").unwrap())
            )
            .is_err()
    );
    assert_eq!(engine.ftp_task_options(id).unwrap(), Some(options.clone()));
    let mut changed = options.clone();
    changed.config.buffer_chunks = 1;
    assert!(engine.set_ftp_defaults(changed).is_err());
    assert_eq!(engine.ftp_defaults(), options);
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
    restored.load(false).unwrap();
    assert_eq!(restored.ftp_defaults(), options);
    let mut add = server.add(dir.path(), "payload.bin");
    add.url = add.url.replacen("ftp:", "ftps:", 1);
    let implicit = restored.add(add).unwrap();
    assert_eq!(
        restored.ftp_task_options(implicit).unwrap().unwrap().tls,
        FtpTlsMode::Implicit
    );
}

#[test]
fn stopped_queue_keeps_waiters_stopped_and_proxy_policy_is_explicit() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    let queue = engine.create_queue("ftp", 1).unwrap();
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 1024,
            window_secs: 3600,
        })
        .unwrap();
    let first = engine
        .enqueue(server.add(dir.path(), "payload.bin"), queue)
        .unwrap();
    let second = engine
        .enqueue(server.add(dir.path(), "empty.bin"), queue)
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.commands("USER"), 0);
    engine.start_queue(queue).unwrap();
    wait_until(|| engine.quota().waiting);
    assert_eq!(
        engine.snapshot_one(second).unwrap().status,
        TaskStatus::Queued
    );
    engine.stop_queue(queue).unwrap();
    released(&engine, first);
    let before = server.commands("USER");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(server.commands("USER"), before);
    assert_eq!(
        engine.snapshot_one(second).unwrap().status,
        TaskStatus::Queued
    );
    engine.set_quota_policy(QuotaPolicy::default()).unwrap();
    engine
        .set_network_policy(NetworkPolicy {
            proxy: Some(ProxyConfig {
                url: format!("http://127.0.0.1:{}", server.proxy),
                bypass: vec![],
            }),
            retries: 0,
            ..Default::default()
        })
        .unwrap();
    for id in [first, second] {
        let mut options = engine.ftp_task_options(id).unwrap().unwrap();
        options.proxy = FtpProxyPolicy::Network;
        engine.configure_ftp_task(id, options, None).unwrap();
    }
    engine.start_queue(queue).unwrap();
    completed(&engine, first);
    completed(&engine, second);
    assert!(server.events().contains("http-tunnel"));
}

#[test]
fn automatic_system_proxy_is_not_silently_bypassed() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    assert!(
        matches!(terminal(&engine, id), TaskStatus::Failed(message) if message.contains("系统代理"))
    );
    assert_eq!(server.commands("USER"), 0);
}

#[test]
fn failed_database_preparation_never_truncates_or_starts_ftp_download() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.set_ftp_defaults(server.options()).unwrap();
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 16384,
            window_secs: 3600,
        })
        .unwrap();
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    wait_until(|| engine.quota().waiting);
    pause(&engine, id);
    let original = std::fs::read(dir.path().join("payload.bin")).unwrap();
    let progress = engine.snapshot_one(id).unwrap().downloaded;
    let requests = server.commands("RETR");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_download BEFORE UPDATE ON tasks WHEN NEW.status = 'downloading' BEGIN SELECT RAISE(ABORT, 'ftp prepare rejected'); END;").unwrap();
    std::fs::write(
        server.root.path().join("payload.bin"),
        vec![1; 2 * 1024 * 1024],
    )
    .unwrap();
    engine.set_quota_policy(QuotaPolicy::default()).unwrap();
    engine.resume(id).unwrap();
    assert!(
        matches!(terminal(&engine, id), TaskStatus::Failed(message) if message.contains("ftp prepare rejected"))
    );
    assert_eq!(server.commands("RETR"), requests);
    assert_eq!(
        std::fs::read(dir.path().join("payload.bin")).unwrap(),
        original
    );
    assert_eq!(engine.snapshot_one(id).unwrap().downloaded, progress);
    assert_eq!(store.load_tasks().unwrap()[0].downloaded, progress);
}

#[test]
fn cancelled_or_failed_overwrite_preserves_original_and_success_uses_safe_publication() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    let path = dir.path().join("payload.bin");
    std::fs::write(&path, b"owned by user").unwrap();
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::FileConflict);
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 16384,
            window_secs: 3600,
        })
        .unwrap();
    engine
        .resolve_file_conflict(id, FileConflictPolicy::Overwrite)
        .unwrap();
    wait_until(|| engine.quota().waiting);
    pause(&engine, id);
    assert_eq!(std::fs::read(&path).unwrap(), b"owned by user");
    std::fs::write(&path, b"user changed original while paused").unwrap();
    engine.set_quota_policy(QuotaPolicy::default()).unwrap();
    engine.resume(id).unwrap();
    assert!(matches!(terminal(&engine, id), TaskStatus::Failed(_)));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"user changed original while paused"
    );
    engine.remove(id, false).unwrap();
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::FileConflict);
    engine
        .resolve_file_conflict(id, FileConflictPolicy::Overwrite)
        .unwrap();
    completed(&engine, id);
    assert_eq!(std::fs::read(&path).unwrap(), server.body());
}

#[test]
fn sync_downloads_full_content_when_size_and_mdtm_do_not_change() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    completed(&engine, id);
    let mut events = engine.subscribe();
    let remote = server.root.path().join("payload.bin");
    let modified = std::fs::metadata(&remote).unwrap().modified().unwrap();
    std::fs::write(&remote, vec![42; 1024 * 1024]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&remote)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(engine.sync_queue(MAIN_QUEUE_ID).unwrap(), 1);
    completed(&engine, id);
    assert_eq!(
        std::fs::read(dir.path().join("payload.bin")).unwrap(),
        vec![42; 1024 * 1024]
    );
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, Event::Completed(task) if task == id))
    );
    let before = server.commands("RETR");
    assert_eq!(engine.sync_queue(MAIN_QUEUE_ID).unwrap(), 1);
    completed(&engine, id);
    assert_eq!(server.commands("RETR"), before + 1);
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, Event::Completed(_)))
    );
}

#[test]
fn task_and_global_limits_apply_to_ftp_chunks() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine.set_speed_limit(256 * 1024);
    let id = engine.add(server.add(dir.path(), "payload.bin")).unwrap();
    engine
        .set_task_properties(
            id,
            TaskProperties {
                speed_limit: 128 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
    let start = Instant::now();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    wait_until(|| engine.snapshot_one(id).unwrap().downloaded >= 128 * 1024);
    assert!(start.elapsed() >= Duration::from_millis(600));
    pause(&engine, id);
    engine
        .set_task_properties(id, TaskProperties::default())
        .unwrap();
    let start = Instant::now();
    engine.resume(id).unwrap();
    completed(&engine, id);
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert_eq!(
        std::fs::read(dir.path().join("payload.bin")).unwrap(),
        server.body()
    );
}

#[test]
fn transient_final_failure_retries_from_zero_and_bills_repeated_writes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    engine
        .set_network_policy(NetworkPolicy {
            retries: 1,
            retry_delay_ms: 1,
            retry_max_delay_ms: 1,
            ..Default::default()
        })
        .unwrap();
    let id = engine
        .add(server.add(dir.path(), "retry-once.bin"))
        .unwrap();
    completed(&engine, id);
    assert_eq!(server.commands("RETR"), 2);
    assert_eq!(server.commands("REST-nonzero"), 0);
    assert_eq!(engine.quota().used_bytes, 2 * 1024 * 1024);
    assert_eq!(
        std::fs::read(dir.path().join("retry-once.bin")).unwrap(),
        server.body()
    );
}

#[test]
fn cancelling_a_delayed_probe_releases_queue_before_any_retr() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start("plain");
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    let id = engine
        .add(server.add(dir.path(), "delayed-probe.bin"))
        .unwrap();
    wait_until(|| server.events().contains("probe-waiting"));
    let start = Instant::now();
    pause(&engine, id);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(server.commands("RETR"), 0);
    assert!(!dir.path().join("delayed-probe.bin").exists());
}

#[test]
fn stored_ftp_options_reject_corruption_and_urls_never_accept_credentials() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    for url in [
        "ftp://alice:secret@127.0.0.1/file",
        "ftp://alice@127.0.0.1/file",
        "ftp://@127.0.0.1/file",
    ] {
        let error = engine
            .add(AddOptions {
                url: url.into(),
                dir: dir.path().into(),
                ..Default::default()
            })
            .unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
    let add = AddOptions {
        url: "ftp://127.0.0.1/file".into(),
        dir: dir.path().into(),
        ..Default::default()
    };
    let mut headers = add.clone();
    headers.headers = vec![("Authorization".into(), "secret".into())];
    assert!(engine.add(headers).is_err());
    let id = engine.add(add).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE tasks SET ftp_options = '{broken' WHERE id = ?1",
        [i64::try_from(id.0).unwrap()],
    )
    .unwrap();
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
    assert!(restored.load(true).is_err());
    assert!(restored.snapshot_one(id).is_none());
}

struct HttpServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HttpServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            for socket in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut socket) = socket else { break };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 262144\r\nETag: \"http-file\"\r\nConnection: close\r\n\r\n");
                let _ = socket.write_all(&vec![7; 256 * 1024]);
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn http_and_ftp_share_the_same_aggregate_quota_and_cancel_without_losing_capacity() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ftp = Server::start("plain");
    let http = HttpServer::start();
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&rt);
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 65536,
            window_secs: 3600,
        })
        .unwrap();
    let ftp_id = engine.add(ftp.add(dir.path(), "payload.bin")).unwrap();
    let http_id = engine
        .add(AddOptions {
            url: format!("http://{}/http.bin", http.address),
            dir: dir.path().into(),
            file_name: Some("http.bin".into()),
            ..Default::default()
        })
        .unwrap();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    wait_until(|| {
        engine.quota().used_bytes == 65536
            && engine.snapshot_one(ftp_id).unwrap().status == TaskStatus::Downloading
            && engine.snapshot_one(http_id).unwrap().status == TaskStatus::Downloading
    });
    assert_eq!(
        engine.snapshot_one(ftp_id).unwrap().downloaded
            + engine.snapshot_one(http_id).unwrap().downloaded,
        65536
    );
    // 停队列会取消两个协议；实际 worker 清理完成后活动槽和配额预留均必须释放。
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    released(&engine, ftp_id);
    let already_written = engine.quota().used_bytes;
    engine.set_quota_policy(QuotaPolicy::default()).unwrap();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    completed(&engine, ftp_id);
    completed(&engine, http_id);
    assert_eq!(
        engine.quota().used_bytes,
        already_written + 1024 * 1024 + 256 * 1024
    );
    assert_eq!(
        std::fs::read(dir.path().join("payload.bin")).unwrap(),
        ftp.body()
    );
    assert_eq!(
        std::fs::read(dir.path().join("http.bin")).unwrap(),
        vec![7; 256 * 1024]
    );
}
