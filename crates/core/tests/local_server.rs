//! 用一个支持 Range 的最小 HTTP 服务器验证引擎端到端：多连接、动态分段、校验内容。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use swoop_core::{AddOptions, Config, Engine, MAX_CONNECTIONS, TaskStatus};

fn make_body(len: usize) -> Vec<u8> {
    // 确定性伪随机，便于逐字节校验
    let mut v = Vec::with_capacity(len);
    let mut x: u32 = 0x9E37_79B9;
    for _ in 0..len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v.push((x & 0xFF) as u8);
    }
    v
}

struct Server {
    url: String,
    range_requests: Arc<AtomicUsize>,
    /// 收到的每个请求的全部头部行（小写、去掉行尾）
    requests: Arc<Mutex<Vec<Vec<String>>>>,
}

#[derive(Clone)]
struct ServerOpts {
    support_range: bool,
    file_name: String,
    /// 每发 64 KB 停顿多久，用来控制单连接的速度
    chunk_delay: Duration,
    /// 设置后，请求必须带这一行头（小写，如 `cookie: token=1`），否则 403
    require: Option<String>,
    etag: Option<String>,
}

fn start_server(body: Arc<Vec<u8>>, support_range: bool, file_name: &str) -> Server {
    start_paced_server(body, support_range, file_name, Duration::from_millis(2))
}

fn start_paced_server(body: Arc<Vec<u8>>, support_range: bool, file_name: &str, chunk_delay: Duration) -> Server {
    start_server_with(
        body,
        ServerOpts { support_range, file_name: file_name.to_string(), chunk_delay, require: None, etag: None },
    )
}

fn start_server_with(body: Arc<Vec<u8>>, opts: ServerOpts) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let url = format!("http://{addr}/files/{}", opts.file_name);
    let (counter2, requests2) = (counter.clone(), requests.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let (body, opts) = (body.clone(), opts.clone());
            let (counter, requests) = (counter2.clone(), requests2.clone());
            std::thread::spawn(move || handle(stream, body, &opts, &counter, &requests));
        }
    });
    Server { url, range_requests: counter, requests }
}

fn handle(
    mut stream: TcpStream,
    body: Arc<Vec<u8>>,
    opts: &ServerOpts,
    counter: &AtomicUsize,
    requests: &Mutex<Vec<Vec<String>>>,
) {
    let (support_range, file_name, chunk_delay) = (opts.support_range, opts.file_name.as_str(), opts.chunk_delay);
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut range: Option<(u64, u64)> = None;
    let mut lines = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        lines.push(lower.trim_end().to_string());
        if let Some(v) = lower.strip_prefix("range: bytes=") {
            let v = v.trim();
            let (a, b) = v.split_once('-').unwrap();
            let start: u64 = a.parse().unwrap();
            let end: u64 = if b.is_empty() { body.len() as u64 - 1 } else { b.parse().unwrap() };
            range = Some((start, end));
        }
    }
    let authorized = opts.require.as_ref().is_none_or(|r| lines.contains(r));
    requests.lock().unwrap().push(lines);
    if !authorized {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let total = body.len() as u64;
    let (status, start, end) = match (range, support_range) {
        (Some((s, e)), true) => {
            counter.fetch_add(1, Ordering::Relaxed);
            ("206 Partial Content", s, e.min(total - 1))
        }
        _ => ("200 OK", 0, total - 1),
    };
    let slice = &body[start as usize..=end as usize];
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"{file_name}\"\r\nConnection: close\r\n",
        slice.len()
    );
    if support_range {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    if let Some(etag) = &opts.etag {
        head.push_str(&format!("ETag: {etag}\r\n"));
    }
    if status.starts_with("206") {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    // 分小块慢速发送，制造连接完成时间差，触发动态分段
    for piece in slice.chunks(64 * 1024) {
        if stream.write_all(piece).is_err() {
            return;
        }
        std::thread::sleep(chunk_delay);
    }
    let _ = stream.flush();
}

fn wait_done(engine: &Engine, id: swoop_core::TaskId, timeout: Duration) -> TaskStatus {
    let t0 = Instant::now();
    loop {
        let snap = engine.snapshot_one(id).expect("task exists");
        match &snap.status {
            TaskStatus::Completed | TaskStatus::Failed(_) => return snap.status,
            _ => {}
        }
        assert!(t0.elapsed() < timeout, "超时，状态: {:?}", snap.status);
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn multi_connection_download_matches_source() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(3 * 1024 * 1024 + 777));
    let server = start_server(body.clone(), true, "data.bin");
    let dir = tempfile::tempdir().unwrap();

    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 6, min_segment: 128 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();

    let status = wait_done(&engine, id, Duration::from_secs(30));
    assert_eq!(status, TaskStatus::Completed);

    let snap = engine.snapshot_one(id).unwrap();
    assert_eq!(snap.file_name, "data.bin");
    assert_eq!(snap.total, Some(body.len() as u64));
    assert_eq!(snap.downloaded, body.len() as u64);
    assert!(snap.segments.len() >= 6, "应有多段，实际 {}", snap.segments.len());

    let mut got = Vec::new();
    std::fs::File::open(dir.path().join("data.bin")).unwrap().read_to_end(&mut got).unwrap();
    assert_eq!(got.len(), body.len());
    assert!(got == *body, "内容不一致");
    // 探测 1 次 + 各区段请求
    assert!(server.range_requests.load(Ordering::Relaxed) >= 7);
}

#[test]
fn falls_back_to_single_connection_without_range() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(512 * 1024 + 13));
    let server = start_server(body.clone(), false, "plain.bin");
    let dir = tempfile::tempdir().unwrap();

    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 64 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);

    let snap = engine.snapshot_one(id).unwrap();
    assert_eq!(snap.segments.len(), 1);
    let got = std::fs::read(dir.path().join("plain.bin")).unwrap();
    assert!(got == *body);
}

/// 桥接先 probe_url 再 add_probed：引擎不再重复探测，链接只多被请求一次
#[test]
fn add_probed_reuses_probe_without_another_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(256 * 1024 + 5));
    let server = start_server(body.clone(), false, "once.bin");
    let dir = tempfile::tempdir().unwrap();

    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    let info = rt.block_on(engine.probe_url(&server.url, &[])).unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    let options = AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), ..Default::default() };
    let id = engine.add_probed(options, info).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);

    assert_eq!(server.requests.lock().unwrap().len(), 2, "探测 1 次 + 下载 1 次");
    assert!(std::fs::read(dir.path().join("once.bin")).unwrap() == *body);
}

/// 持久化：任务完成后用同一个 store 重建引擎，记录应完整恢复
#[test]
fn persisted_tasks_survive_restart() {
    use swoop_core::Store;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(700 * 1024));
    let server = start_server(body.clone(), true, "keep.bin");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swoop.db");

    {
        let store = Arc::new(Store::open(&db).unwrap());
        let engine = Engine::with_store(
            rt.handle().clone(),
            Config { connections: 3, min_segment: 64 * 1024, ..Config::default() },
            store,
        );
        let id = engine
            .add(AddOptions {
                url: server.url.clone(),
                dir: dir.path().to_path_buf(),
                file_name: None,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);
    }

    // "重启"
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store);
    assert_eq!(engine.load(true).unwrap(), 1);
    let tasks = engine.snapshot();
    assert_eq!(tasks.len(), 1);
    let t = &tasks[0];
    assert_eq!(t.status, TaskStatus::Completed);
    assert_eq!(t.file_name, "keep.bin");
    assert_eq!(t.total, Some(body.len() as u64));
    assert_eq!(t.downloaded, body.len() as u64);
    assert!(t.completed_at.is_some());

    // 再添加一个任务，id 必须延续而不是和旧任务冲突
    engine.set_file_conflict_policy(swoop_core::FileConflictPolicy::Rename).unwrap();
    let id2 = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();
    assert_ne!(id2, t.id);
    assert_eq!(wait_done(&engine, id2, Duration::from_secs(30)), TaskStatus::Completed);
    assert_eq!(engine.snapshot_one(id2).unwrap().file_name, "keep (1).bin");
}

