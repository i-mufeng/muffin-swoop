//! swoop-core：Swoop 下载器的引擎层，纯库，不依赖任何 UI。
//!
//! 核心流程：
//! 1. [`probe::probe`] 用 `Range: bytes=0-0` 探测文件大小、是否可续传、文件名。
//! 2. [`segment::SegmentMap`] 按连接数切分区段；某连接提前完成时从最大剩余区段"窃取"一半（动态分段）。
//! 3. [`engine::Engine`] 调度任务，每个任务 N 个 worker 并发写同一个预分配文件（定位写入，无锁）。
//! 4. [`limiter::Limiter`] 全局令牌桶限速。

pub mod auth;
pub mod conflict;
pub mod engine;
pub mod error;
pub mod ftp;
pub mod limiter;
pub mod network;
pub mod probe;
pub mod queue;
pub mod quota;
pub mod segment;
pub mod store;
pub mod sync;
mod target;
pub mod task;
mod util;

pub use auth::{
    AuthCapabilities, AuthSecret, CredentialStorage, CredentialVault, HttpAuthScheme, ProxyAuthRule, ProxyAuthScheme, ProxyCredentials,
    SiteAuthRule, SystemCredentialVault, auth_capabilities,
};
pub use conflict::FileConflictPolicy;
pub use engine::{AddOptions, Config, Engine, Event, MAX_CONNECTIONS, Quiescence, SPEED_HISTORY_LEN};
pub use error::{Error, Result};
pub use ftp::{FtpProxyPolicy, FtpTaskOptions};
pub use network::{NetworkPolicy, ProxyConfig};
pub use probe::ProbeInfo;
pub use queue::{
    DEFAULT_QUEUE_CONCURRENCY, MAIN_QUEUE_ID, MAX_QUEUE_CONCURRENCY, QueueClock, QueueId, QueueSchedule, QueueSnapshot,
    SystemQueueClock,
};
pub use quota::{QuotaPolicy, QuotaSnapshot};
pub use store::{Store, TaskRecord};
pub use swoop_ftp::{ActiveMode, FtpConfig, FtpCredentials, FtpDataMode, FtpProxy, FtpProxyKind, FtpTlsMode};
pub use target::{TargetLease, TargetRegistry};
pub use task::{SegmentSnapshot, TaskId, TaskProperties, TaskSnapshot, TaskStatus};
