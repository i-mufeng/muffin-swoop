use std::{fmt, net::IpAddr, path::PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FtpTlsMode {
    #[default]
    Plain,
    Explicit,
    Implicit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActiveMode {
    /// None uses the address of the control connection. Hostnames are intentionally unsupported.
    pub address: Option<IpAddr>,
    /// Inclusive local listening port range; None lets the OS allocate a port.
    pub port_range: Option<(u16, u16)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FtpDataMode {
    /// EPSV first, with libcurl's PASV fallback. Passive data uses the control peer's IP.
    #[default]
    Passive,
    Active(ActiveMode),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FtpProxyKind {
    #[default]
    HttpTunnel,
    HttpsTunnel,
    Socks5,
    Socks5Hostname,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpProxy {
    pub kind: FtpProxyKind,
    pub host: String,
    pub port: u16,
    pub bypass_hosts: Vec<String>,
}

impl Default for FtpProxy {
    fn default() -> Self {
        Self {
            kind: FtpProxyKind::HttpTunnel,
            host: "127.0.0.1".into(),
            port: 8080,
            bypass_hosts: Vec::new(),
        }
    }
}

/// Non-secret, serializable settings. This library never saves configuration or credentials.
/// System proxy environment variables are disabled: proxy selection is explicit and reproducible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpConfig {
    pub data_mode: FtpDataMode,
    pub proxy: Option<FtpProxy>,
    pub connect_timeout_secs: u64,
    /// Cancels stalled network transfers; time spent paused by backpressure is excluded.
    pub read_timeout_secs: u64,
    pub transfer_timeout_secs: Option<u64>,
    /// Private trust bundle for origin and HTTPS proxy. Hostname and peer checks remain enabled.
    pub ca_certificate: Option<PathBuf>,
    /// Number of buffered callback chunks, each at most 16 KiB, in addition to libcurl's buffer.
    pub buffer_chunks: usize,
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            data_mode: FtpDataMode::Passive,
            proxy: None,
            connect_timeout_secs: 15,
            read_timeout_secs: 30,
            transfer_timeout_secs: None,
            ca_certificate: None,
            buffer_chunks: 4,
        }
    }
}

impl FtpConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.connect_timeout_secs == 0
            || self.connect_timeout_secs > 86_400
            || self.read_timeout_secs == 0
            || self.read_timeout_secs > 86_400
            || self
                .transfer_timeout_secs
                .is_some_and(|n| n == 0 || n > 31_536_000)
        {
            return Err(Error::InvalidConfig("timeouts are out of range"));
        }
        if !(1..=64).contains(&self.buffer_chunks) {
            return Err(Error::InvalidConfig(
                "buffer_chunks must be between 1 and 64",
            ));
        }
        if let FtpDataMode::Active(active) = &self.data_mode {
            if self.proxy.is_some() {
                return Err(Error::ActiveProxy);
            }
            if active
                .port_range
                .is_some_and(|(first, last)| first == 0 || first > last)
            {
                return Err(Error::InvalidConfig("invalid active port range"));
            }
        }
        if let Some(proxy) = &self.proxy
            && (proxy.port == 0
                || proxy.host.is_empty()
                || proxy.host.chars().any(|c| {
                    c.is_control() || c.is_whitespace() || matches!(c, '/' | '@' | '?' | '#')
                })
                || (proxy.host.parse::<IpAddr>().is_err()
                    && url::Host::parse(&proxy.host).is_err())
                || proxy.bypass_hosts.iter().any(|host| {
                    host.is_empty()
                        || host
                            .chars()
                            .any(|c| c.is_control() || c.is_whitespace() || c == ',')
                }))
        {
            return Err(Error::InvalidConfig("invalid proxy address or exclusion"));
        }
        Ok(())
    }
}

/// Credentials are independent of URLs and have no serialization implementation.
#[derive(Clone)]
pub struct FtpCredentials {
    pub(crate) username: String,
    pub(crate) password: String,
}

