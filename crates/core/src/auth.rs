//! Exact-origin HTTP authentication. Only non-secret rules are serializable.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::{Error, Result};

mod http;
pub(crate) mod proxy;
pub(crate) mod transport;
pub use http::{HttpResponse, ProxyHttpClient};
pub use proxy::{ProxyAuthRule, ProxyAuthScheme, ProxyCredentials};
mod vault;
pub use vault::{CredentialVault, SystemCredentialVault};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpAuthScheme {
    Basic,
    #[default]
    Digest,
    Ntlm,
    Negotiate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStorage {
    #[default]
    Session,
    System,
    /// Current Kerberos ticket / Windows SSPI logon. No password is saved.
    CurrentUser,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteAuthRule {
    pub origin: String,
    pub scheme: HttpAuthScheme,
    pub username: String,
    pub storage: CredentialStorage,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credential_ref: Option<String>,
}

impl SiteAuthRule {
    pub fn new(origin: &str, scheme: HttpAuthScheme, username: String, storage: CredentialStorage) -> Result<Self> {
        let url = Url::parse(origin).map_err(|_| Error::Authentication("站点地址无效".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Authentication(
                "站点需填写 http/https 来源（协议、主机和可选端口），不能带路径或凭据".into(),
            ));
        }
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            scheme,
            username,
            storage,
            allow_insecure_http: false,
            credential_ref: None,
        })
    }

    pub fn validate(&self) -> Result<()> {
        let canonical = Self::new(&self.origin, self.scheme, self.username.clone(), self.storage)?;
        if canonical.origin != self.origin
            || self.username.len() > 512
            || self.username.contains(['\0', '\r', '\n'])
            || (self.scheme == HttpAuthScheme::Basic && self.username.contains(':'))
        {
            return Err(Error::Authentication("站点来源或用户名无效".into()));
        }
        if self.origin.starts_with("http:") && !self.allow_insecure_http {
            return Err(Error::Authentication("HTTP 站点认证需明确允许非加密连接；建议使用 HTTPS".into()));
        }
        if self.storage == CredentialStorage::CurrentUser {
            if !matches!(self.scheme, HttpAuthScheme::Negotiate | HttpAuthScheme::Ntlm) || !self.username.is_empty() {
                return Err(Error::Authentication("当前系统身份仅适用于 Negotiate/NTLM，用户名应留空".into()));
            }
        } else if self.username.is_empty() {
            return Err(Error::Authentication("请输入站点用户名".into()));
        }
        if self.credential_ref.as_deref().is_some_and(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Error::Authentication("系统凭据引用无效".into()));
        }
        if self.storage != CredentialStorage::System && self.credential_ref.is_some() {
            return Err(Error::Authentication("会话或当前身份不能带系统凭据引用".into()));
        }
        Ok(())
    }

    pub(crate) fn check_capability(&self) -> Result<()> {
        let caps = auth_capabilities();
        if self.scheme == HttpAuthScheme::Ntlm && !caps.ntlm {
            return Err(Error::Authentication("当前 libcurl 不支持 NTLM".into()));
        }
        if self.scheme == HttpAuthScheme::Negotiate && !caps.negotiate {
            return Err(Error::Authentication("当前 libcurl 不支持 SPNEGO/Kerberos；无法使用 Negotiate".into()));
        }
        if self.storage == CredentialStorage::CurrentUser && self.scheme == HttpAuthScheme::Ntlm && !caps.sspi {
            return Err(Error::Authentication(
                "当前系统身份的 NTLM 登录需要 Windows SSPI；此构建仅支持显式 NTLM 账号密码".into(),
            ));
        }
        Ok(())
    }
}