/// 持久化 + 自动续传：下载中途"退出"，重启后从数据库里的断点继续并完成
#[test]
fn interrupted_download_resumes_after_restart() {
    use swoop_core::Store;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_server(body.clone(), true, "resume2.bin");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swoop.db");

    let id = {
        let store = Arc::new(Store::open(&db).unwrap());
        let engine = Engine::with_store(
            rt.handle().clone(),
            Config { connections: 4, min_segment: 128 * 1024, ..Config::default() },
            store,
        );
        let id = engine
            .add(AddOptions {
                url: server.url.clone(),
                dir: dir.path().to_path_buf(),
                file_name: None,
                ..Default::default()
            })
            .unwrap();
        let t0 = Instant::now();
        loop {
            let s = engine.snapshot_one(id).unwrap();
            if s.downloaded > 256 * 1024 || t0.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // 模拟进程被杀：暂停让进度落盘，然后丢弃引擎（状态仍记为 paused）
        engine.pause(id).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let snap = engine.snapshot_one(id).unwrap();
        assert!(snap.downloaded < body.len() as u64);
        id
    };

    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(
        rt.handle().clone(),
        Config { connections: 4, min_segment: 128 * 1024, ..Config::default() },
        store,
    );
    engine.load(true).unwrap();
    // 暂停态不会自动续传，手动继续
    let before = engine.snapshot_one(id).unwrap();
    assert_eq!(before.status, TaskStatus::Paused);
    assert!(before.downloaded > 0, "应恢复出断点进度");
    engine.resume(id).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);
    let got = std::fs::read(dir.path().join("resume2.bin")).unwrap();
    assert!(got == *body);
}

#[test]
fn pause_and_resume_completes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_server(body.clone(), true, "resume.bin");
    let dir = tempfile::tempdir().unwrap();

    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 128 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();

    // 等它下一点再暂停
    let t0 = Instant::now();
    loop {
        let s = engine.snapshot_one(id).unwrap();
        if s.downloaded > 64 * 1024 || t0.elapsed() > Duration::from_secs(10) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    engine.pause(id).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let paused = engine.snapshot_one(id).unwrap();
    assert_eq!(paused.status, TaskStatus::Paused);
    assert!(paused.downloaded < body.len() as u64);

    engine.resume(id).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);
    let got = std::fs::read(dir.path().join("resume.bin")).unwrap();
    assert!(got == *body);
}

/// 速度曲线：下载中每秒记一个吞吐采样，完成后历史仍可读
#[test]
fn speed_history_samples_each_second() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    // 单连接、每 64 KB 停 40ms（约 1.6 MB/s），4 MB 大约下 2.6 秒，能采到 2 个整秒点
    let body = Arc::new(make_body(4 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "curve.bin", Duration::from_millis(40));
    let dir = tempfile::tempdir().unwrap();
    let pace = 64.0 * 1024.0 / 0.040;

    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    let id = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);

    let snap = engine.snapshot_one(id).unwrap();
    assert!(snap.resumable);
    let history = engine.speed_history(id);
    assert!(history.len() >= 2, "应至少有 2 个采样，实际 {history:?}");
    assert!(history.len() <= swoop_core::SPEED_HISTORY_LEN);
    assert!(history.iter().all(|v| v.is_finite() && *v >= 0.0));
    let peak = history.iter().copied().fold(0.0, f64::max);
    assert!(peak > 0.0);
    assert!(peak < pace * 1.5, "峰值 {peak} 不应超过服务器发送速度");

    assert!(engine.speed_history(swoop_core::TaskId(9999)).is_empty());
}

/// 浏览器带来的 Cookie / User-Agent：探测和所有区段请求都要带上，否则受保护的链接会 403
#[test]
fn custom_headers_reach_every_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(900 * 1024));
    let server = start_server_with(
        body.clone(),
        ServerOpts {
            support_range: true,
            file_name: "secret.bin".into(),
            chunk_delay: Duration::from_millis(2),
            require: Some("cookie: token=1".into()),
            etag: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 3, min_segment: 64 * 1024, ..Config::default() });

    // 不带 Cookie：探测就 403，任务失败
    let denied = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), ..Default::default() })
        .unwrap();
    match wait_done(&engine, denied, Duration::from_secs(30)) {
        TaskStatus::Failed(e) => assert!(e.contains("403"), "{e}"),
        s => panic!("应失败，实际 {s:?}"),
    }

    let headers = vec![
        ("Cookie".to_string(), "token=1".to_string()),
        ("User-Agent".to_string(), "TestBrowser/1.0".to_string()),
        // 调用方传的 Range 必须被忽略，否则会和区段请求冲突
        ("Range".to_string(), "bytes=10-".to_string()),
    ];
    let info = rt.block_on(engine.probe_url(&server.url, &headers)).unwrap();
    assert_eq!(info.file_name, "secret.bin");
    assert_eq!(info.size, Some(body.len() as u64));
    assert!(info.resumable);
    assert!(rt.block_on(engine.probe_url(&server.url, &[])).is_err());

    let before = server.requests.lock().unwrap().len();
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().join("ok"), headers, ..Default::default() })
        .unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);
    assert!(std::fs::read(dir.path().join("ok/secret.bin")).unwrap() == *body);

    let reqs = server.requests.lock().unwrap();
    let ours = &reqs[before..];
    assert!(ours.len() >= 2, "探测 + 至少一个区段请求");
    for lines in ours {
        assert!(lines.iter().any(|l| l == "user-agent: testbrowser/1.0"), "UA 应被覆盖: {lines:?}");
        assert_eq!(lines.iter().filter(|l| l.starts_with("user-agent:")).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.starts_with("range:")).count(), 1);
        assert!(!lines.iter().any(|l| l == "range: bytes=10-"));
    }
}

#[test]
fn rejects_bad_headers_and_schemes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    let bad = AddOptions {
        url: "http://127.0.0.1:1/x".into(),
        headers: vec![("Bad Name".into(), "1".into())],
        ..Default::default()
    };
    assert!(matches!(engine.add(bad), Err(swoop_core::Error::InvalidHeader(_))));
    assert!(matches!(
        rt.block_on(engine.probe_url("mms://example.com/a", &[])),
        Err(swoop_core::Error::UnsupportedScheme(_))
    ));
}

