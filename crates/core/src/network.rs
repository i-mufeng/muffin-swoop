//! 可持久化的网络策略；每次运行冻结客户端、超时、代理和重试参数。

use std::fmt;
use std::time::Duration;

use reqwest::header::{self, HeaderMap};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{Error, Result};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// http/https/socks5/socks5h 地址。当前版本不允许 URL 中包含账号密码。
    pub url: String,
    /// reqwest NO_PROXY 语法：主机、域名后缀、IP/CIDR 或 *；不填写协议和路径。
    pub bypass: Vec<String>,
}

impl ProxyConfig {
    pub(crate) fn bypasses(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']);
        self.bypass.iter().any(|entry| {
            let entry = entry.trim();
            if entry == "*" {
                return true;
            }
            if let Some((network, bits)) = entry.split_once('/') {
                let Ok(bits) = bits.parse::<u32>() else {
                    return false;
                };
                return match (host.parse::<std::net::IpAddr>(), network.parse::<std::net::IpAddr>()) {
                    (Ok(std::net::IpAddr::V4(ip)), Ok(std::net::IpAddr::V4(network))) if bits <= 32 => {
                        u32::from(ip).checked_shr(32 - bits).unwrap_or(0)
                            == u32::from(network).checked_shr(32 - bits).unwrap_or(0)
                    }
                    (Ok(std::net::IpAddr::V6(ip)), Ok(std::net::IpAddr::V6(network))) if bits <= 128 => {
                        u128::from(ip).checked_shr(128 - bits).unwrap_or(0)
                            == u128::from(network).checked_shr(128 - bits).unwrap_or(0)
                    }
                    _ => false,
                };
            }
            let entry = entry.trim_start_matches('.').trim_matches(['[', ']']);
            host.eq_ignore_ascii_case(entry)
                || host.to_ascii_lowercase().ends_with(&format!(".{}", entry.to_ascii_lowercase()))
        })
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let address = Url::parse(&self.url)
            .map(|mut url| {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.to_string()
            })
            .unwrap_or_else(|_| "<无效代理地址>".into());
        f.debug_struct("ProxyConfig").field("url", &address).field("bypass", &self.bypass).finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkPolicy {
    pub connect_timeout_secs: u64,
    pub read_timeout_secs: u64,
    /// 单个 HTTP 请求（包含响应体）的总时间；None 不限制下载总时长。
    pub request_timeout_secs: Option<u64>,
    /// 首次请求之外的最大重试次数；探测和各下载区段分别计数。
    pub retries: u32,
    pub retry_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub proxy: Option<ProxyConfig>,
    /// Non-secret proxy identity. Passwords live only in the runtime or system vault.
    pub proxy_auth: Option<crate::ProxyAuthRule>,
    /// 未配置手动代理时是否遵循系统/环境代理。
    pub use_system_proxy: bool,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 15,
            read_timeout_secs: 30,
            request_timeout_secs: None,
            retries: 5,
            retry_delay_ms: 600,
            retry_max_delay_ms: 9600,
            proxy: None,
            proxy_auth: None,
            use_system_proxy: true,
        }
    }
}

impl NetworkPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=86400).contains(&self.connect_timeout_secs)
            || !(1..=86400).contains(&self.read_timeout_secs)
            || self.request_timeout_secs.is_some_and(|seconds| !(1..=86400).contains(&seconds))
        {
            return Err(Error::InvalidNetwork("超时需要介于 1 秒和 24 小时之间".into()));
        }
        if self.retries > 100
            || self.retry_delay_ms == 0
            || self.retry_max_delay_ms > 3_600_000
            || self.retry_max_delay_ms < self.retry_delay_ms
        {
            return Err(Error::InvalidNetwork(
                "重试次数不得超过 100，退避时间需要介于 1 毫秒和 1 小时之间，且上限不得小于初始值".into(),
            ));
        }
        if let Some(proxy) = &self.proxy {
            let url = Url::parse(proxy.url.trim()).map_err(|_| Error::InvalidNetwork("代理地址无效".into()))?;
            if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") || url.host_str().is_none() {
                return Err(Error::InvalidNetwork("代理需使用 http、https、socks5 或 socks5h 地址".into()));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(Error::InvalidNetwork("代理地址不能包含账号密码；请使用独立代理认证设置".into()));
            }
            if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
                return Err(Error::InvalidNetwork("代理地址不能包含路径、查询参数或片段".into()));
            }
            if proxy
                .bypass
                .iter()
                .any(|host| host.trim().is_empty() || host.contains("://") || host.contains([',', '\r', '\n', '@']))
            {
                return Err(Error::InvalidNetwork("代理排除列表需要填写主机、域名后缀或 IP/CIDR".into()));
            }
        }
        if let Some(rule) = &self.proxy_auth {
            rule.validate()?;
            let proxy = self.proxy.as_ref().ok_or_else(|| Error::Authentication("代理认证需要手动代理地址".into()))?;
            if crate::auth::proxy::canonical_endpoint(&proxy.url)? != rule.endpoint {
                return Err(Error::Authentication("代理地址已改变，请重新配置对应代理凭据".into()));
            }
        }
        Ok(())
    }

    pub(crate) fn retry_delay(&self, retry: u32) -> Duration {
        Duration::from_millis(
            self.retry_delay_ms
                .saturating_mul(2u64.saturating_pow(retry.saturating_sub(1)))
                .min(self.retry_max_delay_ms),
        )
    }
}

