//! swoop-core：Swoop 下载器的引擎层，纯库，不依赖任何 UI。
//!
//! 核心流程：
//! 1. [`probe::probe`] 用 `Range: bytes=0-0` 探测文件大小、是否可续传、文件名。
//! 2. [`segment::SegmentMap`] 按连接数切分区段；某连接提前完成时从最大剩余区段"窃取"一半（动态分段）。
//! 3. [`engine::Engine`] 调度任务，每个任务 N 个 worker 并发写同一个预分配文件（定位写入，无锁）。
//! 4. [`limiter::Limiter`] 全局令牌桶限速。

pub mod engine;
pub mod error;
pub mod limiter;
pub mod probe;
pub mod segment;
pub mod task;
mod util;

pub use engine::{AddOptions, Config, Engine, Event};
pub use error::{Error, Result};
pub use task::{SegmentSnapshot, TaskId, TaskSnapshot, TaskStatus};
