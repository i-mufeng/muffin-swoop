use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::segment::SegmentState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Completed,
    Failed(String),
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Probing | Self::Downloading)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SegmentSnapshot {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
    pub state: SegmentState,
}

/// UI 每帧读取的只读快照。
#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub url: String,
    pub file_name: String,
    pub path: PathBuf,
    /// None 表示大小未知
    pub total: Option<u64>,
    pub downloaded: u64,
    pub speed_bps: f64,
    pub status: TaskStatus,
    pub segments: Vec<SegmentSnapshot>,
    pub active_connections: usize,
    pub created_at: SystemTime,
}

impl TaskSnapshot {
    pub fn progress(&self) -> Option<f32> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.downloaded as f64 / t as f64).clamp(0.0, 1.0) as f32)
    }

    /// 预计剩余秒数
    pub fn eta_secs(&self) -> Option<f64> {
        let total = self.total?;
        if self.speed_bps < 1.0 || !self.status.is_active() {
            return None;
        }
        Some(total.saturating_sub(self.downloaded) as f64 / self.speed_bps)
    }
}