/// 全局限速：多连接并发时总速率仍守在限速值附近（之前会随连接数放大）
#[test]
fn speed_limit_holds_across_connections() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_server(body.clone(), true, "limited.bin");
    let dir = tempfile::tempdir().unwrap();

    // 1 MB/s、4 连接下 2 MB：令牌从 0 起算，理论约 2 秒
    let engine = Engine::new(
        rt.handle().clone(),
        Config { connections: 4, min_segment: 128 * 1024, speed_limit: 1024 * 1024, ..Config::default() },
    );
    let t0 = Instant::now();
    let id = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);
    let secs = t0.elapsed().as_secs_f64();
    assert!(secs > 1.7, "2 MB 在 1 MB/s 限速下只用了 {secs:.2} 秒");
    assert!(secs < 5.0, "限速过严：用了 {secs:.2} 秒");
    assert!(std::fs::read(dir.path().join("limited.bin")).unwrap() == *body);
}

#[test]
fn connection_defaults_are_bounded_at_startup_and_runtime() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for (requested, expected) in [(0, 1), (8, 8), (usize::MAX, MAX_CONNECTIONS)] {
        let engine = Engine::new(rt.handle().clone(), Config { connections: requested, ..Config::default() });
        assert_eq!(engine.connections(), expected);
        assert_eq!(engine.config().connections, expected);
        engine.set_connections(0);
        assert_eq!(engine.connections(), 1);
        engine.set_connections(usize::MAX);
        assert_eq!(engine.connections(), MAX_CONNECTIONS);
        engine.set_connections(3);
        assert_eq!(engine.connections(), 3);
        assert_eq!(engine.config().connections, expected, "启动配置保持不变");
    }
}

#[test]
fn connection_changes_apply_to_new_runs_without_changing_active_downloads() {
    // current_thread 在 block_on 前不执行异步任务，验证 add 本身就捕获配置，
    // 而不是到探测或创建 worker 时才读取已经变化的全局值。
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let body = Arc::new(make_body(4 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "connections.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 128 * 1024, ..Config::default() });
    let first = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: Some("first.bin".into()),
            ..Default::default()
        })
        .unwrap();
    engine.set_connections(1);
    let second = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().to_path_buf(),
            file_name: Some("second.bin".into()),
            ..Default::default()
        })
        .unwrap();
    engine.set_connections(3);

    rt.block_on(async {
        let start = Instant::now();
        loop {
            let a = engine.snapshot_one(first).unwrap();
            let b = engine.snapshot_one(second).unwrap();
            assert!(!matches!(a.status, TaskStatus::Failed(_)), "{a:?}");
            assert!(!matches!(b.status, TaskStatus::Failed(_)), "{b:?}");
            if a.active_connections == 4 && b.active_connections == 1 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "未按 add 时配置建立连接: {a:?}, {b:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        engine.set_connections(2);
        let mut saw_four_after_change = false;
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let a = engine.snapshot_one(first).unwrap();
            let b = engine.snapshot_one(second).unwrap();
            saw_four_after_change |= a.active_connections == 4;
            assert!(a.active_connections <= 4, "运行中的任务超出原连接数");
            assert!(b.active_connections <= 1, "运行中的单连接任务被配置变更扩容");
            assert!(!matches!(a.status, TaskStatus::Failed(_)), "{a:?}");
            assert!(!matches!(b.status, TaskStatus::Failed(_)), "{b:?}");
            if a.status == TaskStatus::Completed && b.status == TaskStatus::Completed {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(15), "下载超时");
        }
        assert!(saw_four_after_change, "降低默认值不应中断运行中的连接");
    });
    for name in ["first.bin", "second.bin"] {
        assert!(std::fs::read(dir.path().join(name)).unwrap() == *body);
    }
}

