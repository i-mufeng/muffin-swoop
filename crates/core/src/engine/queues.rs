use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use crate::queue::{MAIN_QUEUE_ID, MAX_QUEUE_CONCURRENCY, QueueId, QueueRecord, QueueSchedule, QueueSnapshot};
use crate::{Error, Result, TaskId, TaskStatus};

use super::{Engine, Entry, Inner, persist, spawn_run, unix_secs};

pub(super) fn persist_queues(inner: &Inner, queues: &BTreeMap<QueueId, QueueRecord>) -> Result<()> {
    if let Some(store) = &inner.store {
        store.save_queues(queues.values())?;
    }
    Ok(())
}

fn valid_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(Error::InvalidQueue("名称需要 1–80 个字符".into()));
    }
    Ok(name.to_owned())
}

fn park(inner: &Inner, entry: &mut Entry) {
    if entry.shared.status().is_active() {
        entry.shared.queue_again();
        entry.shared.set_speed(0.0);
        entry.cancel.cancel();
        persist(inner, &entry.shared);
    }
}

impl Engine {
    pub fn queues(&self) -> Vec<QueueSnapshot> {
        let tasks = self.inner.tasks.lock().unwrap();
        let queues = self.inner.queues.lock().unwrap();
        let slots = self.inner.run_slots.lock().unwrap();
        queues
            .values()
            .map(|q| QueueSnapshot {
                id: q.id,
                name: q.name.clone(),
                max_concurrent: q.max_concurrent,
                running: q.running,
                sync_enabled: q.sync_enabled,
                task_ids: q.task_ids.clone(),
                schedule: q.schedule.clone(),
                next_start_at: q.next_start_at,
                next_stop_at: q.next_stop_at,
                active_count: slots.values().filter(|id| **id == q.id).count(),
                waiting_count: q
                    .task_ids
                    .iter()
                    .filter(|id| {
                        tasks.get(id).is_some_and(|entry| !entry.running && entry.shared.status() == TaskStatus::Queued)
                    })
                    .count(),
            })
            .collect()
    }

    pub fn queue_for_task(&self, task: TaskId) -> Option<QueueId> {
        self.inner.queues.lock().unwrap().values().find(|q| q.task_ids.contains(&task)).map(|q| q.id)
    }

    /// 新命名队列默认停止，可先整理任务及顺序，再手动或定时启动。
    pub fn create_queue(&self, name: impl AsRef<str>, max_concurrent: usize) -> Result<QueueId> {
        let name = valid_name(name.as_ref())?;
        let mut queues = self.inner.queues.lock().unwrap();
        if queues.values().any(|q| q.name == name) {
            return Err(Error::InvalidQueue("队列名称已存在".into()));
        }
        let id = QueueId(queues.keys().map(|id| id.0).max().unwrap_or(0) + 1);
        let now = unix_secs(self.inner.queue_clock.now());
        let mut updated = queues.clone();
        updated.insert(
            id,
            QueueRecord {
                id,
                name,
                max_concurrent: max_concurrent.clamp(1, MAX_QUEUE_CONCURRENCY),
                running: false,
                sync_enabled: false,
                task_ids: Vec::new(),
                schedule: None,
                next_start_at: None,
                next_stop_at: None,
                created_at: now,
                updated_at: now,
            },
        );
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        Ok(id)
    }

    pub fn rename_queue(&self, id: QueueId, name: impl AsRef<str>) -> Result<()> {
        let name = valid_name(name.as_ref())?;
        self.update_queue(id, |q, all| {
            if all.values().any(|other| other.id != id && other.name == name) {
                return Err(Error::InvalidQueue("队列名称已存在".into()));
            }
            q.name = name;
            Ok(())
        })
    }

    /// 降低上限不打断现有下载；空出的槽位达到新上限后才继续启动等待任务。
    pub fn set_queue_concurrency(&self, id: QueueId, max_concurrent: usize) -> Result<()> {
        self.update_queue(id, |q, _| {
            q.max_concurrent = max_concurrent.clamp(1, MAX_QUEUE_CONCURRENCY);
            Ok(())
        })
    }

