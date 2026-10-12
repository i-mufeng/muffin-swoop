use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("站点认证错误: {0}")]
    Authentication(String),
    #[error("认证传输失败（libcurl 错误 {0}）")]
    AuthTransport(u32),
    #[error("FTP 传输失败: {0}")]
    Ftp(#[from] swoop_ftp::Error),
    #[error("等待 FTP 认证，请重新输入密码后继续")]
    FtpAuthenticationRequired,
    #[error("FTP 文件大小与探测结果不一致，未接受下载结果")]
    FtpContentChanged,
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
    #[error("链接应是 {0}，服务器却返回了网页（可能需要登录、验证，或链接已失效）")]
    HtmlInsteadOfFile(String),
    #[error("服务器不支持断点续传（Range 请求返回了 200）")]
    RangeNotSupported,
    #[error("服务器提前关闭了连接")]
    ShortRead,
    #[error("下载未完成")]
    Incomplete,
    #[error("任务已取消")]
    Cancelled,
    #[error("同名文件已存在，等待处理")]
    FileConflict,
    #[error("已跳过同名文件")]
    FileSkipped,
    #[error("任务不存在")]
    TaskNotFound,
    #[error("无法修改任务: {0}")]
    InvalidTaskUpdate(String),
    #[error("队列不存在")]
    QueueNotFound,
    #[error("无效的队列设置: {0}")]
    InvalidQueue(String),
    #[error("流量额度错误: {0}")]
    InvalidQuota(String),
    #[error("无效的网络设置: {0}")]
    InvalidNetwork(String),
    #[error("无法安全同步: {0}")]
    UnsafeSync(String),
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("序列化错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("内部错误: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
