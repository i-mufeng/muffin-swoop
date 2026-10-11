use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP 状态码 {0}")]
    Http(u16),
    #[error("请求失败: {0}")]
    Request(#[from] reqwest::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("无效的 URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("无效的请求头: {0}")]
    InvalidHeader(String),
    #[error("暂不支持的协议: {0}")]
    UnsupportedScheme(String),
    #[error("服务器不支持断点续传（Range 请求返回了 200）")]
    RangeNotSupported,
    #[error("服务器提前关闭了连接")]
    ShortRead,
    #[error("下载未完成")]
    Incomplete,
    #[error("任务已取消")]
    Cancelled,
    #[error("任务不存在")]
    TaskNotFound,
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("序列化错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("内部错误: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