    fn update_queue(
        &self,
        id: QueueId,
        update: impl FnOnce(&mut QueueRecord, &BTreeMap<QueueId, QueueRecord>) -> Result<()>,
    ) -> Result<()> {
        let mut queues = self.inner.queues.lock().unwrap();
        let mut updated = queues.clone();
        let q = updated.get_mut(&id).ok_or(Error::QueueNotFound)?;
        update(q, &queues)?;
        q.updated_at = unix_secs(self.inner.queue_clock.now());
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    pub fn set_queue_sync_enabled(&self, id: QueueId, enabled: bool) -> Result<()> {
        self.update_queue(id, |q, _| {
            q.sync_enabled = enabled;
            Ok(())
        })
    }

    pub fn start_queue(&self, id: QueueId) -> Result<()> {
        self.start_queue_inner(id, false).map(|_| ())
    }

    /// 手动检查已完成成员；同时启动队列。失败的同步可重试，单独暂停的任务不启动。
    pub fn sync_queue(&self, id: QueueId) -> Result<usize> {
        self.start_queue_inner(id, true)
    }

    fn start_queue_inner(&self, id: QueueId, force_sync: bool) -> Result<usize> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        self.require_admission()?;
        let mut queues = self.inner.queues.lock().unwrap();
        let mut updated = queues.clone();
        let queue = updated.get_mut(&id).ok_or(Error::QueueNotFound)?;
        queue.running = true;
        queue.updated_at = unix_secs(self.inner.queue_clock.now());
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        let queue = &queues[&id];
        let count =
            if force_sync || queue.sync_enabled { queue_sync(&self.inner, &mut tasks, &queue.task_ids)? } else { 0 };
        self.inner.queue_wake.notify_one();
        Ok(count)
    }

    /// 停止在途任务并保留队列等待状态，单独暂停的任务仍保持 Paused。
    pub fn stop_queue(&self, id: QueueId) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let mut queues = self.inner.queues.lock().unwrap();
        let mut updated = queues.clone();
        let queue = updated.get_mut(&id).ok_or(Error::QueueNotFound)?;
        queue.running = false;
        queue.updated_at = unix_secs(self.inner.queue_clock.now());
        let ids = queue.task_ids.clone();
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        for id in ids {
            if let Some(entry) = tasks.get_mut(&id) {
                park(&self.inner, entry);
            }
        }
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    /// 删除命名队列，其成员迁到主队列末尾；不删除任务或文件。
    pub fn delete_queue(&self, id: QueueId) -> Result<()> {
        if id == MAIN_QUEUE_ID {
            return Err(Error::InvalidQueue("不能删除主队列".into()));
        }
        let mut tasks = self.inner.tasks.lock().unwrap();
        let mut queues = self.inner.queues.lock().unwrap();
        let mut updated = queues.clone();
        let removed = updated.remove(&id).ok_or(Error::QueueNotFound)?;
        updated.get_mut(&MAIN_QUEUE_ID).ok_or(Error::QueueNotFound)?.task_ids.extend(&removed.task_ids);
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        for id in removed.task_ids {
            if let Some(entry) = tasks.get_mut(&id) {
                park(&self.inner, entry);
            }
        }
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    /// 任务移动到目标队列末尾；在途任务先收尾，再按目标队列并发限制继续。
    pub fn move_to_queue(&self, task: TaskId, target: QueueId) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get_mut(&task).ok_or(Error::TaskNotFound)?;
        let mut queues = self.inner.queues.lock().unwrap();
        if !queues.contains_key(&target) {
            return Err(Error::QueueNotFound);
        }
        if queues.get(&target).is_some_and(|q| q.task_ids.contains(&task)) {
            return Ok(());
        }
        let mut updated = queues.clone();
        for q in updated.values_mut() {
            q.task_ids.retain(|id| *id != task);
        }
        updated.get_mut(&target).unwrap().task_ids.push(task);
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        park(&self.inner, entry);
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    /// 移出后暂停；以后显式 resume 时重新加入主队列。
    pub fn remove_from_queue(&self, task: TaskId) -> Result<()> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let entry = tasks.get_mut(&task).ok_or(Error::TaskNotFound)?;
        let mut queues = self.inner.queues.lock().unwrap();
        let mut updated = queues.clone();
        for q in updated.values_mut() {
            q.task_ids.retain(|id| *id != task);
        }
        persist_queues(&self.inner, &updated)?;
        *queues = updated;
        if entry.shared.mark_paused() {
            entry.cancel.cancel();
            entry.shared.set_speed(0.0);
            persist(&self.inner, &entry.shared);
        }
        self.inner.queue_wake.notify_one();
        Ok(())
    }

