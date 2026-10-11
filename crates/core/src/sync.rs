//! 周期同步的持久化归属凭据。旧任务没有凭据时，不能自动认领磁盘上的文件。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub baseline: Option<SyncBaseline>,
    pub pending: bool,
    /// 用户明确选择覆盖的初次下载；借用同步的暂存发布与恢复机制。
    pub overwrite: bool,
    /// 原子替换前已落盘的候选凭据；恢复时仅接受旧/候选两种确知内容。
    pub prepared: Option<SyncBaseline>,
    /// 发布时临时移开的原文件；在移动前落盘，断电后可无覆盖恢复。
    pub recovery_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncBaseline {
    pub canonical_path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub url: String,
    #[serde(default)]
    pub request_sha256: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub completed_at: i64,
}

pub(crate) fn strong_etag(etag: &str) -> bool {
    etag.starts_with('"') && etag.ends_with('"') && etag.len() >= 2
}
