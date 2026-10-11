//! 全局令牌桶限速。rate == 0 表示不限速。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
// 用 tokio 的时钟：测试里可以暂停/快进时间
use tokio::time::Instant;

pub struct Limiter {
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

struct Bucket {
    /// 可用配额（字节）；为负表示欠账，后来的申请者要先等欠账还清
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate_bps),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate_bps: u64) {
        self.rate.store(rate_bps, Ordering::Relaxed);
    }

    /// 申请 `n` 字节的配额，必要时睡眠等待。
    /// 不足时照样扣下（余额变负），按欠账睡眠：并发的 worker 依次排在欠账后面，
    /// 总速率才不会随连接数放大；也不会因为"先清零再等"把睡眠期间的配额重复记一遍。
    pub async fn acquire(&self, n: usize) {
        let rate = self.rate() as f64;
        if rate <= 0.0 {
            return;
        }
        let n = n as f64;
        let wait = {
            let mut b = self.bucket.lock().await;
            let now = Instant::now();
            let elapsed = now.duration_since(b.last).as_secs_f64();
            b.last = now;
            // 最多攒 1 秒的突发配额
            b.tokens = (b.tokens + elapsed * rate).min(rate) - n;
            if b.tokens >= 0.0 {
                return;
            }
            Duration::from_secs_f64(-b.tokens / rate)
        };
        tokio::time::sleep(wait).await;
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