#[test]
fn resumed_download_uses_lower_connection_default_and_preserves_progress() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(4 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "resume-connections.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 128 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), ..Default::default() })
        .unwrap();
    let start = Instant::now();
    loop {
        let s = engine.snapshot_one(id).unwrap();
        if s.active_connections == 4 && s.downloaded >= 256 * 1024 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "未进入多连接下载: {s:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    engine.pause(id).unwrap();
    // pause 是异步取消；等旧运行的区段回到 Pending，再开始新的运行。
    let paused = loop {
        let s = engine.snapshot_one(id).unwrap();
        if s.status == TaskStatus::Paused && s.active_connections == 0 {
            break s;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "暂停未收尾: {s:?}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(paused.downloaded > 0 && paused.downloaded < body.len() as u64);
    assert!(paused.segments.len() >= 4);
    engine.set_connections(1);
    engine.resume(id).unwrap();
    engine.set_connections(3);
    let mut saw_single_connection = false;
    loop {
        let s = engine.snapshot_one(id).unwrap();
        assert!(s.downloaded >= paused.downloaded, "续传不得丢失断点");
        assert!(s.active_connections <= 1, "续传必须使用 resume 时的单连接配置: {s:?}");
        saw_single_connection |= s.active_connections == 1;
        assert!(!matches!(s.status, TaskStatus::Failed(_)), "{s:?}");
        if s.status == TaskStatus::Completed {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "续传超时: {s:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saw_single_connection);
    assert!(std::fs::read(dir.path().join("resume-connections.bin")).unwrap() == *body);
}

#[test]
fn rapid_pause_resume_preserves_bytes_without_failed_events() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let body = Arc::new(make_body(4 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "rapid-resume.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 128 * 1024, ..Config::default() });
    let mut events = engine.subscribe();
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), ..Default::default() })
        .unwrap();
    rt.block_on(async {
        let start = Instant::now();
        loop {
            let s = engine.snapshot_one(id).unwrap();
            if s.active_connections == 4 && s.downloaded >= 256 * 1024 {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "任务未开始: {s:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // 不让旧运行先完成取消清理；连续发出暂停、配置变更、继续。
        for _ in 0..12 {
            engine.pause(id).unwrap();
            engine.set_connections(1);
            engine.resume(id).unwrap();
        }
        loop {
            while let Ok(event) = events.try_recv() {
                assert!(!matches!(event, swoop_core::Event::Failed(_, _)), "暂停续传产生错误事件: {event:?}");
            }
            let s = engine.snapshot_one(id).unwrap();
            assert!(!matches!(s.status, TaskStatus::Failed(_)), "暂停续传失败: {s:?}");
            if s.status == TaskStatus::Completed {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(15), "任务未完成: {s:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    assert!(std::fs::read(dir.path().join("rapid-resume.bin")).unwrap() == *body);
}

fn wait_until(label: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(10), "等待 {label} 超时");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn queued_options(server: &Server, dir: &std::path::Path, name: &str) -> AddOptions {
    AddOptions {
        url: format!("{}?task={name}", server.url),
        dir: dir.to_path_buf(),
        file_name: Some(name.into()),
        ..Default::default()
    }
}

fn probe_order(server: &Server) -> Vec<String> {
    server
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|headers| headers.iter().any(|line| line == "range: bytes=0-0"))
        .map(|headers| headers[0].split("?task=").nth(1).unwrap().split(' ').next().unwrap().to_owned())
        .collect()
}

#[test]
fn main_queue_applies_concurrency_and_order_to_add_and_resume() {
    use swoop_core::MAIN_QUEUE_ID;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(1024 * 1024));
    let server = start_paced_server(body.clone(), true, "queue.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 1).unwrap();
    let a = engine.add(queued_options(&server, dir.path(), "a.bin")).unwrap();
    let b = engine.add(queued_options(&server, dir.path(), "b.bin")).unwrap();
    let c = engine.add(queued_options(&server, dir.path(), "c.bin")).unwrap();
    engine.pause(a).unwrap();
    engine.resume(a).unwrap();
    assert_eq!(engine.queue_for_task(a), Some(MAIN_QUEUE_ID));
    assert!(engine.reorder_queue(MAIN_QUEUE_ID, &[a, a, c]).is_err());
    engine.reorder_queue(MAIN_QUEUE_ID, &[b, a, c]).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    assert!(server.requests.lock().unwrap().is_empty(), "停止队列禁止 add/resume 绕过调度器");
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    wait_until("第一个顺序任务产生真实下载进度", || engine.snapshot_one(b).unwrap().downloaded > 0);
    assert_eq!(engine.snapshot_one(a).unwrap().status, TaskStatus::Queued);
    assert_eq!(engine.snapshot_one(c).unwrap().status, TaskStatus::Queued);
    for id in [b, a, c] {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    }
    assert_eq!(probe_order(&server), ["b.bin", "a.bin", "c.bin"]);
    for name in ["a.bin", "b.bin", "c.bin"] {
        assert_eq!(std::fs::read(dir.path().join(name)).unwrap(), *body);
    }
}

#[test]
fn queue_concurrency_is_separate_from_connection_count_and_can_change_live() {
    use swoop_core::MAIN_QUEUE_ID;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "parallel.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 2).unwrap();
    let ids: Vec<_> =
        (0..4).map(|i| engine.add(queued_options(&server, dir.path(), &format!("{i}.bin"))).unwrap()).collect();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    wait_until("两个 HTTP 下载同时产生进度", || {
        ids[..2].iter().all(|id| engine.snapshot_one(*id).unwrap().downloaded > 0)
    });
    assert_eq!(engine.queues()[0].active_count, 2);
    assert_eq!(engine.snapshot_one(ids[2]).unwrap().status, TaskStatus::Queued);
    assert_eq!(engine.snapshot_one(ids[3]).unwrap().status, TaskStatus::Queued);
    assert_eq!(probe_order(&server).len(), 2);
    // 降低并发不取消已运行任务；下一轮只允许一个任务继续。
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 1).unwrap();
    for id in &ids[..2] {
        assert_eq!(wait_done(&engine, *id, Duration::from_secs(10)), TaskStatus::Completed);
    }
    wait_until("第三个任务开始", || engine.snapshot_one(ids[2]).unwrap().downloaded > 0);
    assert_eq!(engine.queues()[0].active_count, 1);
    assert_eq!(engine.snapshot_one(ids[3]).unwrap().status, TaskStatus::Queued);
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 2).unwrap();
    wait_until("提高并发立即释放第四个任务", || engine.snapshot_one(ids[3]).unwrap().downloaded > 0);
    assert_eq!(engine.queues()[0].active_count, 2);
    for id in ids {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    }
}

#[test]
fn stopped_named_queue_keeps_waiting_and_resumed_tasks_from_starting() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "stopped.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    let queue = engine.create_queue("夜间任务", 1).unwrap();
    let a = engine.enqueue(queued_options(&server, dir.path(), "a.bin"), queue).unwrap();
    let b = engine.enqueue(queued_options(&server, dir.path(), "b.bin"), queue).unwrap();
    engine.start_queue(queue).unwrap();
    wait_until("在途任务", || engine.snapshot_one(a).unwrap().downloaded > 0);
    engine.pause(b).unwrap();
    engine.stop_queue(queue).unwrap();
    wait_until("旧运行取消并释放槽位", || {
        engine.queues().iter().find(|q| q.id == queue).unwrap().active_count == 0
    });
    assert_eq!(engine.snapshot_one(a).unwrap().status, TaskStatus::Queued);
    assert_eq!(engine.snapshot_one(b).unwrap().status, TaskStatus::Paused);
    engine.resume_all();
    engine.tick_queues().unwrap();
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(probe_order(&server), ["a.bin"], "等待任务和 resume_all 不能绕过已停止队列");
    let stopped = engine.queues().into_iter().find(|q| q.id == queue).unwrap();
    assert!(!stopped.running);
    assert_eq!(stopped.waiting_count, 2);
    engine.start_queue(queue).unwrap();
    for id in [a, b] {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    }
    assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), *body);
    assert_eq!(std::fs::read(dir.path().join("b.bin")).unwrap(), *body);
}

#[test]
fn moving_running_tasks_obeys_target_queue_and_removal_rejoins_main_on_resume() {
    use swoop_core::MAIN_QUEUE_ID;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "moved.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    let target = engine.create_queue("目标", 1).unwrap();
    let id = engine.add(queued_options(&server, dir.path(), "moved.bin")).unwrap();
    wait_until("任务已下载", || engine.snapshot_one(id).unwrap().downloaded > 0);
    engine.move_to_queue(id, target).unwrap();
    wait_until("移动后的旧运行退出", || engine.queues().iter().all(|q| q.active_count == 0));
    assert_eq!(engine.queue_for_task(id), Some(target));
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Queued);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(probe_order(&server), ["moved.bin"]);
    engine.remove_from_queue(id).unwrap();
    assert_eq!(engine.queue_for_task(id), None);
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Paused);
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine.resume(id).unwrap();
    assert_eq!(engine.queue_for_task(id), Some(MAIN_QUEUE_ID));
    engine.move_to_queue(id, target).unwrap();
    engine.delete_queue(target).unwrap();
    assert_eq!(engine.queue_for_task(id), Some(MAIN_QUEUE_ID));
    assert!(engine.delete_queue(MAIN_QUEUE_ID).is_err());
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(dir.path().join("moved.bin")).unwrap(), *body);
}

struct TestQueueClock(std::sync::atomic::AtomicI64);

impl swoop_core::QueueClock for TestQueueClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + Duration::from_secs(self.0.load(Ordering::SeqCst) as u64)
    }
}