#[derive(Clone)]
pub(crate) struct NetworkRuntime {
    pub policy: NetworkPolicy,
    pub client: Client,
    pub auth: crate::auth::AuthRuntime,
    user_agent: String,
}

impl NetworkRuntime {
    pub(crate) fn inherit_auth(&mut self, previous: &Self) {
        self.auth = previous.auth.clone();
        self.auth.inherit_proxy(self.policy.proxy_auth.as_ref(), previous.policy.proxy_auth.as_ref());
    }
    pub fn new(policy: NetworkPolicy, user_agent: &str) -> Result<Self> {
        policy.validate()?;
        let mut builder = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(policy.connect_timeout_secs))
            .read_timeout(Duration::from_secs(policy.read_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .referer(false);
        if let Some(seconds) = policy.request_timeout_secs {
            builder = builder.timeout(Duration::from_secs(seconds));
        }
        if let Some(config) = &policy.proxy {
            let proxy = reqwest::Proxy::all(config.url.trim())?
                .no_proxy(reqwest::NoProxy::from_string(&config.bypass.join(",")));
            builder = builder.no_proxy().proxy(proxy);
        } else if !policy.use_system_proxy {
            builder = builder.no_proxy();
        }
        Ok(Self {
            policy,
            client: builder.build()?,
            auth: crate::auth::AuthRuntime::default(),
            user_agent: user_agent.to_owned(),
        })
    }
}

pub(crate) fn retryable(error: &Error) -> bool {
    match error {
        Error::AuthTransport(code) => matches!(code, 5 | 6 | 7 | 18 | 28 | 52 | 55 | 56),
        Error::Ftp(error) => error.is_retryable(),
        Error::Request(error) => {
            error.is_timeout() || error.is_connect() || error.is_body() || error.is_decode() || error.is_request()
        }
        Error::Http(status) => matches!(status, 408 | 429 | 500..=599),
        Error::ShortRead | Error::Incomplete => true,
        _ => false,
    }
}

pub(crate) async fn wait_retry(policy: &NetworkPolicy, retry: u32, cancel: &CancellationToken) -> Result<()> {
    tokio::select! {
        _ = tokio::time::sleep(policy.retry_delay(retry)) => Ok(()),
        _ = cancel.cancelled() => Err(Error::Cancelled),
    }
}

/// Engine 的客户端必须禁用自动重定向。跨源后只保留公开的协商头和 Range，且后续跳回源站也不恢复密钥。
pub(crate) async fn get_following_redirects(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
) -> Result<reqwest::Response> {
    let mut current = url.clone();
    let mut headers = headers.clone();
    for hop in 0..=10 {
        let response = client.get(current.clone()).headers(headers.clone()).send().await?;
        if !matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            return Ok(response);
        }
        let Some(location) = response.headers().get(header::LOCATION) else {
            return Ok(response);
        };
        if hop == 10 {
            return Err(Error::InvalidNetwork("重定向次数超过 10 次".into()));
        }
        let next = current.join(location.to_str().map_err(|_| Error::InvalidNetwork("重定向地址无效".into()))?)?;
        if !matches!(next.scheme(), "http" | "https") {
            return Err(Error::UnsupportedScheme(next.scheme().into()));
        }
        if current.origin() != next.origin() {
            let allowed =
                [header::RANGE, header::USER_AGENT, header::ACCEPT, header::ACCEPT_ENCODING, header::ACCEPT_LANGUAGE];
            let mut safe = HeaderMap::new();
            for key in allowed {
                for value in headers.get_all(&key) {
                    safe.append(key.clone(), value.clone());
                }
            }
            headers = safe;
        }
        current = next;
    }
    unreachable!("重定向循环有明确上限")
}

