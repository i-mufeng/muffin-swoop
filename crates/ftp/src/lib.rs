//! FTP/FTPS transport only: the caller owns files, safe resume decisions and bandwidth accounting.
//! A data-stream EOF is not success. Always call `Download::finish` after draining its chunks.

mod config;
mod transport;

pub use bytes::Bytes;
pub use config::{
    ActiveMode, FtpConfig, FtpCredentials, FtpDataMode, FtpProxy, FtpProxyKind, FtpRequest,
    FtpTlsMode,
};
pub use transport::{Download, FtpClient, Probe};

pub type Result<T> = std::result::Result<T, Error>;

/// FTP SIZE and MDTM are weak metadata, not content identity or a strong ETag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FtpMetadata {
    pub size: Option<u64>,
    pub modified_unix: Option<i64>,
}

/// Errors deliberately exclude raw server replies, URLs and libcurl's error buffer.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid FTP configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid FTP URL")]
    InvalidUrl,
    #[error("FTP URLs must not contain credentials")]
    UrlCredentials,
    #[error("invalid FTP credentials")]
    InvalidCredentials,
    #[error("active FTP cannot be used with a proxy")]
    ActiveProxy,
    #[error("FTP transfer cancelled")]
    Cancelled,
    #[error("FTP data must be fully consumed before checking completion")]
    NotDrained,
    #[error("FTP worker failed")]
    Worker,
    #[error("FTP requires a Tokio runtime")]
    MissingRuntime,
    #[error("libcurl operation failed (code {code}, server status {status})")]
    // libcurl 的 C enum 在 MSVC 上是 i32，在其他目标上可能是 u32。
    Transfer { code: curl_sys::CURLcode, status: u32 },
    #[error("libcurl event loop failed (code {0})")]
    Multi(u32),
    #[error("linked libcurl does not support FTP and FTPS")]
    UnsupportedBuild,
}

impl Error {
    /// 仅重试连接/传输暂态故障；认证、证书、配置与协议错误不自动重试。
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transfer { code, status } => {
                ((400..500).contains(status) && *status != 430)
                    || matches!(
                        *code,
                        curl_sys::CURLE_COULDNT_CONNECT
                            | curl_sys::CURLE_COULDNT_RESOLVE_HOST
                            | curl_sys::CURLE_COULDNT_RESOLVE_PROXY
                            | curl_sys::CURLE_OPERATION_TIMEDOUT
                            | curl_sys::CURLE_PARTIAL_FILE
                            | curl_sys::CURLE_RECV_ERROR
                            | curl_sys::CURLE_SEND_ERROR
                            | curl_sys::CURLE_GOT_NOTHING
                    )
            }
            _ => false,
        }
    }
}