#[test]
fn named_queue_order_and_recurring_schedule_survive_restart_with_injected_clock() {
    use swoop_core::{MAIN_QUEUE_ID, QueueSchedule, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "clock.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("queues.db")).unwrap());
    let clock = Arc::new(TestQueueClock(std::sync::atomic::AtomicI64::new(1000)));
    let make_engine = || {
        Engine::with_queue_clock(
            rt.handle().clone(),
            Config { connections: 1, ..Config::default() },
            Some(store.clone()),
            clock.clone(),
        )
    };
    let (queue, a, b) = {
        let engine = make_engine();
        engine.stop_queue(MAIN_QUEUE_ID).unwrap();
        let queue = engine.create_queue("按时下载", 1).unwrap();
        let a = engine.enqueue(queued_options(&server, dir.path(), "a.bin"), queue).unwrap();
        let b = engine.enqueue(queued_options(&server, dir.path(), "b.bin"), queue).unwrap();
        engine.rename_queue(queue, "夜间下载").unwrap();
        engine.reorder_queue(queue, &[b, a]).unwrap();
        engine
            .set_queue_schedule(
                queue,
                Some(QueueSchedule { start_at: Some(1010), stop_at: Some(1020), repeat_every_secs: Some(60) }),
            )
            .unwrap();
        (queue, a, b)
    };
    let engine = make_engine();
    assert_eq!(engine.load(true).unwrap(), 2);
    let q = engine.queues().into_iter().find(|q| q.id == queue).unwrap();
    assert_eq!(q.name, "夜间下载");
    assert_eq!(q.task_ids, [b, a]);
    assert_eq!(q.max_concurrent, 1);
    assert!(!q.running);
    assert_eq!(q.waiting_count, 2);
    clock.0.store(1010, Ordering::SeqCst);
    engine.tick_queues().unwrap();
    wait_until("定时任务已启动", || engine.snapshot_one(b).unwrap().downloaded > 0);
    assert_eq!(engine.snapshot_one(a).unwrap().status, TaskStatus::Queued);
    clock.0.store(1020, Ordering::SeqCst);
    engine.tick_queues().unwrap();
    wait_until("定时停止已释放运行槽位", || engine.queues().iter().all(|q| q.active_count == 0));
    assert_eq!(engine.snapshot_one(b).unwrap().status, TaskStatus::Queued);
    // 跨过两个完整周期，只消费最近边界，错过的任务不得被短暂补跑。
    clock.0.store(1140, Ordering::SeqCst);
    engine.tick_queues().unwrap();
    let q = engine.queues().into_iter().find(|q| q.id == queue).unwrap();
    assert!(!q.running);
    assert_eq!(q.next_start_at, Some(1190));
    assert_eq!(q.next_stop_at, Some(1200));
    drop(engine);
    let restarted = make_engine();
    assert_eq!(restarted.load(true).unwrap(), 2);
    let q = restarted.queues().into_iter().find(|q| q.id == queue).unwrap();
    assert!(!q.running);
    assert_eq!(q.next_start_at, Some(1190));
    assert_eq!(q.task_ids, [b, a]);
    assert_eq!(probe_order(&server), ["b.bin"]);
    clock.0.store(1190, Ordering::SeqCst);
    restarted.tick_queues().unwrap();
    for id in [b, a] {
        assert_eq!(wait_done(&restarted, id, Duration::from_secs(10)), TaskStatus::Completed);
    }
    assert_eq!(std::fs::read(dir.path().join("b.bin")).unwrap(), *body);
}

#[test]
fn loading_history_twice_cannot_replace_a_running_task_or_bypass_its_slot() {
    use swoop_core::{MAIN_QUEUE_ID, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "load-once.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config { connections: 1, ..Config::default() }, store);
    engine.load(true).unwrap();
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 2).unwrap();
    let id = engine.add(queued_options(&server, dir.path(), "load-once.bin")).unwrap();
    wait_until("第一次运行产生进度", || engine.snapshot_one(id).unwrap().downloaded > 0);
    assert!(engine.load(true).is_err());
    assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(probe_order(&server), ["load-once.bin"]);
    assert_eq!(std::fs::read(dir.path().join("load-once.bin")).unwrap(), *body);
}

#[test]
fn deleting_an_active_persisted_task_does_not_reuse_its_queue_slot_or_record() {
    use swoop_core::{MAIN_QUEUE_ID, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "replacement.bin", Duration::from_millis(15));
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config { connections: 1, ..Config::default() }, store.clone());
    engine.set_queue_concurrency(MAIN_QUEUE_ID, 1).unwrap();
    let old = engine.add(queued_options(&server, dir.path(), "old.bin")).unwrap();
    wait_until("旧任务开始", || engine.snapshot_one(old).unwrap().downloaded > 0);
    engine.remove(old, false).unwrap();
    let replacement = engine.add(queued_options(&server, dir.path(), "replacement.bin")).unwrap();
    assert_ne!(old, replacement, "旧任务仍可能收尾，新任务不得复用其 ID");
    assert_eq!(wait_done(&engine, replacement, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(dir.path().join("replacement.bin")).unwrap(), *body);
    let persisted = store.load_tasks().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].id, replacement);
    assert_eq!(persisted[0].status, TaskStatus::Completed);
    assert_eq!(engine.queues()[0].task_ids, [replacement]);
}

#[test]
fn stopping_a_throttled_queue_releases_slots_without_persisting_unwritten_bytes() {
    use swoop_core::{MAIN_QUEUE_ID, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(256 * 1024));
    let server = start_paced_server(body.clone(), true, "throttled.bin", Duration::from_millis(2));
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let engine = Engine::with_store(
        rt.handle().clone(),
        Config { connections: 1, speed_limit: 1, ..Config::default() },
        store.clone(),
    );
    let id = engine.add(queued_options(&server, dir.path(), "throttled.bin")).unwrap();
    wait_until("worker 正在等待限速配额", || engine.snapshot_one(id).unwrap().active_connections == 1);
    // 跨过一次进度持久化周期；1 B/s 下首个块远未取得写入配额。
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(store.load_tasks().unwrap()[0].segments.total_downloaded(), 0);
    assert_eq!(engine.snapshot_one(id).unwrap().downloaded, 0);
    let started = Instant::now();
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    wait_until("低速任务取消", || engine.queues()[0].active_count == 0);
    assert!(started.elapsed() < Duration::from_secs(1), "停止不应等待限速配额");
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Queued);
    engine.set_speed_limit(0);
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(dir.path().join("throttled.bin")).unwrap(), *body);
}

