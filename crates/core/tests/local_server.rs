//! 用一个支持 Range 的最小 HTTP 服务器验证引擎端到端：多连接、动态分段、校验内容。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use swoop_core::{AddOptions, Config, Engine, TaskStatus};

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
}

fn start_server(body: Arc<Vec<u8>>, support_range: bool, file_name: &str) -> Server {
    start_paced_server(body, support_range, file_name, Duration::from_millis(2))
}

/// `chunk_delay`：每发 64 KB 停顿多久，用来控制单连接的速度
fn start_paced_server(body: Arc<Vec<u8>>, support_range: bool, file_name: &str, chunk_delay: Duration) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter2 = counter.clone();
    let url = format!("http://{addr}/files/{file_name}");
    let file_name = file_name.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let body = body.clone();
            let counter = counter2.clone();
            let file_name = file_name.clone();
            std::thread::spawn(move || handle(stream, body, support_range, &counter, &file_name, chunk_delay));
        }
    });
    Server { url, range_requests: counter }
}

fn handle(
    mut stream: TcpStream,
    body: Arc<Vec<u8>>,
    support_range: bool,
    counter: &AtomicUsize,
    file_name: &str,
    chunk_delay: Duration,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut range: Option<(u64, u64)> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("range: bytes=") {
            let v = v.trim();
            let (a, b) = v.split_once('-').unwrap();
            let start: u64 = a.parse().unwrap();
            let end: u64 = if b.is_empty() { body.len() as u64 - 1 } else { b.parse().unwrap() };
            range = Some((start, end));
        }
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

    let engine = Engine::new(
        rt.handle().clone(),
        Config { connections: 6, min_segment: 128 * 1024, ..Config::default() },
    );
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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

    let engine = Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 64 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
        .unwrap();
    assert_eq!(wait_done(&engine, id, Duration::from_secs(30)), TaskStatus::Completed);

    let snap = engine.snapshot_one(id).unwrap();
    assert_eq!(snap.segments.len(), 1);
    let got = std::fs::read(dir.path().join("plain.bin")).unwrap();
    assert!(got == *body);
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
            .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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
    let id2 = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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
            .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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

    let engine = Engine::new(rt.handle().clone(), Config { connections: 4, min_segment: 128 * 1024, ..Config::default() });
    let id = engine
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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
        .add(AddOptions { url: server.url.clone(), dir: dir.path().to_path_buf(), file_name: None })
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
