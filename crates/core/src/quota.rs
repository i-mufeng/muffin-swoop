//! 全任务共享的滚动时间窗口额度。探测不计费，定位写入前持久化预留，重启不清账。
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{Error, QueueClock, Result, Store};

const SETTING_KEY: &str = "core.traffic_quota";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuotaPolicy {
    /// 0 表示关闭额度限制；关闭后仍保留本窗口已经发生的计费。
    pub max_bytes: u64,
    pub window_secs: u64,
}
impl Default for QuotaPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 0,
            window_secs: 86400,
        }
    }
}
impl QuotaPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=31_536_000).contains(&self.window_secs) {
            return Err(Error::InvalidQuota(
                "额度窗口需要介于 1 秒和 365 天之间".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct QuotaSnapshot {
    pub policy: QuotaPolicy,
    pub window_started_at: i64,
    pub resets_at: i64,
    pub used_bytes: u64,
    pub remaining_bytes: Option<u64>,
    pub waiting: bool,
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Ledger {
    policy: QuotaPolicy,
    window_started_at: i64,
    /// 磁盘账本包含尚未提交的预留，崩溃后保守计入已用量。
    used_bytes: u64,
}
struct State {
    ledger: Ledger,
    reserved: u64,
    error: Option<String>,
}

pub(crate) struct Quota {
    state: Mutex<State>,
    cached_snapshot: Mutex<QuotaSnapshot>,
    changed: Notify,
    store: Option<Arc<Store>>,
    clock: Arc<dyn QueueClock>,
}

impl Quota {
    pub fn new(store: Option<Arc<Store>>, clock: Arc<dyn QueueClock>) -> Arc<Self> {
        let now = unix_secs(clock.as_ref());
        let loaded = (|| -> Result<Option<Ledger>> {
            let Some(store) = &store else { return Ok(None) };
            let Some(value) = store.get_setting(SETTING_KEY)? else {
                return Ok(None);
            };
            let ledger: Ledger = serde_json::from_str(&value)?;
            ledger.policy.validate()?;
            if ledger.window_started_at < 0 {
                return Err(Error::InvalidQuota("额度窗口起点无效".into()));
            }
            Ok(Some(ledger))
        })();
        let (ledger, error) = match loaded {
            Ok(Some(ledger)) => (ledger, None),
            Ok(None) => (
                Ledger {
                    policy: QuotaPolicy::default(),
                    window_started_at: now,
                    used_bytes: 0,
                },
                None,
            ),
            Err(error) => (
                Ledger {
                    policy: QuotaPolicy::default(),
                    window_started_at: now,
                    used_bytes: 0,
                },
                Some(format!("读取额度账本失败: {error}")),
            ),
        };
        let state = State {
            ledger,
            reserved: 0,
            error,
        };
        let cached_snapshot = Self::snapshot_of(&state, now);
        Arc::new(Self {
            state: Mutex::new(state),
            cached_snapshot: Mutex::new(cached_snapshot),
            changed: Notify::new(),
            store,
            clock,
        })
    }

    fn persist(&self, ledger: &Ledger, reserved: u64) -> Result<()> {
        if let Some(store) = &self.store {
            let mut durable = ledger.clone();
            durable.used_bytes = durable
                .used_bytes
                .checked_add(reserved)
                .ok_or_else(|| Error::InvalidQuota("额度计数超出范围".into()))?;
            store.set_setting(SETTING_KEY, &serde_json::to_string(&durable)?)?;
        }
        Ok(())
    }

    fn refresh(&self, state: &mut State) -> Result<()> {
        if let Some(error) = &state.error {
            return Err(Error::InvalidQuota(error.clone()));
        }
        let now = unix_secs(self.clock.as_ref());
        let period = state.ledger.policy.window_secs as i64;
        let elapsed = now.saturating_sub(state.ledger.window_started_at);
        if elapsed >= period {
            let mut updated = state.ledger.clone();
            updated.window_started_at = updated
                .window_started_at
                .saturating_add((elapsed / period) * period);
            updated.used_bytes = 0;
            self.persist(&updated, 0)?;
            state.ledger = updated;
            state.reserved = 0;
            self.changed.notify_waiters();
        }
        Ok(())
    }

    fn snapshot_of(state: &State, now: i64) -> QuotaSnapshot {
        let mut ledger = state.ledger.clone();
        let mut reserved = state.reserved;
        let period = ledger.policy.window_secs as i64;
        let elapsed = now.saturating_sub(ledger.window_started_at);
        // 只计算显示状态，不在 UI 读取路径执行 SQLite I/O；真正获取额度时先持久化窗口。
        if state.error.is_none() && elapsed >= period {
            ledger.window_started_at = ledger
                .window_started_at
                .saturating_add((elapsed / period) * period);
            ledger.used_bytes = 0;
            reserved = 0;
        }
        let charged = ledger.used_bytes.saturating_add(reserved);
        let remaining =
            (ledger.policy.max_bytes > 0).then(|| ledger.policy.max_bytes.saturating_sub(charged));
        QuotaSnapshot {
            policy: ledger.policy.clone(),
            window_started_at: ledger.window_started_at,
            resets_at: ledger.window_started_at.saturating_add(period),
            used_bytes: ledger.used_bytes,
            remaining_bytes: remaining,
            waiting: state.error.is_some() || remaining == Some(0),
            error: state.error.clone(),
        }
    }

    fn cache(&self, state: &State) {
        *self.cached_snapshot.lock().unwrap() =
            Self::snapshot_of(state, unix_secs(self.clock.as_ref()));
    }

    /// UI 不等后台磁盘写入或 SQLite 事务；繁忙时返回最近一次完整快照。
    pub fn snapshot(&self) -> QuotaSnapshot {
        if let Ok(state) = self.state.try_lock() {
            self.cache(&state);
        }
        self.cached_snapshot.lock().unwrap().clone()
    }

    pub fn set_policy(&self, policy: QuotaPolicy) -> Result<()> {
        policy.validate()?;
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        let mut updated = state.ledger.clone();
        updated.policy = policy;
        self.persist(&updated, state.reserved)?;
        state.ledger = updated;
        self.cache(&state);
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }

    /// 返回本次可写入的字节数，可能小于请求量；调用者保留剩余响应体等待下一窗口。
    pub async fn acquire(
        self: &Arc<Self>,
        bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<Permit> {
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                self.refresh(&mut state)?;
                let charged = state.ledger.used_bytes.saturating_add(state.reserved);
                let available = if state.ledger.policy.max_bytes == 0 {
                    u64::MAX - charged
                } else {
                    state.ledger.policy.max_bytes.saturating_sub(charged)
                };
                let granted = available.min(bytes as u64);
                if granted > 0 {
                    let reserved = state
                        .reserved
                        .checked_add(granted)
                        .ok_or_else(|| Error::InvalidQuota("额度预留超出范围".into()))?;
                    self.persist(&state.ledger, reserved)?;
                    state.reserved = reserved;
                    self.cache(&state);
                    return Ok(Permit {
                        quota: self.clone(),
                        bytes: granted,
                        window: state.ledger.window_started_at,
                        settled: false,
                    });
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                _ = &mut changed => {},
                // 系统时钟和测试注入时钟都会在下次轮询生效；不占用 UI 线程。
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            }
        }
    }

    fn settle_locked(&self, state: &mut State, bytes: u64, written: u64) -> Result<()> {
        let reserved = state.reserved.saturating_sub(bytes);
        let mut updated = state.ledger.clone();
        updated.used_bytes = updated
            .used_bytes
            .checked_add(written)
            .ok_or_else(|| Error::InvalidQuota("额度计数超出范围".into()))?;
        // 全量写入只把预留转成已用，磁盘总额不变。
        if written < bytes
            && let Err(error) = self.persist(&updated, reserved)
        {
            state.error = Some(format!("返还未写入额度失败: {error}"));
            self.cache(state);
            return Err(error);
        }
        state.ledger = updated;
        state.reserved = reserved;
        self.cache(state);
        self.changed.notify_waiters();
        Ok(())
    }

    fn settle(&self, bytes: u64, written: u64, window: i64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.ledger.window_started_at != window {
            return Ok(());
        }
        self.settle_locked(&mut state, bytes, written)
    }
}

pub(crate) struct Permit {
    quota: Arc<Quota>,
    bytes: u64,
    window: i64,
    settled: bool,
}
impl Permit {
    pub fn bytes(&self) -> usize {
        self.bytes as usize
    }
    /// 写入和结算与窗口切换互斥；若等待限速时窗口过期，调用者重新取得本窗口配额。
    pub fn write(
        mut self,
        write: impl FnOnce(usize) -> (usize, std::io::Result<()>),
    ) -> Result<Option<std::io::Result<()>>> {
        let mut state = self.quota.state.lock().unwrap();
        self.quota.refresh(&mut state)?;
        if state.ledger.window_started_at != self.window {
            return Ok(None);
        }
        // 等待限速期间可能调低额度：旧许可也必须遵守当前上限，不能超支新策略。
        let allowed = if state.ledger.policy.max_bytes == 0 {
            self.bytes
        } else {
            self.bytes.min(
                state
                    .ledger
                    .policy
                    .max_bytes
                    .saturating_sub(state.ledger.used_bytes),
            )
        };
        if allowed == 0 {
            self.settled = true;
            self.quota.settle_locked(&mut state, self.bytes, 0)?;
            return Ok(None);
        }
        let (written, result) = write(allowed as usize);
        self.settled = true;
        self.quota
            .settle_locked(&mut state, self.bytes, written as u64)?;
        Ok(Some(result))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if !self.settled
            && let Err(error) = self.quota.settle(self.bytes, 0, self.window)
        {
            tracing::error!("返还下载额度失败: {error}");
        }
    }
}
fn unix_secs(clock: &dyn QueueClock) -> i64 {
    clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::SystemTime;

    struct Clock(AtomicI64);
    impl QueueClock for Clock {
        fn now(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(self.0.load(Ordering::SeqCst) as u64)
        }
    }

    #[test]
    fn ui_snapshots_do_not_wait_for_a_busy_writer() {
        let quota = Quota::new(None, Arc::new(Clock(AtomicI64::new(1000))));
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 8,
                window_secs: 10,
            })
            .unwrap();
        let guard = quota.state.lock().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = quota.clone();
        let thread = std::thread::spawn(move || {
            send.send(reader.snapshot()).unwrap();
        });
        let snapshot = receive
            .recv_timeout(Duration::from_millis(200))
            .expect("UI 快照被后台写锁阻塞");
        assert_eq!(snapshot.policy.max_bytes, 8);
        drop(guard);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn unused_reservations_are_returned_and_policy_changes_do_not_clear_usage() {
        let quota = Quota::new(None, Arc::new(Clock(AtomicI64::new(1000))));
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 8,
                window_secs: 10,
            })
            .unwrap();
        let cancel = CancellationToken::new();
        let permit = quota.acquire(6, &cancel).await.unwrap();
        assert_eq!(quota.snapshot().remaining_bytes, Some(2));
        drop(permit);
        assert_eq!(quota.snapshot().remaining_bytes, Some(8));
        quota
            .acquire(6, &cancel)
            .await
            .unwrap()
            .write(|_| (4, Ok(())))
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(quota.snapshot().used_bytes, 4);
        assert_eq!(quota.snapshot().remaining_bytes, Some(4));
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 0,
                window_secs: 10,
            })
            .unwrap();
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 3,
                window_secs: 10,
            })
            .unwrap();
        assert_eq!(quota.snapshot().used_bytes, 4);
        assert!(quota.snapshot().waiting);
        cancel.cancel();
        assert!(matches!(
            quota.acquire(1, &cancel).await,
            Err(Error::Cancelled)
        ));
    }

    #[tokio::test]
    async fn reducing_the_limit_also_bounds_already_reserved_writes() {
        let quota = Quota::new(None, Arc::new(Clock(AtomicI64::new(1000))));
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 8,
                window_secs: 10,
            })
            .unwrap();
        let permit = quota.acquire(8, &CancellationToken::new()).await.unwrap();
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 3,
                window_secs: 10,
            })
            .unwrap();
        permit
            .write(|allowed| {
                assert_eq!(allowed, 3);
                (allowed, Ok(()))
            })
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(quota.snapshot().used_bytes, 3);
        assert_eq!(quota.snapshot().remaining_bytes, Some(0));
    }

    #[tokio::test]
    async fn expired_reservations_cannot_write_into_the_new_window() {
        let clock = Arc::new(Clock(AtomicI64::new(1000)));
        let quota = Quota::new(None, clock.clone());
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 4,
                window_secs: 10,
            })
            .unwrap();
        let permit = quota.acquire(4, &CancellationToken::new()).await.unwrap();
        clock.0.store(1010, Ordering::SeqCst);
        assert!(
            permit
                .write(|_| panic!("旧窗口许可不能写新窗口"))
                .unwrap()
                .is_none()
        );
        assert_eq!(quota.snapshot().used_bytes, 0);
        assert_eq!(quota.snapshot().remaining_bytes, Some(4));
    }

    #[tokio::test]
    async fn restart_counts_durable_reservations_and_corrupt_ledgers_fail_closed() {
        let store = Arc::new(Store::in_memory().unwrap());
        let clock = Arc::new(Clock(AtomicI64::new(1000)));
        let quota = Quota::new(Some(store.clone()), clock.clone());
        quota
            .set_policy(QuotaPolicy {
                max_bytes: 8,
                window_secs: 10,
            })
            .unwrap();
        let permit = quota.acquire(8, &CancellationToken::new()).await.unwrap();
        // 模拟进程在写入前崩溃：复制当时磁盘账本，不执行内存许可的 Drop。
        let crash_store = Arc::new(Store::in_memory().unwrap());
        crash_store
            .set_setting(
                SETTING_KEY,
                &store.get_setting(SETTING_KEY).unwrap().unwrap(),
            )
            .unwrap();
        drop(permit);
        let restarted = Quota::new(Some(crash_store.clone()), clock.clone());
        assert_eq!(restarted.snapshot().used_bytes, 8);
        assert!(restarted.snapshot().waiting);
        crash_store.set_setting(SETTING_KEY, "{broken").unwrap();
        let broken = Quota::new(Some(crash_store), clock);
        let error = broken.snapshot().error;
        assert!(error.is_some());
        assert_eq!(
            broken.snapshot().error,
            error,
            "状态读取不得反复叠加错误前缀"
        );
        assert!(matches!(
            broken.acquire(1, &CancellationToken::new()).await,
            Err(Error::InvalidQuota(_))
        ));
    }
}
