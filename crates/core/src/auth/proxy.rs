//! Proxy credentials are scoped to one explicit proxy endpoint, never to a destination URL.
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use url::Url;

use super::{AuthRuntime, AuthSecret, CredentialStorage, HttpAuthScheme, SiteAuthRule, VaultLease};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAuthScheme {
    #[default]
    Basic,
    Digest,
    Ntlm,
    Negotiate,
    Socks5,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyAuthRule {
    pub endpoint: String,
    pub scheme: ProxyAuthScheme,
    pub username: String,
    pub storage: CredentialStorage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credential_ref: Option<String>,
}
impl std::fmt::Debug for ProxyAuthRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyAuthRule")
            .field(
                "endpoint",
                &canonical_endpoint(&self.endpoint).unwrap_or_else(|_| "<invalid proxy endpoint>".into()),
            )
            .field("scheme", &self.scheme)
            .field("username", &self.username)
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}
impl ProxyAuthRule {
    pub fn new(endpoint: &str, scheme: ProxyAuthScheme, username: String, storage: CredentialStorage) -> Result<Self> {
        let rule = Self { endpoint: canonical_endpoint(endpoint)?, scheme, username, storage, credential_ref: None };
        Ok(rule)
    }
    pub fn validate(&self) -> Result<()> {
        if canonical_endpoint(&self.endpoint)? != self.endpoint {
            return Err(Error::Authentication("代理认证端点必须使用规范地址".into()));
        }
        let socks = self.endpoint.starts_with("socks5:") || self.endpoint.starts_with("socks5h:");
        if socks != (self.scheme == ProxyAuthScheme::Socks5) {
            return Err(Error::Authentication("SOCKS5 代理必须选择 SOCKS5 用户名密码认证".into()));
        }
        if socks
            && (self.username.is_empty() || self.username.len() > 255 || self.storage == CredentialStorage::CurrentUser)
        {
            return Err(Error::Authentication("SOCKS5 用户名需为 1–255 字节，且不支持当前系统身份".into()));
        }
        self.site_shape().validate()
    }
    pub(crate) fn check_capability(&self) -> Result<()> {
        self.site_shape().check_capability()
    }
    fn site_shape(&self) -> SiteAuthRule {
        SiteAuthRule {
            origin: "https://proxy.invalid".into(),
            scheme: match self.scheme {
                ProxyAuthScheme::Basic => HttpAuthScheme::Basic,
                ProxyAuthScheme::Digest | ProxyAuthScheme::Socks5 => HttpAuthScheme::Digest,
                ProxyAuthScheme::Ntlm => HttpAuthScheme::Ntlm,
                ProxyAuthScheme::Negotiate => HttpAuthScheme::Negotiate,
            },
            username: self.username.clone(),
            storage: self.storage,
            allow_insecure_http: false,
            credential_ref: self.credential_ref.clone(),
        }
    }
    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.endpoint == other.endpoint
            && self.scheme == other.scheme
            && self.username == other.username
            && self.storage == other.storage
    }
}

pub(crate) fn canonical_endpoint(value: &str) -> Result<String> {
    let url = Url::parse(value.trim()).map_err(|_| Error::Authentication("代理认证地址无效".into()))?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Authentication("代理地址只能包含协议、主机和端口；密码需单独填写".into()));
    }
    Ok(url.to_string())
}

impl AuthRuntime {
    pub(crate) fn inherit_proxy(&mut self, rule: Option<&ProxyAuthRule>, previous: Option<&ProxyAuthRule>) {
        if !rule.zip(previous).is_some_and(|(new, old)| new.same_identity(old)) {
            self.proxy_secret = None;
            self.proxy_scope = Arc::new(super::new_credential_ref("proxy-snapshot"));
        }
        let key = rule.and_then(|rule| rule.credential_ref.as_ref());
        if self.proxy_lease.as_ref().map(|lease| &lease.key) != key {
            self.proxy_scope = Arc::new(super::new_credential_ref("proxy-snapshot"));
            self.proxy_lease = key.map(|key| Arc::new(VaultLease::new(key.clone(), self.vault.clone())));
        }
    }
    pub(crate) async fn proxy_credentials(&self, rule: &ProxyAuthRule) -> Result<Option<Arc<AuthSecret>>> {
        rule.validate()?;
        rule.check_capability()?;
        let secret = match rule.storage {
            CredentialStorage::CurrentUser => return Ok(None),
            CredentialStorage::Session => self
                .proxy_secret
                .clone()
                .ok_or_else(|| Error::Authentication("等待代理认证：请重新输入会话密码".into()))?,
            CredentialStorage::System => {
                let lease = self
                    .proxy_lease
                    .clone()
                    .ok_or_else(|| Error::Authentication("代理系统凭据引用缺失，请重新保存密码".into()))?;
                tokio::task::spawn_blocking(move || lease.vault.get(&lease.key))
                    .await
                    .map_err(|_| Error::Authentication("读取代理系统凭据失败".into()))??
                    .map(Arc::new)
                    .ok_or_else(|| Error::Authentication("代理系统凭据已失效，请重新保存密码".into()))?
            }
        };
        if rule.scheme == ProxyAuthScheme::Socks5 && !(1..=255).contains(&secret.expose().len()) {
            return Err(Error::Authentication("SOCKS5 密码需为 1–255 字节".into()));
        }
        Ok(Some(secret))
    }
}

/// A frozen credential handle, intentionally not serializable. Its vault lease
/// keeps old credentials alive when settings change during a download.
#[derive(Clone)]
pub struct ProxyCredentials {
    pub(crate) rule: ProxyAuthRule,
    pub(crate) runtime: AuthRuntime,
}
impl std::fmt::Debug for ProxyCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProxyCredentials(<redacted>)")
    }
}
impl ProxyCredentials {
    pub fn session(mut rule: ProxyAuthRule, password: AuthSecret) -> Result<Self> {
        if rule.storage != CredentialStorage::Session {
            return Err(Error::Authentication("独立代理凭据只支持本次会话".into()));
        }
        rule.credential_ref = None;
        rule.validate()?;
        rule.check_capability()?;
        if rule.scheme == ProxyAuthScheme::Socks5 && !(1..=255).contains(&password.expose().len()) {
            return Err(Error::Authentication("SOCKS5 密码需为 1–255 字节".into()));
        }
        let runtime = AuthRuntime { proxy_secret: Some(Arc::new(password)), ..Default::default() };
        Ok(Self { rule, runtime })
    }
    pub fn rule(&self) -> &ProxyAuthRule {
        &self.rule
    }
    pub fn cache_scope(&self) -> &str {
        &self.runtime.proxy_scope
    }
    pub(crate) fn snapshot(rule: ProxyAuthRule, runtime: &AuthRuntime) -> Self {
        Self {
            rule,
            runtime: AuthRuntime {
                vault: runtime.vault.clone(),
                proxy_secret: runtime.proxy_secret.clone(),
                proxy_lease: runtime.proxy_lease.clone(),
                proxy_scope: runtime.proxy_scope.clone(),
                ..Default::default()
            },
        }
    }
}