    /// order 必须包含该队列全部成员且每个恰好一次；不会中断已运行任务。
    pub fn reorder_queue(&self, id: QueueId, order: &[TaskId]) -> Result<()> {
        self.update_queue(id, |q, _| {
            let current: BTreeSet<_> = q.task_ids.iter().copied().collect();
            let next: BTreeSet<_> = order.iter().copied().collect();
            if current != next || order.len() != next.len() {
                return Err(Error::InvalidQueue("排序必须包含队列的全部任务，且不得重复".into()));
            }
            q.task_ids = order.to_vec();
            Ok(())
        })
    }

    pub fn set_queue_schedule(&self, id: QueueId, schedule: Option<QueueSchedule>) -> Result<()> {
        if let Some(schedule) = &schedule {
            schedule.validate()?;
        }
        self.update_queue(id, |q, _| {
            q.next_start_at = schedule.as_ref().and_then(|s| s.start_at);
            q.next_stop_at = schedule.as_ref().and_then(|s| s.stop_at);
            q.schedule = schedule;
            Ok(())
        })
    }

    /// 立即按注入时钟消费调度边界，并尝试启动符合条件的等待任务。
    pub fn tick_queues(&self) -> Result<()> {
        dispatch(&self.inner)
    }
}

pub(super) fn start_loop(inner: &Arc<Inner>) {
    let weak = Arc::downgrade(inner);
    let wake = inner.queue_wake.clone();
    inner.handle.spawn(async move {
        loop {
            let Some(inner) = weak.upgrade() else { break };
            if let Err(e) = dispatch(&inner) {
                tracing::error!("队列调度失败: {e}");
            }
            drop(inner);
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
    });
}

fn dispatch(inner: &Arc<Inner>) -> Result<()> {
    let mut tasks = inner.tasks.lock().unwrap();
    if inner.quiescent.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let mut queues = inner.queues.lock().unwrap();
    let now = unix_secs(inner.queue_clock.now());
    let mut updated = queues.clone();
    let changed: Vec<_> = updated.values_mut().filter_map(|q| q.tick(now).then_some(q.id)).collect();
    if !changed.is_empty() {
        persist_queues(inner, &updated)?;
        *queues = updated;
        for id in changed {
            let q = &queues[&id];
            if q.running && q.sync_enabled {
                queue_sync(inner, &mut tasks, &q.task_ids)?;
            } else if !q.running {
                for task in &q.task_ids {
                    if let Some(entry) = tasks.get_mut(task) {
                        park(inner, entry);
                    }
                }
            }
        }
    }
    let mut slots = inner.run_slots.lock().unwrap();
    for q in queues.values().filter(|q| q.running) {
        let active = slots.values().filter(|id| **id == q.id).count();
        let mut free = q.max_concurrent.saturating_sub(active);
        for id in &q.task_ids {
            if free == 0 {
                break;
            }
            let Some(entry) = tasks.get_mut(id) else {
                continue;
            };
            if entry.running || entry.shared.status() != TaskStatus::Queued {
                continue;
            }
            entry.cancel = tokio_util::sync::CancellationToken::new();
            entry.running = true;
            entry.run_queue = Some(q.id);
            slots.insert(*id, q.id);
            spawn_run(inner.clone(), entry.shared.clone(), entry.cancel.clone(), entry.connections);
            free -= 1;
        }
    }
    Ok(())
}

fn queue_sync(inner: &Inner, tasks: &mut BTreeMap<TaskId, Entry>, ids: &[TaskId]) -> Result<usize> {
    let mut count = 0;
    for id in ids {
        let Some(entry) = tasks.get_mut(id) else {
            continue;
        };
        if entry.running {
            continue;
        }
        let state = entry.shared.sync_state.lock().unwrap().clone();
        if entry.shared.status() != TaskStatus::Completed
            && !(state.pending && matches!(entry.shared.status(), TaskStatus::Failed(_)))
        {
            continue;
        }
        let mut rec = super::record_of(&entry.shared);
        rec.sync_state.pending = true;
        rec.status = TaskStatus::Queued;
        if let Some(store) = &inner.store {
            store.save_task(&rec)?;
        }
        entry.shared.sync_state.lock().unwrap().pending = true;
        entry.shared.queue_again();
        count += 1;
    }
    Ok(count)
}
