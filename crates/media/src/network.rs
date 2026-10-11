//! One upstream transport for manifests, media bytes and encryption keys.
use crate::{MediaError, MediaSessionAuth, network_url};
use hyper::body::Bytes;
use reqwest::{
    Client, StatusCode,
    header::{self, HeaderMap},
};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc, time::Duration};
pub use swoop_core::auth::ProxyCredentials as MediaProxyCredentials;
use swoop_core::auth::{HttpResponse, ProxyHttpClient};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaProxyConfig {
    pub url: String,
    #[serde(default)]
    pub bypass: Vec<String>,
}
impl fmt::Debug for MediaProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaProxyConfig(<redacted>)")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MediaNetworkConfig {
    pub proxy: Option<MediaProxyConfig>,
    pub proxy_auth: Option<swoop_core::ProxyAuthRule>,
    /// Combined body throughput across this downloader and all its clones. Zero is unlimited.
    pub speed_limit_bps: u64,
    pub resume_partial_segments: bool,
    pub partial_flush_bytes: u64,
    pub use_system_proxy: bool,
    pub connect_timeout_secs: u64,
    pub read_timeout_secs: u64,
    pub max_redirects: usize,
    pub max_resources: usize,
    pub max_concurrent_requests: usize,
    /// Retries before a response body is exposed; partial media is never silently skipped.
    pub request_retries: usize,
    pub retry_delay_ms: u64,
    /// Minimum interval between refreshes of the same live manifest.
    pub live_manifest_refresh_ms: u64,
    pub resume_cache_dir: Option<std::path::PathBuf>,
    pub cache_max_mib: usize,
    pub cache_resource_mib: usize,
}
impl Default for MediaNetworkConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            proxy_auth: None,
            speed_limit_bps: 0,
            resume_partial_segments: true,
            partial_flush_bytes: 1024 * 1024,
            use_system_proxy: false,
            connect_timeout_secs: 15,
            read_timeout_secs: 30,
            max_redirects: 8,
            max_resources: 20_000,
            max_concurrent_requests: 16,
            request_retries: 2,
            retry_delay_ms: 250,
            live_manifest_refresh_ms: 250,
            resume_cache_dir: None,
            cache_max_mib: 512,
            cache_resource_mib: 16,
        }
    }
}
impl MediaNetworkConfig {
    pub fn validate(&self) -> Result<(), MediaError> {
        if !(1..=86400).contains(&self.connect_timeout_secs)
            || !(1..=86400).contains(&self.read_timeout_secs)
            || self.request_retries > 10
            || self.retry_delay_ms > 30_000
            || !(50..=30_000).contains(&self.live_manifest_refresh_ms)
            || !(16..=8192).contains(&self.cache_max_mib)
            || !(1..=64).contains(&self.cache_resource_mib)
            || self.cache_resource_mib > self.cache_max_mib
            || self.resume_cache_dir.as_ref().is_some_and(|path| path.as_os_str().is_empty())
            || !(16 * 1024..=16 * 1024 * 1024).contains(&self.partial_flush_bytes)
            || self.max_redirects > 16
            || !(1..=100_000).contains(&self.max_resources)
            || !(1..=64).contains(&self.max_concurrent_requests)
        {
            return Err(MediaError::InvalidNetwork);
        }
        if let Some(proxy) = &self.proxy {
            let url = Url::parse(proxy.url.trim()).map_err(|_| MediaError::InvalidNetwork)?;
            if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || !matches!(url.path(), "" | "/")
                || url.query().is_some()
                || url.fragment().is_some()
                || proxy.bypass.len() > 128
                || proxy.bypass.iter().any(|value| {
                    value.is_empty()
                        || value.len() > 256
                        || value.contains("://")
                        || value.contains([',', '\r', '\n', '\0', '@'])
                })
            {
                return Err(MediaError::InvalidNetwork);
            }
        }
        self.core_policy().validate().map_err(|_| MediaError::InvalidNetwork)?;
        Ok(())
    }
    fn core_policy(&self) -> swoop_core::NetworkPolicy {
        swoop_core::NetworkPolicy {
            proxy: self
                .proxy
                .as_ref()
                .map(|proxy| swoop_core::ProxyConfig { url: proxy.url.clone(), bypass: proxy.bypass.clone() }),
            proxy_auth: self.proxy_auth.clone(),
            use_system_proxy: self.use_system_proxy,
            connect_timeout_secs: self.connect_timeout_secs,
            read_timeout_secs: self.read_timeout_secs,
            request_timeout_secs: None,
            retries: 0,
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub(crate) struct NetworkTransport {
    client: Client,
    upstream: ProxyHttpClient,
    global_limit: Arc<crate::limiter::Limiter>,
    task_limit: Arc<crate::limiter::Limiter>,
    header_timeout: Duration,
    read_timeout: Duration,
    pub resume_partial_segments: bool,
    pub partial_flush_bytes: u64,
    pub has_proxy: bool,
    pub cache: Option<std::sync::Arc<crate::cache::Cache>>,
    max_redirects: usize,
    pub max_resources: usize,
    pub max_concurrent_requests: usize,
    pub live_manifest_refresh: Duration,
    request_retries: usize,
    retry_delay_ms: u64,
}
impl NetworkTransport {
    #[cfg(test)]
    pub fn new(config: &MediaNetworkConfig, timeout: Duration) -> Result<Self, MediaError> {
        Self::new_with_proxy(config, timeout, None)
    }
    pub fn new_with_proxy(
        config: &MediaNetworkConfig,
        timeout: Duration,
        credentials: Option<MediaProxyCredentials>,
    ) -> Result<Self, MediaError> {
        config.validate()?;
        let upstream = ProxyHttpClient::new(config.core_policy(), "Swoop Media", credentials).map_err(media_error)?;
        Ok(Self {
            client: Client::builder().no_proxy().build().map_err(|_| MediaError::InvalidNetwork)?,
            upstream,
            global_limit: Arc::new(crate::limiter::Limiter::new(config.speed_limit_bps)),
            task_limit: Arc::new(crate::limiter::Limiter::new(0)),
            header_timeout: timeout,
            read_timeout: Duration::from_secs(config.read_timeout_secs),
            resume_partial_segments: config.resume_partial_segments,
            partial_flush_bytes: config.partial_flush_bytes,
            has_proxy: config.proxy.is_some() || config.use_system_proxy,
            cache: config.resume_cache_dir.clone().map(|path| {
                std::sync::Arc::new(crate::cache::Cache::new(path, config.cache_max_mib, config.cache_resource_mib))
            }),
            max_redirects: config.max_redirects,
            max_resources: config.max_resources,
            max_concurrent_requests: config.max_concurrent_requests,
            live_manifest_refresh: Duration::from_millis(config.live_manifest_refresh_ms),
            request_retries: config.request_retries,
            retry_delay_ms: config.retry_delay_ms,
        })
    }

    pub fn for_task(&self, limit_bps: u64) -> Self {
        let mut cloned = self.clone();
        cloned.task_limit = Arc::new(crate::limiter::Limiter::new(limit_bps));
        cloned
    }
    pub fn set_task_speed_limit(&self, limit_bps: u64) {
        self.task_limit.set_rate(limit_bps);
    }
    #[allow(dead_code)] // mmsh_gateway.rs 尚未挂载
    pub fn has_limits(&self) -> bool {
        self.global_limit.rate() != 0 || self.task_limit.rate() != 0
    }
    pub fn cache_scope(&self, auth: &MediaSessionAuth) -> String {
        format!("{}:{}", self.upstream.cache_scope(), auth.cache_scope())
    }

    pub async fn get(
        &self,
        url: Url,
        public: &HeaderMap,
        auth: &MediaSessionAuth,
        cancel: &CancellationToken,
    ) -> Result<NetworkResponse, MediaError> {
        self.get_inner(url, public, auth, cancel, false).await
    }

    /// MMSH needs repeatable Pragma fields for its describe/play negotiation.
    /// Only the capability-authenticated loopback gateway may call this entry.
    #[allow(dead_code)] // mmsh_gateway.rs 尚未挂载
    pub(crate) async fn get_mmsh(
        &self,
        url: Url,
        public: &HeaderMap,
        auth: &MediaSessionAuth,
        cancel: &CancellationToken,
    ) -> Result<NetworkResponse, MediaError> {
        self.get_inner(url, public, auth, cancel, true).await
    }

    async fn get_inner(
        &self,
        mut url: Url,
        public: &HeaderMap,
        auth: &MediaSessionAuth,
        cancel: &CancellationToken,
        mmsh: bool,
    ) -> Result<NetworkResponse, MediaError> {
        let mut headers = public.clone();
        if headers.keys().any(|key| {
            !(mmsh && key == header::PRAGMA)
                && !matches!(
                    key.as_str(),
                    "user-agent" | "accept" | "accept-language" | "referer" | "range" | "if-none-match" | "if-range"
                )
        }) {
            return Err(MediaError::UnsupportedHeader);
        }
        let mut credentials_allowed = true;
        for hop in 0..=self.max_redirects {
            network_url(url.as_str())?;
            let mut attempt = 0;
            let response = loop {
                let request =
                    self.client.get(url.clone()).headers(headers.clone()).header(header::ACCEPT_ENCODING, "identity");
                let request = if credentials_allowed { auth.apply(request, &url) } else { request };
                let request = request.build().map_err(|_| MediaError::InvalidHeader)?;
                let response = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(MediaError::Cancelled),
                    result = tokio::time::timeout(self.header_timeout, self.upstream.get(&url, request.headers())) => match result {
                        Ok(result) => result.map_err(media_error),
                        Err(_) => Err(MediaError::Network),
                    },
                };
                let retry = match &response {
                    Ok(response) => {
                        response.status().is_server_error()
                            || matches!(response.status(), StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS)
                    }
                    Err(error) => matches!(error, MediaError::Network),
                };
                if !retry || attempt == self.request_retries {
                    break response?;
                }
                drop(response);
                let delay = self.retry_delay_ms.saturating_mul(1u64 << attempt.min(7)).min(30_000);
                attempt += 1;
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(MediaError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
                }
            };
            if !matches!(
                response.status(),
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            ) {
                return Ok(NetworkResponse {
                    inner: response,
                    pending: Bytes::new(),
                    global_limit: self.global_limit.clone(),
                    task_limit: self.task_limit.clone(),
                    cancel: cancel.clone(),
                    read_timeout: self.read_timeout,
                });
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(MediaError::Network)?;
            if hop == self.max_redirects || location.contains(['\r', '\n', '\0']) {
                return Err(MediaError::Network);
            }
            let next = url.join(location).map_err(|_| MediaError::InvalidUrl)?;
            network_url(next.as_str())?;
            if url.scheme() == "https" && next.scheme() != "https" {
                return Err(MediaError::InvalidUrl);
            }
            if url.origin() != next.origin() {
                credentials_allowed = false;
                headers.remove(header::REFERER);
            }
            url = next;
        }
        Err(MediaError::Network)
    }
}

/// All consumers use this surface so concurrent requests share the same two limiters.
pub(crate) struct NetworkResponse {
    inner: HttpResponse,
    pending: Bytes,
    global_limit: Arc<crate::limiter::Limiter>,
    task_limit: Arc<crate::limiter::Limiter>,
    cancel: CancellationToken,
    read_timeout: Duration,
}
impl NetworkResponse {
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }
    pub fn headers(&self) -> &HeaderMap {
        self.inner.headers()
    }
    pub fn url(&self) -> &Url {
        self.inner.url()
    }
    pub fn content_length(&self) -> Option<u64> {
        self.inner.content_length()
    }
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, MediaError> {
        if self.cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        if self.pending.is_empty() {
            let chunk = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(MediaError::Cancelled),
                result = tokio::time::timeout(self.read_timeout, self.inner.chunk()) => result.map_err(|_| MediaError::Network)?.map_err(|_| MediaError::Network)?,
            };
            let Some(chunk) = chunk else {
                return Ok(None);
            };
            self.pending = chunk;
        }
        let count = self.pending.len().min(self.global_limit.quantum()).min(self.task_limit.quantum());
        self.task_limit.acquire(count, &self.cancel).await?;
        self.global_limit.acquire(count, &self.cancel).await?;
        Ok(Some(self.pending.split_to(count)))
    }
    pub fn bytes_stream(self) -> impl futures_util::Stream<Item = Result<Bytes, MediaError>> + Send {
        futures_util::stream::unfold(Some(self), |response| async move {
            let mut response = response?;
            match response.chunk().await {
                Ok(Some(chunk)) => Some((Ok(chunk), Some(response))),
                Ok(None) => None,
                Err(error) => Some((Err(error), None)),
            }
        })
    }
}

fn media_error(error: swoop_core::Error) -> MediaError {
    match error {
        // Authentication messages are controlled by the core adapter and vault;
        // curl/reqwest strings can contain sensitive URLs and are never exposed.
        swoop_core::Error::Authentication(message) => MediaError::ProxyAuthentication(message),
        _ => MediaError::Network,
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
