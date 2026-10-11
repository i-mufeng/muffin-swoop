//! Shared token bucket. Waiting owns no network read deadline and is cancellable.
use crate::MediaError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) struct Limiter {
    rate: AtomicU64,
    state: Mutex<State>,
    changed: Notify,
}
struct State {
    tokens: f64,
    updated: Instant,
}
impl Limiter {
    pub fn new(rate: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate),
            state: Mutex::new(State { tokens: 0.0, updated: Instant::now() }),
            changed: Notify::new(),
        }
    }
    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Acquire)
    }
    pub fn set_rate(&self, rate: u64) {
        self.rate.store(rate, Ordering::Release);
        self.changed.notify_waiters();
    }
    pub fn quantum(&self) -> usize {
        match self.rate() {
            0 => 16 * 1024,
            rate => (rate / 10).clamp(1, 16 * 1024) as usize,
        }
    }
    pub async fn acquire(&self, bytes: usize, cancel: &CancellationToken) -> Result<(), MediaError> {
        let mut remaining = bytes;
        let mut state = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(MediaError::Cancelled),
            state = self.state.lock() => state,
        };
        while remaining > 0 {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if cancel.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let rate = self.rate();
            if rate == 0 {
                state.tokens = 0.0;
                state.updated = Instant::now();
                return Ok(());
            }
            let now = Instant::now();
            let capacity = (rate / 10).clamp(1, 16 * 1024) as f64;
            state.tokens = (state.tokens + now.duration_since(state.updated).as_secs_f64() * rate as f64).min(capacity);
            state.updated = now;
            let take = remaining.min(capacity as usize);
            if state.tokens >= take as f64 {
                state.tokens -= take as f64;
                remaining -= take;
                continue;
            }
            let delay = Duration::from_secs_f64(((take as f64 - state.tokens) / rate as f64).clamp(0.001, 3600.0));
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(MediaError::Cancelled),
                _ = &mut changed => {},
                _ = tokio::time::sleep(delay) => {},
            }
        }
        Ok(())
    }
}
