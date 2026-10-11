//! 全局令牌桶限速。rate == 0 表示不限速。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use std::sync::Mutex;
use tokio::sync::Notify;
// 用 tokio 的时钟：测试里可以暂停/快进时间
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct Limiter {
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
    changed: Notify,
}

struct Bucket {
    /// 可用配额（字节）；为负表示欠账，后来的申请者要先等欠账还清
    tokens: f64,
    last: Instant,
    generation: u64,
}

impl Limiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate_bps),
            bucket: Mutex::new(Bucket {
                tokens: 0.0,
                last: Instant::now(),
                generation: 0,
            }),
            changed: Notify::new(),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate_bps: u64) {
        let mut bucket = self.bucket.lock().unwrap();
        if self.rate() == rate_bps {
            return;
        }
        self.rate.store(rate_bps, Ordering::Relaxed);
        bucket.tokens = 0.0;
        bucket.last = Instant::now();
        bucket.generation = bucket.generation.wrapping_add(1);
        drop(bucket);
        self.changed.notify_waiters();
    }

    /// 申请 `n` 字节的配额，必要时睡眠等待。
    /// 不足时照样扣下（余额变负），按欠账睡眠：并发的 worker 依次排在欠账后面，
    /// 总速率才不会随连接数放大；也不会因为"先清零再等"把睡眠期间的配额重复记一遍。
    pub async fn acquire(&self, n: usize) {
        self.acquire_cancellable(n, &CancellationToken::new()).await;
    }

    /// 队列停止或单任务暂停时，立即取消等待并归还本次预扣配额。
    pub(crate) async fn acquire_cancellable(&self, n: usize, cancel: &CancellationToken) -> bool {
        let n = n as f64;
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // 在读取配额之前订阅，避免 set_rate 发生在读取和 select 之间丢失唤醒。
            changed.as_mut().enable();
            let (wait, generation) = {
                let mut b = self.bucket.lock().unwrap();
                let rate = self.rate() as f64;
                if rate <= 0.0 {
                    return true;
                }
                let now = Instant::now();
                let elapsed = now.duration_since(b.last).as_secs_f64();
                b.last = now;
                b.tokens = (b.tokens + elapsed * rate).min(rate) - n;
                if b.tokens >= 0.0 {
                    return true;
                }
                (Duration::from_secs_f64(-b.tokens / rate), b.generation)
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let mut b = self.bucket.lock().unwrap();
                    if b.generation == generation { b.tokens = (b.tokens + n).min(self.rate() as f64); }
                    return false;
                }
                _ = &mut changed => {
                    // 新限速已重置令牌桶；旧等待重新申请，不能把旧代次配额退进新桶。
                }
                _ = tokio::time::sleep(wait) => return true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    const KB: usize = 1024;
    const MB: u64 = 1024 * 1024;

    /// 在暂停的时钟下申请 `total` 字节（每次 `chunk`），返回耗时秒数
    async fn drain(limiter: Arc<Limiter>, workers: usize, total: usize, chunk: usize) -> f64 {
        let t0 = Instant::now();
        let per = total / workers;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..workers {
            let l = limiter.clone();
            set.spawn(async move {
                let mut left = per;
                while left > 0 {
                    let n = chunk.min(left);
                    l.acquire(n).await;
                    left -= n;
                }
            });
        }
        set.join_all().await;
        t0.elapsed().as_secs_f64()
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_wait_returns_its_unspent_budget() {
        let limiter = Arc::new(Limiter::new(1024));
        let cancel = CancellationToken::new();
        let worker = {
            let limiter = limiter.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { limiter.acquire_cancellable(1024, &cancel).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        assert!(!worker.await.unwrap());
        let start = Instant::now();
        limiter.acquire(1024).await;
        let elapsed = start.elapsed().as_secs_f64();
        assert!(
            (0.8..1.1).contains(&elapsed),
            "取消后仍残留配额欠账: {elapsed}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn changing_the_rate_wakes_existing_waits_immediately() {
        let limiter = Arc::new(Limiter::new(1));
        let waiter = {
            let limiter = limiter.clone();
            tokio::spawn(async move { limiter.acquire(1024).await })
        };
        tokio::task::yield_now().await;
        limiter.set_rate(0);
        tokio::time::timeout(Duration::from_millis(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn single_worker_holds_rate() {
        // 新建时余额为 0：3 MB 在 1 MB/s 下应接近 3 秒（最后一块无需等待，略少于 3 秒）
        let l = Arc::new(Limiter::new(MB));
        let secs = drain(l, 1, 3 * MB as usize, 16 * KB).await;
        assert!((2.9..=3.1).contains(&secs), "耗时 {secs}");
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_workers_share_rate() {
        // 8 个 worker 合计 3 MB：总速率仍是 1 MB/s，而不是 8 倍
        let l = Arc::new(Limiter::new(MB));
        let secs = drain(l, 8, 3 * MB as usize, 16 * KB).await;
        assert!((2.9..=3.1).contains(&secs), "耗时 {secs}");
    }

    #[tokio::test(start_paused = true)]
    async fn idle_allows_one_second_burst() {
        // 空闲久了最多攒 1 秒配额：1.5 MB 中前 1 MB 立即放行，剩下 0.5 MB 约 0.5 秒
        let l = Arc::new(Limiter::new(MB));
        tokio::time::sleep(Duration::from_secs(10)).await;
        let secs = drain(l, 4, 3 * MB as usize / 2, 16 * KB).await;
        assert!((0.4..=0.6).contains(&secs), "耗时 {secs}");
    }

    #[tokio::test(start_paused = true)]
    async fn zero_rate_is_unlimited() {
        let l = Arc::new(Limiter::new(0));
        let secs = drain(l, 4, 100 * MB as usize, 64 * KB).await;
        assert_eq!(secs, 0.0);
    }
}