/// Zeroized on drop; deliberately has no Serialize implementation.
#[derive(Clone)]
pub struct AuthSecret(Zeroizing<String>);
impl AuthSecret {
    pub fn new(value: String) -> Result<Self> {
        let value = Zeroizing::new(value);
        if value.len() > 2048 || value.contains('\0') {
            return Err(Error::Authentication("密码长度不得超过 2048 字节，且不能包含空字符".into()));
        }
        Ok(Self(value))
    }
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}
impl fmt::Debug for AuthSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthSecret([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct AuthCapabilities {
    pub ntlm: bool,
    pub negotiate: bool,
    pub sspi: bool,
    pub system_vault: bool,
    pub transport_version: String,
}
pub fn auth_capabilities() -> AuthCapabilities {
    let version = curl::Version::get();
    AuthCapabilities {
        ntlm: version.feature_ntlm(),
        negotiate: version.feature_spnego(),
        sspi: version.feature_sspi(),
        system_vault: cfg!(any(target_os = "macos", windows)),
        transport_version: version.version().to_owned(),
    }
}

#[derive(Clone)]
pub(crate) struct AuthRuntime {
    pub rules: Vec<SiteAuthRule>,
    pub secrets: BTreeMap<String, Arc<AuthSecret>>,
    pub vault: Arc<dyn CredentialVault>,
    pub leases: BTreeMap<String, Arc<VaultLease>>,
    pub proxy_secret: Option<Arc<AuthSecret>>,
    pub proxy_lease: Option<Arc<VaultLease>>,
    pub proxy_scope: Arc<String>,
}
impl Default for AuthRuntime {
    fn default() -> Self {
        Self {
            rules: vec![],
            secrets: BTreeMap::new(),
            vault: Arc::new(SystemCredentialVault),
            leases: BTreeMap::new(),
            proxy_secret: None,
            proxy_lease: None,
            proxy_scope: Arc::new(new_credential_ref("proxy-snapshot")),
        }
    }
}
impl AuthRuntime {
    pub fn load(&mut self, json: &str) -> Result<()> {
        let rules: Vec<SiteAuthRule> = serde_json::from_str(json)?;
        if rules.len() > 128 {
            return Err(Error::Authentication("站点认证最多保存 128 项".into()));
        }
        let mut origins = std::collections::BTreeSet::new();
        for rule in &rules {
            rule.validate()?;
            if !origins.insert(rule.origin.clone()) {
                return Err(Error::Authentication("站点认证来源重复".into()));
            }
        }
        self.leases = rules
            .iter()
            .filter_map(|rule| rule.credential_ref.as_ref())
            .map(|key| (key.clone(), Arc::new(VaultLease::new(key.clone(), self.vault.clone()))))
            .collect();
        self.rules = rules;
        self.secrets.clear();
        Ok(())
    }

    pub fn rule(&self, url: &Url) -> Option<&SiteAuthRule> {
        let origin = url.origin().ascii_serialization();
        self.rules.iter().find(|rule| rule.origin == origin)
    }

    pub async fn credentials(&self, rule: &SiteAuthRule) -> Result<Option<Arc<AuthSecret>>> {
        rule.validate()?;
        rule.check_capability()?;
        match rule.storage {
            CredentialStorage::CurrentUser => Ok(None),
            CredentialStorage::Session => self
                .secrets
                .get(&rule.origin)
                .cloned()
                .map(Some)
                .ok_or_else(|| Error::Authentication("等待站点认证：请重新输入会话密码".into())),
            CredentialStorage::System => {
                let key = rule
                    .credential_ref
                    .clone()
                    .ok_or_else(|| Error::Authentication("系统凭据引用缺失，请重新保存密码".into()))?;
                let lease = self
                    .leases
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| Error::Authentication("系统凭据生命周期缺失".into()))?;
                tokio::task::spawn_blocking(move || lease.vault.get(&lease.key))
                    .await
                    .map_err(|_| Error::Authentication("读取系统凭据失败".into()))??
                    .map(|secret| Some(Arc::new(secret)))
                    .ok_or_else(|| Error::Authentication("系统凭据已失效或被移除，请重新保存密码".into()))
            }
        }
    }
}

pub(crate) fn new_credential_ref(origin: &str) -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut digest = Sha256::new();
    digest.update(origin.as_bytes());
    digest.update(std::process::id().to_le_bytes());
    digest.update(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes(),
    );
    digest.update(SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_le_bytes());
    format!("{:x}", digest.finalize())
}

/// Retired vault entries survive until all in-flight network snapshots release them.
/// Current persisted entries survive application exit (retired=false).
pub(crate) struct VaultLease {
    key: String,
    vault: Arc<dyn CredentialVault>,
    retired: std::sync::atomic::AtomicBool,
}
impl VaultLease {
    pub fn new(key: String, vault: Arc<dyn CredentialVault>) -> Self {
        Self { key, vault, retired: std::sync::atomic::AtomicBool::new(false) }
    }
    pub fn retire(&self) {
        self.retired.store(true, std::sync::atomic::Ordering::Release);
    }
}
impl Drop for VaultLease {
    fn drop(&mut self) {
        if self.retired.load(std::sync::atomic::Ordering::Acquire)
            && let Err(error) = self.vault.delete(&self.key)
        {
            tracing::warn!("旧站点凭据未能清理: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_origin_and_secret_boundaries() {
        let mut rule = SiteAuthRule::new(
            "http://EXAMPLE.com:80",
            HttpAuthScheme::Digest,
            "user".into(),
            CredentialStorage::Session,
        )
        .unwrap();
        assert_eq!(rule.origin, "http://example.com");
        assert!(rule.validate().is_err());
        rule.allow_insecure_http = true;
        rule.validate().unwrap();
        let runtime = AuthRuntime { rules: vec![rule], ..Default::default() };
        assert!(runtime.rule(&Url::parse("http://example.com/a").unwrap()).is_some());
        assert!(runtime.rule(&Url::parse("http://example.com:81/a").unwrap()).is_none());
        assert!(runtime.rule(&Url::parse("https://example.com/a").unwrap()).is_none());
        assert!(
            SiteAuthRule::new(
                "https://example.com/a",
                HttpAuthScheme::Basic,
                "user".into(),
                CredentialStorage::Session
            )
            .is_err()
        );
        assert!(
            SiteAuthRule::new(
                "https://u:p@example.com",
                HttpAuthScheme::Basic,
                "user".into(),
                CredentialStorage::Session
            )
            .is_err()
        );
        assert!(!format!("{:?}", AuthSecret::new("secret-marker".into()).unwrap()).contains("secret-marker"));
    }
    #[test]
    fn loaded_config_rejects_unknown_secret_fields_and_duplicates() {
        let mut runtime = AuthRuntime::default();
        assert!(runtime.load(r#"[{"origin":"https://example.com","scheme":"basic","username":"u","storage":"session","password":"bad"}]"#).is_err());
        let rule =
            SiteAuthRule::new("https://example.com", HttpAuthScheme::Digest, "u".into(), CredentialStorage::Session)
                .unwrap();
        assert!(runtime.load(&serde_json::to_string(&vec![rule.clone(), rule]).unwrap()).is_err());
    }
}
