//! 区段表：决定每个连接下载哪一段，并实现 IDM 式的"动态分段"（work stealing）。

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentState {
    Pending,
    Active,
    Done,
}

/// 半开区间 `[start, end)`；`pos` 为该区段内已完成的字节数。
/// `end == u64::MAX` 表示大小未知的流式下载。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
    pub state: SegmentState,
}

impl Segment {
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.start.saturating_add(self.pos))
    }

    pub fn is_done(&self) -> bool {
        self.end != u64::MAX && self.remaining() == 0
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SegmentMap {
    pub segments: Vec<Segment>,
    /// 窃取时产生的新区段至少要有这么大，避免切得过碎
    pub min_split: u64,
}

impl SegmentMap {
    /// 已知大小且可续传：按连接数均分。
    pub fn plan(size: u64, connections: usize, min_segment: u64) -> Self {
        let min_segment = min_segment.max(1);
        let n = if size == 0 {
            1
        } else {
            (size / min_segment).clamp(1, connections.max(1) as u64) as usize
        };
        let base = size / n as u64;
        let rem = size % n as u64;
        let mut segments = Vec::with_capacity(n);
        let mut cur = 0;
        for i in 0..n {
            let len = base + u64::from((i as u64) < rem);
            segments.push(Segment { start: cur, end: cur + len, pos: 0, state: SegmentState::Pending });
            cur += len;
        }
        Self { segments, min_split: min_segment }
    }

    /// 已知大小但不可续传：单连接。
    pub fn single_known(size: u64) -> Self {
        Self {
            segments: vec![Segment { start: 0, end: size, pos: 0, state: SegmentState::Pending }],
            min_split: u64::MAX,
        }
    }

    /// 大小未知：单连接流式。
    pub fn single_unknown() -> Self {
        Self {
            segments: vec![Segment { start: 0, end: u64::MAX, pos: 0, state: SegmentState::Pending }],
            min_split: u64::MAX,
        }
    }

    pub fn total_downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.pos).sum()
    }

    pub fn all_done(&self) -> bool {
        self.segments.iter().all(Segment::is_done)
    }

    pub fn active_count(&self) -> usize {
        self.segments.iter().filter(|s| s.state == SegmentState::Active).count()
    }

    /// 暂停/失败后重新开始前调用：Active 回到 Pending，进度保留。
    pub fn reset_active(&mut self) {
        for s in &mut self.segments {
            if s.state == SegmentState::Active && !s.is_done() {
                s.state = SegmentState::Pending;
            }
        }
    }

    /// 取一段活给 worker：优先 Pending；没有则从剩余最多的 Active 区段切走一半。
    /// 返回区段索引。
    pub fn next_work(&mut self) -> Option<usize> {
        if let Some(i) = self
            .segments
            .iter()
            .position(|s| s.state == SegmentState::Pending && !s.is_done())
        {
            self.segments[i].state = SegmentState::Active;
            return Some(i);
        }

        let (i, victim) = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.state == SegmentState::Active && s.end != u64::MAX)
            .map(|(i, s)| (i, *s))
            .max_by_key(|(_, s)| s.remaining())?;

        let remaining = victim.remaining();
        if remaining < self.min_split.saturating_mul(2) {
            return None;
        }
        let mid = victim.start + victim.pos + remaining / 2;
        self.segments[i].end = mid;
        self.segments.push(Segment { start: mid, end: victim.end, pos: 0, state: SegmentState::Active });
        Some(self.segments.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_splits_evenly_and_covers_everything() {
        let map = SegmentMap::plan(10_001, 4, 1000);
        assert_eq!(map.segments.len(), 4);
        assert_eq!(map.segments[0].start, 0);
        assert_eq!(map.segments.last().unwrap().end, 10_001);
        for w in map.segments.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        let total: u64 = map.segments.iter().map(|s| s.end - s.start).sum();
        assert_eq!(total, 10_001);
    }

    #[test]
    fn plan_respects_min_segment() {
        let map = SegmentMap::plan(3000, 8, 1000);
        assert_eq!(map.segments.len(), 3);
        let map = SegmentMap::plan(500, 8, 1000);
        assert_eq!(map.segments.len(), 1);
    }

    #[test]
    fn next_work_steals_from_largest_active() {
        let mut map = SegmentMap::plan(8000, 2, 100);
        let a = map.next_work().unwrap();
        let b = map.next_work().unwrap();
        assert_eq!((a, b), (0, 1));
        // 模拟区段 0 已下了 3000，区段 1 一点没动 → 从区段 1 偷
        map.segments[0].pos = 3000;
        let c = map.next_work().unwrap();
        assert_eq!(c, 2);
        assert_eq!(map.segments[1].end, 6000);
        assert_eq!(map.segments[2].start, 6000);
        assert_eq!(map.segments[2].end, 8000);
        let total: u64 = map.segments.iter().map(|s| s.end - s.start).sum();
        assert_eq!(total, 8000);
    }

    #[test]
    fn next_work_stops_when_too_small() {
        let mut map = SegmentMap::plan(1000, 1, 600);
        map.next_work().unwrap();
        assert!(map.next_work().is_none());
    }
}
