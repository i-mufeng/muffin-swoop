use std::sync::Arc;
use swoop_core::{AddOptions, Config, Engine, MAIN_QUEUE_ID, TaskStatus};

#[test]
fn removed_task_must_finish_its_worker_before_idle_guard_can_be_acquired() {
    // A current-thread runtime lets us deterministically hold a started worker before its cleanup.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let engine = Engine::new(runtime.handle().clone(), Config::default());
    let root = tempfile::tempdir().unwrap();
    let id = engine
        .add(AddOptions {
            url: "http://127.0.0.1:1/file".into(),
            dir: root.path().into(),
            file_name: Some("file".into()),
            headers: vec![],
        })
        .unwrap();
    engine.tick_queues().unwrap();
    engine.remove(id, false).unwrap();
    assert!(engine.snapshot().is_empty());
    assert!(engine.try_quiesce().is_none(), "removed entry still has an unfinished worker");
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if engine.try_quiesce().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    });
}

#[test]
fn idle_guard_blocks_concurrent_admission_and_resumes_without_changing_saved_queue_state() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let engine = Arc::new(Engine::new(runtime.handle().clone(), Config::default()));
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let root = tempfile::tempdir().unwrap();
    let options = || AddOptions {
        url: "http://127.0.0.1:1/file".into(),
        dir: root.path().into(),
        file_name: Some("file".into()),
        headers: vec![],
    };
    let task = engine.add(options()).unwrap();
    assert!(engine.try_quiesce().is_none());
    engine.pause(task).unwrap();
    let guard = engine.try_quiesce().unwrap();
    assert!(engine.try_quiesce().is_none());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                assert!(engine.add(options()).is_err());
                assert!(engine.resume(task).is_err());
                assert!(engine.start_queue(MAIN_QUEUE_ID).is_err());
                engine.tick_queues().unwrap();
            });
        }
    });
    assert_eq!(engine.snapshot().len(), 1);
    assert_eq!(engine.snapshot_one(task).unwrap().status, TaskStatus::Paused);
    assert!(!engine.queues()[0].running);
    drop(guard);
    engine.resume(task).unwrap();
    assert_eq!(engine.snapshot_one(task).unwrap().status, TaskStatus::Queued);
    engine.pause(task).unwrap();
    engine.try_quiesce().unwrap().commit();
    assert!(engine.add(options()).is_err());
    assert!(engine.start_queue(MAIN_QUEUE_ID).is_err());
}