/// Shared response shape preserves the existing Range / validator / streaming checks.
pub(crate) enum NetworkResponse {
    Http(reqwest::Response),
    Auth(crate::auth::transport::CurlResponse),
}
impl NetworkResponse {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Http(r) => r.status(),
            Self::Auth(r) => r.status(),
        }
    }
    pub fn headers(&self) -> &HeaderMap {
        match self {
            Self::Http(r) => r.headers(),
            Self::Auth(r) => r.headers(),
        }
    }
    pub fn url(&self) -> &Url {
        match self {
            Self::Http(r) => r.url(),
            Self::Auth(r) => r.url(),
        }
    }
    pub fn content_length(&self) -> Option<u64> {
        self.headers().get(header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
    }
    pub async fn chunk(&mut self) -> Result<Option<bytes::Bytes>> {
        match self {
            Self::Http(r) => Ok(r.chunk().await?),
            Self::Auth(r) => r.chunk().await,
        }
    }
}
impl NetworkRuntime {
    pub async fn get(&self, url: &Url, headers: &HeaderMap) -> Result<NetworkResponse> {
        let rule = self.auth.rule(url).cloned();
        if rule.is_none() && self.policy.proxy_auth.is_none() {
            return get_following_redirects(&self.client, url, headers).await.map(NetworkResponse::Http);
        }
        if rule.is_some() && headers.contains_key(header::AUTHORIZATION) {
            return Err(Error::Authentication("站点认证不能与自定义 Authorization 请求头同时使用".into()));
        }
        if headers.contains_key(header::PROXY_AUTHORIZATION) {
            return Err(Error::Authentication("请使用代理认证配置，不能通过请求头传递代理密码".into()));
        }
        let site = match rule {
            Some(rule) => Some((rule.clone(), self.auth.credentials(&rule).await?)),
            None => None,
        };
        let mut current = url.clone();
        let mut headers = headers.clone();
        let mut auth_allowed = true;
        for hop in 0..=10 {
            let bypass = self.policy.proxy.as_ref().is_some_and(|proxy| proxy.bypasses(&current));
            let proxy_rule = self.policy.proxy_auth.as_ref().filter(|_| !bypass);
            let proxy_secret = match proxy_rule {
                Some(rule) => self.auth.proxy_credentials(rule).await?,
                None => None,
            };
            let mut policy = self.policy.clone();
            if bypass {
                policy.proxy = None;
                policy.proxy_auth = None;
                policy.use_system_proxy = false;
            }
            let response = if (auth_allowed && site.is_some()) || self.policy.proxy_auth.is_some() {
                NetworkResponse::Auth(
                    crate::auth::transport::get(
                        policy,
                        self.user_agent.clone(),
                        current.clone(),
                        headers.clone(),
                        if auth_allowed { site.clone() } else { None },
                        proxy_secret.clone(),
                    )
                    .await?,
                )
            } else {
                NetworkResponse::Http(self.client.get(current.clone()).headers(headers.clone()).send().await?)
            };
            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let Some(location) = response.headers().get(header::LOCATION) else {
                return Ok(response);
            };
            if hop == 10 {
                return Err(Error::InvalidNetwork("重定向次数超过 10 次".into()));
            }
            let next = current.join(location.to_str().map_err(|_| Error::InvalidNetwork("重定向地址无效".into()))?)?;
            if !matches!(next.scheme(), "http" | "https") || !next.username().is_empty() || next.password().is_some() {
                return Err(Error::Authentication("认证请求拒绝含凭据或非 HTTP 的重定向".into()));
            }
            if current.scheme() == "https" && next.scheme() == "http" {
                return Err(Error::Authentication("认证请求拒绝 HTTPS 降级重定向".into()));
            }
            if current.origin() != next.origin() {
                auth_allowed = false;
                let mut safe = HeaderMap::new();
                for key in [
                    header::RANGE,
                    header::USER_AGENT,
                    header::ACCEPT,
                    header::ACCEPT_ENCODING,
                    header::ACCEPT_LANGUAGE,
                ] {
                    for value in headers.get_all(&key) {
                        safe.append(key.clone(), value.clone());
                    }
                }
                headers = safe;
            }
            current = next;
        }
        unreachable!("bounded redirect loop")
    }
}
