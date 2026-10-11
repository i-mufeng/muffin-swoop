//! FTP 任务的非敏感持久化选项。密码始终使用独立的会话凭据。
use serde::{Deserialize, Serialize};
use swoop_ftp::{FtpClient, FtpConfig, FtpProxy, FtpRequest, FtpTlsMode};
use url::Url;

use crate::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FtpProxyPolicy {
    /// 遵循 NetworkPolicy 的手动代理；不支持自动系统代理时明确失败。
    #[default]
    Network,
    /// 用户明确选择 FTP 直连，不隐式忽略已有网络代理。
    Direct,
    Custom(FtpProxy),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpTaskOptions {
    /// FTP 专用超时、主动/被动连接、CA、缓冲配置。proxy 必须留空，代理由 proxy 字段选择。
    pub config: FtpConfig,
    pub tls: FtpTlsMode,
    /// Some 表示必须有匹配的会话凭据；恢复任务时不得退回匿名登录。
    pub username: Option<String>,
    pub proxy: FtpProxyPolicy,
}

impl FtpTaskOptions {
    pub fn for_url(value: &str) -> Result<Self> {
        let _ = FtpRequest::new(value)?;
        let url = Url::parse(value)?;
        Ok(Self {
            tls: if url.scheme() == "ftps" {
                FtpTlsMode::Implicit
            } else {
                FtpTlsMode::Plain
            },
            ..Self::default()
        })
    }

    pub fn validate(&self, url: &str) -> Result<()> {
        FtpRequest::new(url)?.with_tls(self.tls)?;
        if self
            .username
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.contains(['\0', '\r', '\n']))
        {
            return Err(Error::InvalidTaskUpdate("FTP 用户名无效".into()));
        }
        if self.config.proxy.is_some() {
            return Err(Error::InvalidNetwork(
                "FTP 代理须通过 FtpProxyPolicy 选择".into(),
            ));
        }
        let mut config = self.config.clone();
        if let FtpProxyPolicy::Custom(proxy) = &self.proxy {
            config.proxy = Some(proxy.clone());
        }
        FtpClient::new(config)?;
        Ok(())
    }
}
