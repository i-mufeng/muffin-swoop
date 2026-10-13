//! 同名任务的删除归属，以及写库失败时禁止修改目标的真实 HTTP / SQLite 回归。
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use swoop_core::{AddOptions, Config, Engine, FileConflictPolicy, MAIN_QUEUE_ID, Store, TaskId, TaskStatus};

struct Server {
    address: SocketAddr,
    body: Arc<Mutex<Vec<u8>>>,
    downloads: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn start(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = Arc::new(Mutex::new(body));
        let downloads = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (response, requests, stopped) = (body.clone(), downloads.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let mut workers = Vec::new();
            for socket in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut socket) = socket else { break };
                let (response, requests) = (response.clone(), requests.clone());
                workers.push(std::thread::spawn(move || {
                    let _ = serve(&mut socket, &response, &requests);
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self { address, body, downloads, stop, thread: Some(thread) }
    }

    fn options(&self, dir: &Path) -> AddOptions {
        AddOptions {
            url: format!("http://{}/file.bin", self.address),
            dir: dir.into(),
            file_name: Some("file.bin".into()),
            ..Default::default()
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn serve(socket: &mut TcpStream, body: &Mutex<Vec<u8>>, downloads: &AtomicUsize) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(socket.try_clone()?);
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            let (start, end) = value.trim().split_once('-').unwrap();
            range = Some((start.parse::<usize>().unwrap(), end.parse::<usize>().ok()));
        }
    }
    let body = body.lock().unwrap().clone();
    let total = body.len();
    if range != Some((0, Some(0))) {
        downloads.fetch_add(1, Ordering::SeqCst);
    }
    let (start, end) = range.map(|(start, end)| (start, end.unwrap_or(total - 1))).unwrap_or((0, total - 1));
    let mut headers = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nETag: \"{}-{total}\"\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        if range.is_some() { "206 Partial Content" } else { "200 OK" },
        end - start + 1,
        body[0],
    );
    if range.is_some() {
        headers += &format!("Content-Range: bytes {start}-{end}/{total}\r\n");
    }
    socket.write_all(format!("{headers}\r\n").as_bytes())?;
    for chunk in body[start..=end].chunks(16 * 1024) {
        socket.write_all(chunk)?;
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn wait_for(engine: &Engine, id: TaskId, expected: impl Fn(&TaskStatus) -> bool) -> TaskStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let actual = engine.snapshot_one(id).unwrap().status;
        if expected(&actual) {
            return actual;
        }
        assert!(Instant::now() < deadline, "unexpected status: {actual:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_partial(engine: &Engine, id: TaskId) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.snapshot_one(id).unwrap().downloaded == 0 {
        assert!(Instant::now() < deadline, "no download progress: {:?}", engine.snapshot_one(id));
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_released(engine: &Engine, path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while engine.target_registry().is_reserved(path).unwrap() {
        assert!(Instant::now() < deadline, "worker did not release target");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pause_finished(engine: &Engine, id: TaskId) {
    engine.pause(id).unwrap();
    wait_released(engine, &engine.snapshot_one(id).unwrap().path);
}
fn reject_downloading_updates(path: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_download BEFORE UPDATE ON tasks
             WHEN NEW.status = 'downloading'
             BEGIN SELECT RAISE(ABORT, 'injected ownership save failure'); END;",
        )
        .unwrap();
    connection
}

#[test]
fn deleting_unconfirmed_or_skipped_conflicts_preserves_original_after_restart() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 64 * 1024]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.bin");
    std::fs::write(&path, b"unowned original").unwrap();
    let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    let id = engine.add(server.options(dir.path())).unwrap();
    wait_for(&engine, id, |status| *status == TaskStatus::FileConflict);
    wait_released(&engine, &path);
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
    restored.load(false).unwrap();
    restored.remove(id, true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"unowned original");

    let skipped = restored.add(server.options(dir.path())).unwrap();
    wait_for(&restored, skipped, |status| *status == TaskStatus::FileConflict);
    restored.resolve_file_conflict(skipped, FileConflictPolicy::Skip).unwrap();
    restored.remove(skipped, true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"unowned original");
}

#[test]
fn deleting_queued_task_never_deletes_an_existing_file() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 64 * 1024]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.bin");
    std::fs::write(&path, b"not downloaded by task").unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let id = engine.add(server.options(dir.path())).unwrap();
    engine.remove(id, true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"not downloaded by task");
    assert_eq!(server.downloads.load(Ordering::SeqCst), 0);
}

#[test]
fn deleting_pending_overwrite_preserves_original_while_running_and_after_restart() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 1024 * 1024]);
    for restart in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.bin");
        std::fs::write(&path, b"original until successful publication").unwrap();
        let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
        let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
        let id = engine.add(server.options(dir.path())).unwrap();
        wait_for(&engine, id, |status| *status == TaskStatus::FileConflict);
        wait_released(&engine, &path);
        engine.resolve_file_conflict(id, FileConflictPolicy::Overwrite).unwrap();
        wait_partial(&engine, id);
        let engine = if restart {
            pause_finished(&engine, id);
            drop(engine);
            let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
            restored.load(false).unwrap();
            restored
        } else {
            engine
        };
        engine.remove(id, true).unwrap();
        wait_released(&engine, &path);
        assert_eq!(std::fs::read(&path).unwrap(), b"original until successful publication");
        assert!(engine.snapshot_one(id).is_none());
    }
}

