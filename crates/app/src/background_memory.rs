//! 每段连续的托盘空闲期仅回收一次驻留页，不周期性清空，也不影响下载中的热数据。
//! 此操作减少物理驻留内存，不能视为释放提交内存；渲染器的实际分配另行优化。
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct BackgroundMemory {
    idle_since: Option<Instant>,
    trimmed: bool,
}

impl BackgroundMemory {
    pub fn update(&mut self, hidden: bool, idle: bool) {
        if self.should_trim(hidden, idle, Instant::now()) {
            #[cfg(windows)]
            unsafe {
                // 当前进程伪句柄无需关闭；由 OS 决定哪些页能回收，唤回时按需载入。
                let process = windows_sys::Win32::System::Threading::GetCurrentProcess();
                if windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet(process) == 0 {
                    tracing::debug!("后台工作集回收未完成：{}", std::io::Error::last_os_error());
                }
            }
        }
    }

    fn should_trim(&mut self, hidden: bool, idle: bool, now: Instant) -> bool {
        if !hidden {
            *self = Self::default();
            return false;
        }
        if !idle {
            self.idle_since = None;
            self.trimmed = false;
            return false;
        }
        if self.trimmed {
            return false;
        }
        let since = self.idle_since.get_or_insert(now);
        if now.duration_since(*since) < Duration::from_secs(5) {
            return false;
        }
        self.trimmed = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_once_per_idle_period_and_never_during_work() {
        let mut memory = BackgroundMemory::default();
        let now = Instant::now();
        assert!(!memory.should_trim(true, true, now));
        assert!(!memory.should_trim(true, false, now + Duration::from_secs(6)));
        assert!(!memory.should_trim(true, true, now + Duration::from_secs(7)));
        assert!(memory.should_trim(true, true, now + Duration::from_secs(12)));
        assert!(!memory.should_trim(true, true, now + Duration::from_secs(60)));
        assert!(!memory.should_trim(true, false, now + Duration::from_secs(61)));
        assert!(!memory.should_trim(true, true, now + Duration::from_secs(70)));
        assert!(memory.should_trim(true, true, now + Duration::from_secs(75)));
        assert!(!memory.should_trim(true, true, now + Duration::from_secs(80)));
        assert!(!memory.should_trim(false, true, now + Duration::from_secs(81)));
        assert!(!memory.should_trim(true, true, now + Duration::from_secs(82)));
        assert!(memory.should_trim(true, true, now + Duration::from_secs(87)));
    }
}