#[test]
fn task_properties_persist_and_url_changes_clear_credentials_unless_explicitly_replaced() {
    use swoop_core::{MAIN_QUEUE_ID, Store, TaskProperties};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("properties.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let id = engine
        .add(AddOptions {
            url: "http://127.0.0.1:1/old".into(),
            dir: dir.path().to_path_buf(),
            headers: vec![("Cookie".into(), "secret=old".into())],
            ..Default::default()
        })
        .unwrap();
    let properties = TaskProperties { description: "离线安装包".into(), speed_limit: 512 * 1024 };
    engine.set_task_properties(id, properties.clone()).unwrap();
    engine.update_task_url(id, "http://127.0.0.1:1/old", None).unwrap();
    assert_eq!(store.load_tasks().unwrap()[0].headers.len(), 1, "同URL编辑保留原头");
    engine.update_task_url(id, "http://127.0.0.1:1/new", None).unwrap();
    assert!(store.load_tasks().unwrap()[0].headers.is_empty());
    engine
        .update_task_url(id, "http://127.0.0.1:1/latest", Some(vec![("Authorization".into(), "Bearer new".into())]))
        .unwrap();
    assert_eq!(store.load_tasks().unwrap()[0].headers, vec![("Authorization".into(), "Bearer new".into())]);
    // 模拟进度线程此前持有的旧快照，不允许其覆盖新属性。
    let mut stale = store.load_tasks().unwrap().remove(0);
    stale.description = String::new();
    stale.speed_limit = 0;
    store.save_task(&stale).unwrap();
    drop(engine);
    let restarted = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    restarted.load(false).unwrap();
    assert_eq!(restarted.task_properties(id).unwrap(), properties);
    assert!(store.load_tasks().unwrap()[0].source_changed);
    assert!(
        restarted.set_task_properties(id, TaskProperties { description: "长".repeat(4097), speed_limit: 0 }).is_err()
    );
    assert_eq!(restarted.task_properties(id).unwrap(), properties);
}

#[test]
fn refreshed_urls_resume_only_with_matching_strong_etag_and_size_after_restart() {
    use swoop_core::Store;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(1024 * 1024));
    for (old_etag, new_etag, same_size, preserve) in [
        (None, None, true, false),
        (Some("W/\"same\""), Some("W/\"same\""), true, false),
        (Some("\"old\""), Some("\"new\""), true, false),
        (Some("\"same\""), Some("\"same\""), false, false),
        (Some("\"same\""), Some("\"same\""), true, true),
    ] {
        let old = start_server_with(
            body.clone(),
            ServerOpts {
                support_range: true,
                file_name: "update.bin".into(),
                chunk_delay: Duration::from_millis(25),
                require: None,
                etag: old_etag.map(str::to_owned),
            },
        );
        let new_body = if preserve {
            body.clone()
        } else {
            Arc::new(
                body.iter()
                    .take(if same_size { body.len() } else { body.len() / 2 })
                    .map(|byte| byte ^ 0xff)
                    .collect::<Vec<_>>(),
            )
        };
        let new = start_server_with(
            new_body.clone(),
            ServerOpts {
                support_range: true,
                file_name: "update.bin".into(),
                chunk_delay: Duration::from_millis(1),
                require: None,
                etag: new_etag.map(str::to_owned),
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("update.db")).unwrap());
        let config = Config { connections: 1, ..Config::default() };
        let engine = Engine::with_store(rt.handle().clone(), config.clone(), store.clone());
        let mut opts = queued_options(&old, dir.path(), "update.bin");
        opts.headers = vec![("Cookie".into(), "session=private".into()), ("Authorization".into(), "Bearer old".into())];
        let id = engine.add(opts).unwrap();
        wait_until("旧下载拥有真实片段", || engine.snapshot_one(id).unwrap().downloaded >= 64 * 1024);
        assert!(engine.update_task_url(id, &new.url, None).is_err(), "运行时禁止换地址");
        engine.pause(id).unwrap();
        wait_until("旧run完全退出", || engine.queues()[0].active_count == 0);
        let old_progress = engine.snapshot_one(id).unwrap().downloaded;
        assert!(old_progress > 0 && old_progress < body.len() as u64);
        engine.update_task_url(id, &new.url, None).unwrap();
        assert!(store.load_tasks().unwrap()[0].source_changed);
        drop(engine);
        let restarted = Engine::with_store(rt.handle().clone(), config, store.clone());
        restarted.load(false).unwrap();
        restarted.resume(id).unwrap();
        assert_eq!(wait_done(&restarted, id, Duration::from_secs(10)), TaskStatus::Completed);
        assert_eq!(std::fs::read(dir.path().join("update.bin")).unwrap(), *new_body, "不能拼接不同实体");
        let requests = new.requests.lock().unwrap();
        assert!(requests.iter().all(|request| {
            request.iter().all(|line| !line.starts_with("cookie:") && !line.starts_with("authorization:"))
        }));
        let first_download_range = requests
            .iter()
            .flat_map(|request| request.iter())
            .find(|line| line.starts_with("range: bytes=") && *line != "range: bytes=0-0")
            .unwrap();
        assert_eq!(
            first_download_range.starts_with("range: bytes=0-"),
            !preserve,
            "{first_download_range}, preserve={preserve}"
        );
        assert!(!store.load_tasks().unwrap()[0].source_changed);
    }
}

#[test]
fn task_speed_limit_does_not_throttle_other_tasks_and_changes_wake_existing_waits() {
    use swoop_core::{MAIN_QUEUE_ID, TaskProperties};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(256 * 1024));
    let server = start_paced_server(body.clone(), true, "limits.bin", Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Config::default() });
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let slow = engine.add(queued_options(&server, dir.path(), "slow.bin")).unwrap();
    let fast = engine.add(queued_options(&server, dir.path(), "fast.bin")).unwrap();
    engine.set_task_properties(slow, TaskProperties { description: String::new(), speed_limit: 1 }).unwrap();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    assert_eq!(wait_done(&engine, fast, Duration::from_secs(5)), TaskStatus::Completed);
    assert_eq!(engine.snapshot_one(slow).unwrap().downloaded, 0, "独立慢任务仍在等自己的配额");
    let changed = Instant::now();
    engine.set_task_properties(slow, TaskProperties::default()).unwrap();
    assert_eq!(wait_done(&engine, slow, Duration::from_secs(2)), TaskStatus::Completed);
    assert!(changed.elapsed() < Duration::from_secs(1), "设置改变须唤醒已有等待");
    assert_eq!(std::fs::read(dir.path().join("slow.bin")).unwrap(), *body);
}

#[test]
fn task_and_global_limiters_both_apply_and_pausing_a_slow_task_is_prompt() {
    use swoop_core::{MAIN_QUEUE_ID, TaskProperties};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(512 * 1024));
    let server = start_paced_server(body.clone(), true, "combined.bin", Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::new(rt.handle().clone(), Config { connections: 1, speed_limit: 512 * 1024, ..Config::default() });
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let ids: Vec<_> =
        (0..2).map(|i| engine.add(queued_options(&server, dir.path(), &format!("{i}.bin"))).unwrap()).collect();
    for id in &ids {
        engine
            .set_task_properties(*id, TaskProperties { description: String::new(), speed_limit: 384 * 1024 })
            .unwrap();
    }
    let started = Instant::now();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    for id in ids {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    }
    assert!(started.elapsed() >= Duration::from_millis(1600), "两个任务不能突破全局总额");
    let paused = engine.add(queued_options(&server, dir.path(), "paused.bin")).unwrap();
    engine.set_task_properties(paused, TaskProperties { description: String::new(), speed_limit: 1 }).unwrap();
    wait_until("低速任务开始", || engine.snapshot_one(paused).unwrap().active_connections > 0);
    let started = Instant::now();
    engine.pause(paused).unwrap();
    wait_until("任务限速等待取消", || engine.queues()[0].active_count == 0);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(engine.snapshot_one(paused).unwrap().status, TaskStatus::Paused);
}

#[test]
fn quota_aggregates_all_tasks_and_changes_or_pause_wake_waiters() {
    use swoop_core::{MAIN_QUEUE_ID, QuotaPolicy};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let clock = Arc::new(TestQueueClock(std::sync::atomic::AtomicI64::new(1000)));
    let body = Arc::new(make_body(16 * 1024));
    let server = start_paced_server(body.clone(), true, "aggregate.bin", Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let engine =
        Engine::with_queue_clock(rt.handle().clone(), Config { connections: 2, ..Config::default() }, None, clock);
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    engine.set_quota_policy(QuotaPolicy { max_bytes: 5000, window_secs: 10 }).unwrap();
    let ids: Vec<_> =
        (0..3).map(|i| engine.add(queued_options(&server, dir.path(), &format!("{i}.bin"))).unwrap()).collect();
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    wait_until("全局配额耗尽", || engine.quota().used_bytes == 5000);
    assert_eq!(engine.snapshot().iter().map(|task| task.downloaded).sum::<u64>(), 5000);
    assert_eq!(engine.quota().remaining_bytes, Some(0));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(engine.quota().used_bytes, 5000, "probe和等待不重复计费");
    let paused = Instant::now();
    engine.pause_all();
    wait_until("取消全部配额等待", || engine.queues()[0].active_count == 0);
    assert!(paused.elapsed() < Duration::from_secs(1));
    assert_eq!(engine.quota().used_bytes, 5000);
    engine.set_quota_policy(QuotaPolicy { max_bytes: 8000, window_secs: 10 }).unwrap();
    engine.resume_all();
    wait_until("加额后只放行额外3000字节", || engine.quota().used_bytes == 8000);
    engine.set_quota_policy(QuotaPolicy { max_bytes: 0, window_secs: 10 }).unwrap();
    for id in ids {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(5)), TaskStatus::Completed);
    }
    assert_eq!(engine.quota().used_bytes, 3 * body.len() as u64);
    for i in 0..3 {
        assert_eq!(std::fs::read(dir.path().join(format!("{i}.bin"))).unwrap(), *body);
    }
}

