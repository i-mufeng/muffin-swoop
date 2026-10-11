//! Proxy-only HTTP adapter. Redirects, cancellation and consumer-aware deadlines
//! remain owned by the caller; no site credentials are activated implicitly.
use super::ProxyCredentials;
use crate::{Error, NetworkPolicy, Result};
use reqwest::{
    Client, StatusCode,
    header::{HeaderMap, PROXY_AUTHORIZATION},
};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct ProxyHttpClient {
    client: Client,
    policy: NetworkPolicy,
    credentials: Option<ProxyCredentials>,
    user_agent: String,
}
impl ProxyHttpClient {
    pub fn new(policy: NetworkPolicy, user_agent: &str, credentials: Option<ProxyCredentials>) -> Result<Self> {
        policy.validate()?;
        if policy.proxy_auth.as_ref() != credentials.as_ref().map(ProxyCredentials::rule) {
            return Err(Error::Authentication("代理身份与凭据快照不匹配".into()));
        }
        if let Some(rule) = &policy.proxy_auth {
            rule.check_capability()?;
        }
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(policy.connect_timeout_secs));
        if let Some(config) = &policy.proxy {
            builder = builder.no_proxy().proxy(
                reqwest::Proxy::all(&config.url)?.no_proxy(reqwest::NoProxy::from_string(&config.bypass.join(","))),
            );
        } else if !policy.use_system_proxy {
            builder = builder.no_proxy();
        }
        Ok(Self { client: builder.build()?, policy, credentials, user_agent: user_agent.to_owned() })
    }
    /// Sends exactly one request. Callers enforce deadlines only while awaiting network input.
    pub async fn get(&self, url: &Url, headers: &HeaderMap) -> Result<HttpResponse> {
        if headers.contains_key(PROXY_AUTHORIZATION)
            || !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Authentication("HTTP 适配器拒绝 URL 凭据或代理认证请求头".into()));
        }
        let bypass = self.policy.proxy.as_ref().is_some_and(|proxy| proxy.bypasses(url));
        let inner = if let Some(credentials) = &self.credentials {
            let secret = if bypass { None } else { credentials.runtime.proxy_credentials(&credentials.rule).await? };
            let mut policy = self.policy.clone();
            policy.request_timeout_secs = None;
            if bypass {
                policy.proxy = None;
                policy.proxy_auth = None;
                policy.use_system_proxy = false;
            }
            crate::network::NetworkResponse::Auth(
                super::transport::get_with_idle_timeout(
                    policy,
                    self.user_agent.clone(),
                    url.clone(),
                    headers.clone(),
                    None,
                    secret,
                    false,
                )
                .await?,
            )
        } else {
            crate::network::NetworkResponse::Http(self.client.get(url.clone()).headers(headers.clone()).send().await?)
        };
        Ok(HttpResponse { inner })
    }
    pub fn cache_scope(&self) -> &str {
        self.credentials.as_ref().map_or("public", ProxyCredentials::cache_scope)
    }
}

pub struct HttpResponse {
    inner: crate::network::NetworkResponse,
}
impl HttpResponse {
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
    pub async fn chunk(&mut self) -> Result<Option<bytes::Bytes>> {
        self.inner.chunk().await
    }
}
