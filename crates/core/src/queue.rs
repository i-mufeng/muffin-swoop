//! 命名队列与可注入时钟的定时调度；连接数与并发任务数相互独立。

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, TaskId};

pub const MAIN_QUEUE_ID: QueueId = QueueId(1);
pub const MAX_QUEUE_CONCURRENCY: usize = 32;
pub const DEFAULT_QUEUE_CONCURRENCY: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QueueId(pub u64);

/// Unix 秒的起止边界；周期以 start_at/stop_at 为锚点，不补跑错过的历史周期。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSchedule {
    pub start_at: Option<i64>,
    pub stop_at: Option<i64>,
    pub repeat_every_secs: Option<u64>,
}

impl QueueSchedule {
    pub fn validate(&self) -> Result<()> {
        if self.start_at.is_none() && self.stop_at.is_none() {
            return Err(Error::InvalidQueue("至少需要一个开始或停止时间".into()));
        }
        if self.start_at.is_some_and(|t| t < 0) || self.stop_at.is_some_and(|t| t < 0) {
            return Err(Error::InvalidQueue("调度时间不得早于 Unix 纪元".into()));
        }
        if self
            .repeat_every_secs
            .is_some_and(|period| period == 0 || period > i64::MAX as u64)
        {
            return Err(Error::InvalidQueue("重复间隔必须是有效的正秒数".into()));
        }
        if let (Some(start), Some(stop)) = (self.start_at, self.stop_at) {
            if stop <= start {
                return Err(Error::InvalidQueue("停止时间必须晚于开始时间".into()));
            }
            if self
                .repeat_every_secs
                .is_some_and(|period| (stop - start) as u64 >= period)
            {
                return Err(Error::InvalidQueue("单次运行时段必须短于重复间隔".into()));
            }
        }
        Ok(())
    }
}

pub trait QueueClock: Send + Sync {
    fn now(&self) -> SystemTime;
}

pub struct SystemQueueClock;

impl QueueClock for SystemQueueClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Clone, Debug)]
pub struct QueueSnapshot {
    pub id: QueueId,
    pub name: String,
    pub max_concurrent: usize,
    pub running: bool,
    /// 每次启动或周期开始时重新检查已完成任务。
    pub sync_enabled: bool,
    pub task_ids: Vec<TaskId>,
    pub active_count: usize,
    pub waiting_count: usize,
    pub schedule: Option<QueueSchedule>,
    pub next_start_at: Option<i64>,
    pub next_stop_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct QueueRecord {
    pub id: QueueId,
    pub name: String,
    pub max_concurrent: usize,
    pub running: bool,
    /// 每次启动或周期开始时重新检查已完成任务。
    pub sync_enabled: bool,
    pub task_ids: Vec<TaskId>,
    pub schedule: Option<QueueSchedule>,
    pub next_start_at: Option<i64>,
    pub next_stop_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl QueueRecord {
    pub fn main() -> Self {
        Self {
            id: MAIN_QUEUE_ID,
            name: "主队列".into(),
            max_concurrent: DEFAULT_QUEUE_CONCURRENCY,
            running: true,
            sync_enabled: false,
            task_ids: Vec::new(),
            schedule: None,
            next_start_at: None,
            next_stop_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// 返回是否消费了定时边界；同次轮询同时跨越多个边界时，只应用最近的一个。
    pub fn tick(&mut self, now: i64) -> bool {
        let Some(schedule) = &self.schedule else {
            return false;
        };
        let start = consume_due(&mut self.next_start_at, schedule.repeat_every_secs, now);
        let stop = consume_due(&mut self.next_stop_at, schedule.repeat_every_secs, now);
        match (start, stop) {
            (None, None) => return false,
            (Some(_), None) => self.running = true,
            (None, Some(_)) => self.running = false,
            (Some(start), Some(stop)) => self.running = start > stop,
        }
        self.updated_at = now;
        true
    }
}

fn consume_due(next: &mut Option<i64>, period: Option<u64>, now: i64) -> Option<i64> {
    let due = (*next).filter(|due| *due <= now)?;
    let latest = match period {
        Some(period) => {
            let period = period as i64;
            let latest = due + ((now - due) / period) * period;
            *next = latest.checked_add(period);
            latest
        }
        None => {
            *next = None;
            due
        }
    };
    Some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_skips_missed_cycles_and_stop_wins() {
        let mut q = QueueRecord::main();
        q.schedule = Some(QueueSchedule {
            start_at: Some(100),
            stop_at: Some(120),
            repeat_every_secs: Some(60),
        });
        q.next_start_at = Some(100);
        q.next_stop_at = Some(120);
        assert!(q.tick(325));
        assert!(!q.running);
        assert_eq!(q.next_start_at, Some(340));
        assert_eq!(q.next_stop_at, Some(360));
        assert!(q.tick(345));
        assert!(q.running);
        assert!(!q.tick(345));
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn invalid_schedules_cannot_create_zero_period_or_overlapping_cycles() {
        for schedule in [
            QueueSchedule {
                start_at: None,
                stop_at: None,
                repeat_every_secs: None,
            },
            QueueSchedule {
                start_at: Some(-1),
                stop_at: None,
                repeat_every_secs: None,
            },
            QueueSchedule {
                start_at: Some(10),
                stop_at: Some(10),
                repeat_every_secs: None,
            },
            QueueSchedule {
                start_at: Some(10),
                stop_at: Some(30),
                repeat_every_secs: Some(20),
            },
            QueueSchedule {
                start_at: Some(10),
                stop_at: None,
                repeat_every_secs: Some(0),
            },
            QueueSchedule {
                start_at: Some(10),
                stop_at: None,
                repeat_every_secs: Some(u64::MAX),
            },
        ] {
            assert!(schedule.validate().is_err(), "非法调度被接受: {schedule:?}");
        }
        assert!(
            QueueSchedule {
                start_at: Some(10),
                stop_at: Some(30),
                repeat_every_secs: Some(21)
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn one_shot_boundaries_are_consumed_once_and_do_not_undo_manual_changes() {
        let mut q = QueueRecord::main();
        q.running = false;
        q.schedule = Some(QueueSchedule {
            start_at: Some(100),
            stop_at: Some(120),
            repeat_every_secs: None,
        });
        q.next_start_at = Some(100);
        q.next_stop_at = Some(120);
        assert!(!q.tick(99));
        assert!(q.tick(100));
        assert!(q.running);
        q.running = false; // 定时启动后用户手动停止，不应被同一边界重新启动。
        assert!(!q.tick(101));
        assert!(!q.running);
        assert!(q.tick(120));
        assert!(!q.running);
        assert_eq!(q.next_start_at, None);
        assert_eq!(q.next_stop_at, None);
        q.running = true;
        assert!(!q.tick(121));
        assert!(q.running);
    }

    #[test]
    fn start_only_schedule_repeats_without_inventing_stop_boundaries() {
        let mut q = QueueRecord::main();
        q.running = false;
        q.schedule = Some(QueueSchedule {
            start_at: Some(100),
            stop_at: None,
            repeat_every_secs: Some(60),
        });
        q.next_start_at = Some(100);
        assert!(q.tick(161));
        assert!(q.running);
        assert_eq!(q.next_start_at, Some(220));
        assert_eq!(q.next_stop_at, None);
        q.running = false;
        assert!(!q.tick(219));
        assert!(q.tick(220));
        assert!(q.running);
    }
}