impl FtpCredentials {
    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Result<Self> {
        let username = username.into();
        let password = password.into();
        if username.is_empty()
            || [&username, &password]
                .into_iter()
                .any(|value| value.contains(['\0', '\r', '\n']))
        {
            return Err(Error::InvalidCredentials);
        }
        Ok(Self { username, password })
    }
}

impl fmt::Debug for FtpCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FtpCredentials([redacted])")
    }
}

#[derive(Clone)]
pub struct FtpRequest {
    pub(crate) url: Url,
    pub(crate) tls: FtpTlsMode,
    pub(crate) credentials: Option<FtpCredentials>,
}

impl FtpRequest {
    pub fn new(value: &str) -> Result<Self> {
        let url = Url::parse(value).map_err(|_| Error::InvalidUrl)?;
        if value.split_once("://").is_some_and(|(_, rest)| {
            rest.split('/')
                .next()
                .is_some_and(|authority| authority.contains('@'))
        }) {
            return Err(Error::UrlCredentials);
        }
        if !matches!(url.scheme(), "ftp" | "ftps")
            || url.host_str().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path().is_empty()
            || url.path().ends_with('/')
        {
            return Err(Error::InvalidUrl);
        }
        // libcurl interprets an unescaped suffix as an ASCII/directory transfer directive.
        // Downloaded bytes must remain binary. Literal semicolons can be percent-encoded.
        if url.path().to_ascii_lowercase().contains(";type=") {
            return Err(Error::InvalidUrl);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::UrlCredentials);
        }
        let path = percent_encoding::percent_decode_str(url.path())
            .decode_utf8()
            .map_err(|_| Error::InvalidUrl)?;
        if path.chars().any(char::is_control) {
            return Err(Error::InvalidUrl);
        }
        let tls = if url.scheme() == "ftps" {
            FtpTlsMode::Implicit
        } else {
            FtpTlsMode::Plain
        };
        Ok(Self {
            url,
            tls,
            credentials: None,
        })
    }

    pub fn with_credentials(mut self, credentials: FtpCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    pub fn with_tls(mut self, tls: FtpTlsMode) -> Result<Self> {
        if self.url.scheme() == "ftps" && tls != FtpTlsMode::Implicit {
            return Err(Error::InvalidConfig("ftps URLs require implicit TLS"));
        }
        self.tls = tls;
        Ok(self)
    }
}

impl fmt::Debug for FtpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The path itself can contain access tokens; omit the entire URL.
        f.debug_struct("FtpRequest")
            .field("tls", &self.tls)
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_credential_urls_and_command_injection() {
        for url in [
            "ftp://alice:secret@localhost/file",
            "ftp://alice@localhost/file",
            "ftp://@localhost/file",
        ] {
            assert_eq!(FtpRequest::new(url).unwrap_err(), Error::UrlCredentials);
        }
        for url in [
            "http://localhost/file",
            "ftp://localhost/a%0d%0aDELE%20x",
            "ftp://localhost/a%00",
            "ftp://localhost/dir/",
            "ftp://localhost/file;type=a",
            "ftp://localhost/dir;type=d",
        ] {
            assert_eq!(FtpRequest::new(url).unwrap_err(), Error::InvalidUrl);
        }
        assert!(FtpCredentials::new("user", "a\r\nDELE x").is_err());
    }

    #[test]
    fn secrets_do_not_appear_in_debug_output() {
        let credentials = FtpCredentials::new("secret-user", "secret-password").unwrap();
        let request = FtpRequest::new("ftp://localhost/secret-path")
            .unwrap()
            .with_credentials(credentials.clone());
        let text = format!("{credentials:?} {request:?}");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn settings_default_and_round_trip_without_credentials() {
        let mut config: FtpConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, FtpConfig::default());
        config.data_mode = FtpDataMode::Active(ActiveMode {
            address: Some("127.0.0.1".parse().unwrap()),
            port_range: Some((41000, 41010)),
        });
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<FtpConfig>(&json).unwrap(), config);
        config.proxy = Some(FtpProxy::default());
        assert_eq!(config.validate().unwrap_err(), Error::ActiveProxy);
    }
}
