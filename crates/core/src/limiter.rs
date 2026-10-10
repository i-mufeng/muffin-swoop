//! 全局令牌桶限速。rate == 0 表示不限速。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

pub struct Limiter {
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

struct Bucket {
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
    /// 不足时"预扣"并按欠额睡眠，避免多个 worker 之间饥饿。
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
            b.tokens = (b.tokens + elapsed * rate).min(rate);
            if b.tokens >= n {
                b.tokens -= n;
                return;
            }
            let deficit = n - b.tokens;
            b.tokens = 0.0;
            Duration::from_secs_f64(deficit / rate)
        };
        tokio::time::sleep(wait).await;
    }
}