#[test]
fn deleting_owned_partial_and_completed_downloads_still_removes_the_file() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 1024 * 1024]);
    for complete in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
        let id = engine.add(server.options(dir.path())).unwrap();
        if complete {
            wait_for(&engine, id, |status| *status == TaskStatus::Completed);
            wait_released(&engine, &dir.path().join("file.bin"));
        } else {
            wait_partial(&engine, id);
            pause_finished(&engine, id);
        }
        let target = engine.snapshot_one(id).unwrap().path;
        let path = if complete { target.clone() } else { dir.path().join("file.bin.swoop") };
        assert_eq!(target.exists(), complete, "未完成的数据只在 .swoop 临时文件里");
        assert!(path.exists());
        engine.remove(id, true).unwrap();
        assert!(!path.exists());
        assert!(!target.exists());
    }
}

#[test]
fn sqlite_failure_before_first_write_keeps_placeholder_empty_and_records_no_download() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 1024 * 1024]);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let id = engine.add(server.options(dir.path())).unwrap();
    let connection = reject_downloading_updates(&db);
    engine.start_queue(MAIN_QUEUE_ID).unwrap();
    let failure = wait_for(&engine, id, |status| matches!(status, TaskStatus::Failed(_)));
    assert!(matches!(failure, TaskStatus::Failed(message) if message.contains("injected ownership save failure")));
    let path = dir.path().join("file.bin");
    let part = dir.path().join("file.bin.swoop");
    wait_released(&engine, &path);
    assert!(!path.exists());
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
    assert_eq!(server.downloads.load(Ordering::SeqCst), 0);
    assert_eq!(engine.snapshot_one(id).unwrap().downloaded, 0);
    assert!(store.load_tasks().unwrap()[0].segments.segments.is_empty());
    drop(engine);
    connection.execute_batch("DROP TRIGGER reject_download;").unwrap();
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store);
    restored.load(false).unwrap();
    // 空的临时文件属于本任务，重启后直接重用，不再当成同名文件询问。
    restored.resume(id).unwrap();
    wait_for(&restored, id, |status| *status == TaskStatus::Completed);
    assert!(!part.exists());
    assert_eq!(std::fs::read(path).unwrap(), vec![7; 1024 * 1024]);
}

#[test]
fn sqlite_failure_before_replanning_preserves_partial_bytes_and_resume_validators() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 1024 * 1024]);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine =
        Engine::with_store(rt.handle().clone(), Config { connections: 1, ..Default::default() }, store.clone());
    let id = engine.add(server.options(dir.path())).unwrap();
    wait_partial(&engine, id);
    pause_finished(&engine, id);
    let path = dir.path().join("file.bin");
    let part = dir.path().join("file.bin.swoop");
    let original = std::fs::read(&part).unwrap();
    let record = store.load_tasks().unwrap().remove(0);
    let downloaded = server.downloads.load(Ordering::SeqCst);
    *server.body.lock().unwrap() = vec![8; 2 * 1024 * 1024];
    let connection = reject_downloading_updates(&db);
    engine.resume(id).unwrap();
    wait_for(&engine, id, |status| matches!(status, TaskStatus::Failed(_)));
    wait_released(&engine, &path);
    assert_eq!(std::fs::read(&part).unwrap(), original);
    assert_eq!(server.downloads.load(Ordering::SeqCst), downloaded);
    let failed = store.load_tasks().unwrap().remove(0);
    assert_eq!(failed.total, record.total);
    assert_eq!(failed.etag, record.etag);
    assert_eq!(failed.downloaded, record.downloaded);
    assert_eq!(failed.segments.total_downloaded(), record.segments.total_downloaded());
    connection.execute_batch("DROP TRIGGER reject_download;").unwrap();
    engine.resume(id).unwrap();
    wait_for(&engine, id, |status| *status == TaskStatus::Completed);
    assert_eq!(std::fs::read(path).unwrap(), vec![8; 2 * 1024 * 1024]);
}