#[test]
fn quota_windows_reset_automatically_without_discarding_a_partially_written_http_chunk() {
    use swoop_core::QuotaPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let clock = Arc::new(TestQueueClock(std::sync::atomic::AtomicI64::new(1000)));
    let body = Arc::new(make_body(16 * 1024));
    let server = start_paced_server(body.clone(), true, "windows.bin", Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::with_queue_clock(
        rt.handle().clone(),
        Config { connections: 1, ..Config::default() },
        None,
        clock.clone(),
    );
    engine.set_quota_policy(QuotaPolicy { max_bytes: 4096, window_secs: 10 }).unwrap();
    let id = engine.add(queued_options(&server, dir.path(), "windows.bin")).unwrap();
    for window in 0..4 {
        clock.0.store(1000 + window * 10, Ordering::SeqCst);
        wait_until("当前窗口字节写满", || {
            engine.snapshot_one(id).unwrap().downloaded == ((window + 1) * 4096) as u64
        });
        assert_eq!(engine.quota().used_bytes, 4096);
        assert_eq!(engine.quota().resets_at, 1010 + window * 10);
    }
    assert_eq!(wait_done(&engine, id, Duration::from_secs(5)), TaskStatus::Completed);
    assert_eq!(std::fs::read(dir.path().join("windows.bin")).unwrap(), *body);
    assert_eq!(server.range_requests.load(Ordering::Relaxed), 2, "跨窗口应保留响应块而非重发请求");
}

#[test]
fn quota_usage_survives_restart_and_only_expiry_releases_the_window() {
    use swoop_core::{QuotaPolicy, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let clock = Arc::new(TestQueueClock(std::sync::atomic::AtomicI64::new(1000)));
    let body = Arc::new(make_body(256 * 1024));
    let server = start_paced_server(body.clone(), true, "durable.bin", Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("quota.db")).unwrap());
    let make_engine = || {
        Engine::with_queue_clock(
            rt.handle().clone(),
            Config { connections: 1, ..Config::default() },
            Some(store.clone()),
            clock.clone(),
        )
    };
    let engine = make_engine();
    engine.set_quota_policy(QuotaPolicy { max_bytes: 64 * 1024, window_secs: 10 }).unwrap();
    let id = engine.add(queued_options(&server, dir.path(), "durable.bin")).unwrap();
    wait_until("第一窗口额度耗尽", || engine.quota().used_bytes == 64 * 1024);
    engine.pause_all();
    wait_until("退出前全部运行停止", || engine.queues()[0].active_count == 0);
    drop(engine);
    let restarted = make_engine();
    restarted.load(false).unwrap();
    assert_eq!(restarted.quota().used_bytes, 64 * 1024);
    assert_eq!(restarted.quota().policy.max_bytes, 64 * 1024);
    restarted.resume(id).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(restarted.snapshot_one(id).unwrap().downloaded, 64 * 1024, "重启不能领取新额度");
    clock.0.store(1010, Ordering::SeqCst);
    wait_until("下一窗口自动释放", || restarted.snapshot_one(id).unwrap().downloaded == 128 * 1024);
    assert_eq!(restarted.quota().used_bytes, 64 * 1024);
    restarted.set_quota_policy(QuotaPolicy { max_bytes: 0, window_secs: 10 }).unwrap();
    assert_eq!(wait_done(&restarted, id, Duration::from_secs(5)), TaskStatus::Completed);
    assert_eq!(restarted.quota().used_bytes, 192 * 1024);
    assert_eq!(std::fs::read(dir.path().join("durable.bin")).unwrap(), *body);
}

#[test]
fn named_downloads_reserve_distinct_files_without_overwriting_existing_content() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(512 * 1024));
    let server = start_server(body.clone(), true, "shared.bin");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shared.bin"), b"user's original file").unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    engine.set_file_conflict_policy(swoop_core::FileConflictPolicy::Rename).unwrap();
    let ids: Vec<_> = (0..3)
        .map(|_| {
            engine
                .add(AddOptions {
                    url: server.url.clone(),
                    dir: dir.path().into(),
                    file_name: Some("shared.bin".into()),
                    ..Default::default()
                })
                .unwrap()
        })
        .collect();
    let mut paths = std::collections::HashSet::new();
    for id in ids {
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
        let snapshot = engine.snapshot_one(id).unwrap();
        assert_ne!(snapshot.file_name, "shared.bin", "a new download claimed the user's existing file");
        assert!(paths.insert(snapshot.path.clone()), "two tasks claimed the same file");
        assert_eq!(std::fs::read(snapshot.path).unwrap(), *body);
    }
    assert_eq!(std::fs::read(dir.path().join("shared.bin")).unwrap(), b"user's original file");
}

#[test]
fn explicit_names_cannot_escape_the_download_directory() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    engine.stop_queue(swoop_core::MAIN_QUEUE_ID).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for name in ["../escape.bin", "sub/file.bin", "sub\\file.bin", "/tmp/outside", "", ".", ".."] {
        assert!(
            engine
                .add(AddOptions {
                    url: "http://127.0.0.1:1/file".into(),
                    dir: dir.path().into(),
                    file_name: Some(name.into()),
                    ..Default::default()
                })
                .is_err(),
            "accepted unsafe name: {name:?}"
        );
    }
    assert!(engine.snapshot().is_empty());
}

#[test]
fn missing_or_truncated_partial_files_fail_instead_of_completing_with_missing_bytes() {
    for remove in [true, false] {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let body = Arc::new(make_body(4 * 1024 * 1024));
        let server = start_paced_server(body, true, "partial.bin", Duration::from_millis(20));
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
        let id =
            engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while engine.snapshot_one(id).unwrap().downloaded < 64 * 1024 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.pause(id).unwrap();
        // 更新相同地址只在旧 run 已退出后才允许，不依赖一个猜测的 sleep。
        while engine.update_task_url(id, &server.url, None).is_err() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let path = dir.path().join("partial.bin.swoop");
        assert!(!engine.snapshot_one(id).unwrap().path.exists(), "未完成时不应出现目标文件");
        if remove {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, b"user changed the partial file").unwrap();
        }
        engine.resume(id).unwrap();
        assert!(matches!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Failed(_)));
        if remove {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read(path).unwrap(), b"user changed the partial file");
        }
    }
}

