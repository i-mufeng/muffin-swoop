//! 演示数据：`SWOOP_DEMO=1` 时在列表里混入几条假任务，用于调界面和截图。
//! 只在 UI 层拼 `TaskSnapshot`，不经过引擎；对假任务的操作会被引擎以 TaskNotFound 忽略。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use swoop_core::segment::SegmentState;
use swoop_core::{SPEED_HISTORY_LEN, SegmentSnapshot, TaskId, TaskSnapshot, TaskStatus};

pub fn enabled() -> bool {
    std::env::var_os("SWOOP_DEMO").is_some()
}

/// 演示模式启动时默认选中的任务（打开详情面板）
pub const SELECTED: TaskId = TaskId(9001);

/// 假任务的速度曲线：围绕当前速度起伏，下载中的按秒向左滚动；非演示任务返回 None
pub fn speed_history(t: &TaskSnapshot) -> Option<Vec<f64>> {
    if !enabled() || !(9001..=9008).contains(&t.id.0) {
        return None;
    }
    let (len, base) = match t.id.0 {
        9001 => (SPEED_HISTORY_LEN, t.speed_bps),
        // 刚开始不久，曲线只铺了一段
        9002 => (48, t.speed_bps),
        // 已暂停：保留暂停前那段
        9003 => (75, 9.0 * MB as f64),
        9005 => (SPEED_HISTORY_LEN, t.speed_bps),
        _ => (0, 0.0),
    };
    let rolling = t.status == TaskStatus::Downloading;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as f64;
    let seed = t.id.0 as f64;
    Some(
        (0..len)
            .map(|i| {
                let k = if rolling { now - (len - 1 - i) as f64 } else { i as f64 };
                let wave = 0.82 + 0.12 * (k * 0.19 + seed).sin() + 0.06 * (k * 0.71 + seed * 2.0).sin();
                let ramp = if len < SPEED_HISTORY_LEN { ((i + 1) as f64 / 5.0).min(1.0) } else { 1.0 };
                base * wave * ramp
            })
            .collect(),
    )
}

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

pub fn tasks(dir: &Path) -> Vec<TaskSnapshot> {
    vec![
        task(
            9001,
            "ubuntu-24.04.3-desktop-amd64.iso",
            "https://releases.ubuntu.com/24.04/ubuntu-24.04.3-desktop-amd64.iso",
            dir,
            Some(6 * GB + 120 * MB),
            // 8 段，进度不一，体现动态分段
            &[(0.0, 0.125, 1.0), (0.125, 0.25, 0.92), (0.25, 0.375, 0.55), (0.375, 0.5, 0.71), (0.5, 0.625, 0.18), (0.625, 0.75, 0.33), (0.75, 0.875, 0.06), (0.875, 1.0, 0.0)],
            38.4 * MB as f64,
            TaskStatus::Downloading,
        ),
        task(
            9002,
            "Xcode_16.4.xip",
            "https://download.developer.apple.com/Developer_Tools/Xcode_16.4/Xcode_16.4.xip",
            dir,
            Some(3 * GB + 210 * MB),
            &[(0.0, 0.25, 0.12), (0.25, 0.5, 0.08), (0.5, 0.75, 0.05), (0.75, 1.0, 0.03)],
            12.1 * MB as f64,
            TaskStatus::Downloading,
        ),
        task(
            9003,
            "设计规范_v3_终稿.pdf",
            "https://cdn.example.com/docs/%E8%AE%BE%E8%AE%A1%E8%A7%84%E8%8C%83_v3.pdf",
            dir,
            Some(24 * MB),
            &[(0.0, 0.5, 1.0), (0.5, 1.0, 0.2)],
            0.0,
            TaskStatus::Paused,
        ),
        task(
            9004,
            "node-v22.12.0.pkg",
            "https://nodejs.org/dist/v22.12.0/node-v22.12.0.pkg",
            dir,
            Some(78 * MB),
            &[],
            0.0,
            TaskStatus::Queued,
        ),
        task(
            9005,
            "lecture_01_1080p.mp4",
            "https://media.example.edu/stream/lecture_01_1080p.mp4",
            dir,
            None,
            &[(0.0, 1.0, 0.0)],
            4.6 * MB as f64,
            TaskStatus::Downloading,
        ),
        task(
            9006,
            "rustup-init",
            "https://static.rust-lang.org/rustup/dist/aarch64-apple-darwin/rustup-init",
            dir,
            Some(11 * MB + 300 * 1024),
            &[(0.0, 1.0, 1.0)],
            0.0,
            TaskStatus::Completed,
        ),
        task(
            9007,
            "FiraCode-6.2.zip",
            "https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip",
            dir,
            Some(29 * MB),
            &[(0.0, 1.0, 1.0)],
            0.0,
            TaskStatus::Completed,
        ),
        task(
            9008,
            "backup-2026-10.tar.gz",
            "https://files.example.com/private/backup-2026-10.tar.gz",
            dir,
            Some(GB + 400 * MB),
            &[(0.0, 0.5, 0.3), (0.5, 1.0, 0.0)],
            0.0,
            TaskStatus::Failed("HTTP 状态码 403".into()),
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn task(
    id: u64,
    name: &str,
    url: &str,
    dir: &Path,
    total: Option<u64>,
    // (起点比例, 终点比例, 该段完成比例)
    segs: &[(f64, f64, f64)],
    speed: f64,
    status: TaskStatus,
) -> TaskSnapshot {
    let size = total.unwrap_or(0);
    let mut segments = Vec::new();
    let mut downloaded = 0u64;
    if let Some(size) = total {
        for (a, b, done) in segs {
            let start = (size as f64 * a) as u64;
            let end = (size as f64 * b) as u64;
            let pos = ((end - start) as f64 * done) as u64;
            downloaded += pos;
            let state = if pos >= end - start {
                SegmentState::Done
            } else if *done > 0.0 && status == TaskStatus::Downloading {
                SegmentState::Active
            } else {
                SegmentState::Pending
            };
            segments.push(SegmentSnapshot { start, end, pos, state });
        }
    } else {
        downloaded = 150 * MB;
        segments.push(SegmentSnapshot { start: 0, end: u64::MAX, pos: downloaded, state: SegmentState::Active });
    }
    let _ = size;
    let active_connections = segments.iter().filter(|s| s.state == SegmentState::Active).count();
    let completed_at = (status == TaskStatus::Completed).then(SystemTime::now);
    TaskSnapshot {
        id: TaskId(id),
        url: url.to_string(),
        file_name: name.to_string(),
        path: dir.join(name),
        total,
        downloaded,
        speed_bps: speed,
        status,
        resumable: total.is_some(),
        segments,
        active_connections,
        created_at: SystemTime::now(),
        completed_at,
    }
}