#[test]
fn deleting_task_with_persisted_recovery_path_preserves_both_files() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 64 * 1024]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.bin");
    let recovery = dir.path().join(".swoop-recovery-owned");
    let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    let id = engine.add(server.options(dir.path())).unwrap();
    wait_for(&engine, id, |status| *status == TaskStatus::Completed);
    wait_released(&engine, &path);
    let mut record = store.load_tasks().unwrap().remove(0);
    record.sync_state.recovery_path = Some(recovery.clone());
    record.sync_state.pending = true;
    store.save_task(&record).unwrap();
    std::fs::write(&recovery, b"original moved aside during publication").unwrap();
    drop(engine);
    let restored = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    restored.load(false).unwrap();
    for delete_file in [false, true] {
        let error = restored.remove(id, delete_file).unwrap_err().to_string();
        assert!(error.contains(recovery.to_str().unwrap()));
        assert!(restored.snapshot_one(id).is_some());
        assert_eq!(store.load_tasks().unwrap().len(), 1);
    }
    assert_eq!(std::fs::read(&path).unwrap(), vec![7; 64 * 1024]);
    assert_eq!(std::fs::read(&recovery).unwrap(), b"original moved aside during publication");
    // 文件已经由恢复流程处理，但内存仍带旧标记时，允许移除记录。
    std::fs::remove_file(&recovery).unwrap();
    restored.remove(id, false).unwrap();
    assert!(restored.snapshot_one(id).is_none());
    assert!(store.load_tasks().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), vec![7; 64 * 1024]);
}

#[test]
fn part_file_hides_progress_and_publication_never_clobbers_a_late_user_file() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = vec![7; 1024 * 1024];
    let server = Server::start(body.clone());
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
    let id = engine.add(server.options(dir.path())).unwrap();
    wait_partial(&engine, id);
    let (path, part) = (dir.path().join("file.bin"), dir.path().join("file.bin.swoop"));
    assert!(!path.exists());
    assert!(part.is_file());
    // 下载期间别的程序占用了目标名：完成时顺延编号，不覆盖用户文件。
    std::fs::write(&path, b"user").unwrap();
    wait_for(&engine, id, |status| *status == TaskStatus::Completed);
    let task = engine.snapshot_one(id).unwrap();
    assert_eq!(task.file_name, "file (1).bin");
    assert_eq!(std::fs::read(&task.path).unwrap(), body);
    assert_eq!(std::fs::read(&path).unwrap(), b"user");
    assert!(!part.exists());
}

#[test]
fn removing_a_running_download_deletes_its_part_file_only_when_asked() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = Server::start(vec![7; 1024 * 1024]);
    for delete in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(rt.handle().clone(), Config { connections: 1, ..Default::default() });
        let id = engine.add(server.options(dir.path())).unwrap();
        wait_partial(&engine, id);
        let (path, part) = (dir.path().join("file.bin"), dir.path().join("file.bin.swoop"));
        engine.remove(id, delete).unwrap();
        wait_released(&engine, &path);
        assert_eq!(part.exists(), !delete);
        assert!(!path.exists());
    }
}

#[test]
fn legacy_tasks_without_part_file_keep_resuming_in_place() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = vec![7; 1024 * 1024];
    let server = Server::start(body.clone());
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine =
        Engine::with_store(rt.handle().clone(), Config { connections: 1, ..Default::default() }, store.clone());
    let id = engine.add(server.options(dir.path())).unwrap();
    wait_partial(&engine, id);
    pause_finished(&engine, id);
    drop(engine);
    // 模拟升级前的任务：半成品直接写在目标文件名上，库里 part_file 为默认值 0。
    let (path, part) = (dir.path().join("file.bin"), dir.path().join("file.bin.swoop"));
    std::fs::rename(&part, &path).unwrap();
    rusqlite::Connection::open(&db).unwrap().execute_batch("UPDATE tasks SET part_file = 0").unwrap();
    let restored = Engine::with_store(rt.handle().clone(), Config { connections: 1, ..Default::default() }, store);
    restored.load(false).unwrap();
    restored.resume(id).unwrap();
    wait_for(&restored, id, |status| *status == TaskStatus::Completed);
    assert_eq!(restored.snapshot_one(id).unwrap().file_name, "file.bin");
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(!part.exists());
}