fn wait_for_status(engine: &Engine, id: swoop_core::TaskId, expected: TaskStatus) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let actual = engine.snapshot_one(id).unwrap().status;
        if actual == expected {
            return;
        }
        assert!(!matches!(actual, TaskStatus::Failed(_)), "unexpected failure: {actual:?}");
        assert!(Instant::now() < deadline, "expected {expected:?}, got {actual:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn same_name_asks_by_default_survives_restart_and_can_be_renamed_or_skipped() {
    use swoop_core::{FileConflictPolicy, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(128 * 1024));
    let server = start_server(body.clone(), true, "conflict.bin");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conflict.bin");
    std::fs::write(&path, b"preserve original").unwrap();
    let store = Arc::new(Store::open(&dir.path().join("test.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    assert_eq!(engine.file_conflict_policy(), FileConflictPolicy::Ask);
    let id = engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve original");
    assert_eq!(store.load_tasks().unwrap()[0].status, TaskStatus::FileConflict);
    assert!(engine.resolve_file_conflict(id, FileConflictPolicy::Ask).is_err());
    // 等待确认不占用队列槽，其他任务仍可完成。
    let free = engine
        .add(AddOptions {
            url: server.url.clone(),
            dir: dir.path().into(),
            file_name: Some("free.bin".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait_done(&engine, free, Duration::from_secs(10)), TaskStatus::Completed);
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    restored.load(true).unwrap();
    assert_eq!(restored.snapshot_one(id).unwrap().status, TaskStatus::FileConflict);
    std::fs::write(&path, b"edited after restart").unwrap();
    restored.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
    wait_for_status(&restored, id, TaskStatus::FileConflict);
    assert_eq!(std::fs::read(&path).unwrap(), b"edited after restart");
    restored.resolve_file_conflict(id, FileConflictPolicy::Rename).unwrap();
    assert_eq!(wait_done(&restored, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(restored.snapshot_one(id).unwrap().file_name, "conflict (1).bin");
    assert_eq!(std::fs::read(restored.snapshot_one(id).unwrap().path).unwrap(), *body);
    assert_eq!(std::fs::read(&path).unwrap(), b"edited after restart");
    let skipped =
        restored.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&restored, skipped, TaskStatus::FileConflict);
    restored.resolve_file_conflict(skipped, FileConflictPolicy::Skip).unwrap();
    assert_eq!(restored.snapshot_one(skipped).unwrap().status, TaskStatus::Paused);
    restored.set_file_conflict_policy(FileConflictPolicy::Skip).unwrap();
    assert_eq!(
        Engine::with_store(rt.handle().clone(), Config::default(), store).file_conflict_policy(),
        FileConflictPolicy::Skip
    );
}

#[test]
fn overwrite_waits_for_success_and_cancel_keeps_original_until_retry_finishes() {
    use swoop_core::FileConflictPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "replace.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replace.bin");
    std::fs::write(&path, b"original file").unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    let id = engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
    wait_for_status(&engine, id, TaskStatus::Downloading);
    assert_eq!(std::fs::read(&path).unwrap(), b"original file");
    engine.pause(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.update_task_url(id, &server.url, None).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"original file");
    engine.resume(id).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), *body);
    assert_eq!(engine.snapshot_one(id).unwrap().path, path);
    assert!(
        !std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".swoop-sync-"))
    );
}

#[test]
fn edits_during_question_reask_and_edits_during_overwrite_are_never_replaced() {
    use swoop_core::FileConflictPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body, true, "edited.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edited.bin");
    std::fs::write(&path, b"original").unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    let id = engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    std::fs::write(&path, b"edited while question was open").unwrap();
    engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    assert_eq!(std::fs::read(&path).unwrap(), b"edited while question was open");
    engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
    wait_for_status(&engine, id, TaskStatus::Downloading);
    std::fs::write(&path, b"user edited again during download").unwrap();
    assert!(matches!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Failed(_)));
    assert_eq!(std::fs::read(&path).unwrap(), b"user edited again during download");
}

#[test]
fn configured_skip_and_overwrite_apply_after_restart_including_identical_bytes() {
    use swoop_core::{FileConflictPolicy, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(128 * 1024));
    let server = start_server(body.clone(), true, "configured.bin");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("configured.bin");
    std::fs::write(&path, b"preserve on skip").unwrap();
    let store = Arc::new(Store::open(&dir.path().join("test.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.set_file_conflict_policy(FileConflictPolicy::Skip).unwrap();
    drop(engine);
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    let skipped =
        engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, skipped, TaskStatus::Paused);
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve on skip");
    engine.set_file_conflict_policy(FileConflictPolicy::Overwrite).unwrap();
    drop(engine);
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store);
    assert_eq!(engine.file_conflict_policy(), FileConflictPolicy::Overwrite);
    for _ in 0..2 {
        let id =
            engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
        assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
        assert_eq!(std::fs::read(&path).unwrap(), *body);
        assert_eq!(engine.snapshot_one(id).unwrap().path, path);
    }
}

#[test]
fn approved_overwrite_can_resume_after_restart_without_touching_original_early() {
    use swoop_core::{FileConflictPolicy, Store};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "restart.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.bin");
    std::fs::write(&path, b"original survives restart").unwrap();
    let store = Arc::new(Store::open(&dir.path().join("test.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    let id = engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
    wait_for_status(&engine, id, TaskStatus::Downloading);
    engine.pause(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.update_task_url(id, &server.url, None).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"original survives restart");
    let record = store.load_tasks().unwrap().into_iter().find(|record| record.id == id).unwrap();
    assert!(record.sync_state.overwrite && record.sync_state.pending);
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
    restored.load(false).unwrap();
    assert_eq!(restored.snapshot_one(id).unwrap().status, TaskStatus::Paused);
    restored.resume(id).unwrap();
    assert_eq!(wait_done(&restored, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), *body);
}

#[test]
fn external_writer_reservation_blocks_http_overwrite_and_rename_avoids_it() {
    use swoop_core::FileConflictPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(128 * 1024));
    let server = start_server(body.clone(), true, "media.mp4");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("media.mp4");
    let engine = Engine::new(rt.handle().clone(), Config::default());
    engine.set_file_conflict_policy(FileConflictPolicy::Overwrite).unwrap();
    let media_lease = engine.target_registry().reserve(&path).unwrap();
    let id = engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, id, TaskStatus::FileConflict);
    assert!(!path.exists(), "HTTP must not create a file reserved by a media worker");
    assert!(engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).is_err());
    engine.resolve_file_conflict(id, FileConflictPolicy::Rename).unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(engine.snapshot_one(id).unwrap().file_name, "media (1).mp4");
    assert_eq!(std::fs::read(engine.snapshot_one(id).unwrap().path).unwrap(), *body);
    assert!(!path.exists());
    drop(media_lease);
    assert!(engine.target_registry().reserve(&path).is_ok());
}

#[test]
fn overwrite_does_not_replace_another_tasks_paused_partial_file() {
    use swoop_core::FileConflictPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = Arc::new(make_body(2 * 1024 * 1024));
    let server = start_paced_server(body.clone(), true, "owned.bin", Duration::from_millis(20));
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
    let original =
        engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    wait_for_status(&engine, original, TaskStatus::Downloading);
    engine.pause(original).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.update_task_url(original, &server.url, None).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let path = engine.snapshot_one(original).unwrap().path;
    let part = dir.path().join("owned.bin.swoop");
    let partial = std::fs::read(&part).unwrap();
    engine.set_file_conflict_policy(FileConflictPolicy::Overwrite).unwrap();
    // 另一个任务的 `.swoop` 不算同名文件，也不能被覆盖：新任务自动顺延编号。
    let replacement =
        engine.add(AddOptions { url: server.url.clone(), dir: dir.path().into(), ..Default::default() }).unwrap();
    assert_eq!(wait_done(&engine, replacement, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(engine.snapshot_one(replacement).unwrap().file_name, "owned (1).bin");
    assert_eq!(std::fs::read(&part).unwrap(), partial);
    assert!(!path.exists());
    engine.resume(original).unwrap();
    assert_eq!(wait_done(&engine, original, Duration::from_secs(10)), TaskStatus::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), *body);
}
