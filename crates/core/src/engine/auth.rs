use std::sync::Arc;

use super::Engine;
use crate::auth::{VaultLease, new_credential_ref};
use crate::network::{NetworkPolicy, NetworkRuntime};
use crate::{AuthSecret, CredentialStorage, CredentialVault, Error, Result, SiteAuthRule};

const SETTING: &str = "site_auth_rules";
impl Engine {
    pub fn proxy_credentials_snapshot(&self) -> Option<crate::auth::ProxyCredentials> {
        let network = self.inner.network.lock().unwrap();
        network.policy.proxy_auth.clone().map(|rule| crate::auth::ProxyCredentials::snapshot(rule, &network.auth))
    }
    /// Saves policy and non-secret proxy identity together; call off the UI thread.
    /// A missing password retains credentials only for the exact same endpoint and identity.
    pub fn save_network_policy(&self, mut policy: NetworkPolicy, password: Option<AuthSecret>) -> Result<()> {
        let password_changed = password.is_some();
        // The editor may change storage on a cloned persisted rule. Never accept a caller's
        // credential reference; retain only our matching current identity below.
        if let Some(rule) = &mut policy.proxy_auth {
            rule.credential_ref = None;
        }
        let mut replacement = NetworkRuntime::new(policy, &self.inner.config.user_agent)?;
        let mut current = self.inner.network.lock().unwrap();
        let previous = current.policy.proxy_auth.as_ref();
        let mut prepared = None;
        let mut session = None;
        if let Some(rule) = &mut replacement.policy.proxy_auth {
            rule.check_capability()?;
            let matching = previous.filter(|old| rule.same_identity(old));
            if rule.scheme == crate::ProxyAuthScheme::Socks5
                && password.as_ref().is_some_and(|secret| !(1..=255).contains(&secret.expose().len()))
            {
                return Err(Error::Authentication("SOCKS5 密码需为 1–255 字节".into()));
            }
            match rule.storage {
                CredentialStorage::Session => {
                    rule.credential_ref = None;
                    session = Some(
                        password
                            .map(Arc::new)
                            .or_else(|| matching.and_then(|_| current.auth.proxy_secret.clone()))
                            .ok_or_else(|| Error::Authentication("请输入代理会话密码".into()))?,
                    );
                }
                CredentialStorage::System => {
                    if let Some(password) = password {
                        let key = new_credential_ref(&format!("proxy:{}", rule.endpoint));
                        current.auth.vault.set(&key, &password)?;
                        prepared = Some(key.clone());
                        rule.credential_ref = Some(key);
                    } else {
                        rule.credential_ref = matching.and_then(|old| old.credential_ref.clone());
                        if rule.credential_ref.is_none() {
                            return Err(Error::Authentication("请输入要保存至系统凭据库的代理密码".into()));
                        }
                    }
                }
                CredentialStorage::CurrentUser => {
                    if password.is_some() {
                        return Err(Error::Authentication("当前系统身份不接受代理密码".into()));
                    }
                    rule.credential_ref = None;
                }
            }
        } else if password.is_some() {
            return Err(Error::Authentication("请先启用代理认证".into()));
        }
        if let Some(store) = &self.inner.store {
            let save = serde_json::to_string(&replacement.policy)
                .map_err(Error::from)
                .and_then(|json| store.set_setting("network_policy", &json));
            if let Err(error) = save {
                if let Some(key) = prepared {
                    let _ = current.auth.vault.delete(&key);
                }
                return Err(error);
            }
        }
        replacement.inherit_auth(&current);
        replacement.auth.proxy_secret = session;
        if password_changed {
            replacement.auth.proxy_scope = Arc::new(new_credential_ref("proxy-snapshot"));
        }
        let old_key = previous.and_then(|rule| rule.credential_ref.as_ref());
        let new_key = replacement.policy.proxy_auth.as_ref().and_then(|rule| rule.credential_ref.as_ref());
        if old_key != new_key
            && let Some(lease) = &current.auth.proxy_lease
        {
            lease.retire();
        }
        *current = replacement;
        Ok(())
    }

    pub fn site_auth_rules(&self) -> Vec<SiteAuthRule> {
        self.inner.network.lock().unwrap().auth.rules.clone()
    }

    /// Synchronous OS vault operations may request OS unlock; call from a background UI worker.
    /// A new vault item is prepared first; SQLite commit precedes replacing the runtime snapshot.
    /// Omitting a password only retains credentials if origin, scheme and username are unchanged.
    pub fn set_site_auth(&self, mut rule: SiteAuthRule, password: Option<AuthSecret>) -> Result<()> {
        rule.validate()?;
        rule.check_capability()?;
        let mut network = self.inner.network.lock().unwrap();
        let auth = &mut network.auth;
        let previous = auth.rules.iter().find(|old| old.origin == rule.origin).cloned();
        if previous.is_none() && auth.rules.len() >= 128 {
            return Err(Error::Authentication("站点认证最多保存 128 项".into()));
        }
        let matching = previous
            .as_ref()
            .filter(|old| old.scheme == rule.scheme && old.username == rule.username && old.storage == rule.storage);
        let mut prepared = None;
        let session = match rule.storage {
            CredentialStorage::Session => {
                rule.credential_ref = None;
                Some(
                    password
                        .map(Arc::new)
                        .or_else(|| matching.and_then(|_| auth.secrets.get(&rule.origin).cloned()))
                        .ok_or_else(|| Error::Authentication("请输入会话密码".into()))?,
                )
            }
            CredentialStorage::System => {
                if let Some(password) = &password {
                    let key = new_credential_ref(&rule.origin);
                    auth.vault.set(&key, password)?;
                    prepared = Some(key.clone());
                    rule.credential_ref = Some(key);
                } else {
                    rule.credential_ref = matching.and_then(|old| old.credential_ref.clone());
                    if rule.credential_ref.is_none() {
                        return Err(Error::Authentication("请输入要保存至系统凭据库的密码".into()));
                    }
                }
                None
            }
            CredentialStorage::CurrentUser => {
                if password.is_some() {
                    return Err(Error::Authentication("当前系统身份不接受密码".into()));
                }
                rule.credential_ref = None;
                None
            }
        };
        let mut rules = auth.rules.clone();
        rules.retain(|old| old.origin != rule.origin);
        rules.push(rule.clone());
        rules.sort_by(|a, b| a.origin.cmp(&b.origin));
        if let Some(store) = &self.inner.store {
            let save =
                serde_json::to_string(&rules).map_err(Error::from).and_then(|json| store.set_setting(SETTING, &json));
            if let Err(error) = save {
                if let Some(key) = prepared {
                    let _ = auth.vault.delete(&key);
                }
                return Err(error);
            }
        }
        auth.rules = rules;
        if let Some(key) = &rule.credential_ref {
            auth.leases
                .entry(key.clone())
                .or_insert_with(|| Arc::new(VaultLease::new(key.clone(), auth.vault.clone())));
        }
        auth.secrets.remove(&rule.origin);
        if let Some(secret) = session {
            auth.secrets.insert(rule.origin.clone(), secret);
        }
        if let Some(key) =
            previous.and_then(|old| old.credential_ref).filter(|key| Some(key) != rule.credential_ref.as_ref())
            && let Some(lease) = auth.leases.remove(&key)
        {
            lease.retire();
        }
        Ok(())
    }

    pub fn remove_site_auth(&self, origin: &str) -> Result<()> {
        let mut network = self.inner.network.lock().unwrap();
        let auth = &mut network.auth;
        let previous = auth.rules.iter().find(|rule| rule.origin == origin).cloned();
        let mut rules = auth.rules.clone();
        rules.retain(|rule| rule.origin != origin);
        if let Some(store) = &self.inner.store {
            store.set_setting(SETTING, &serde_json::to_string(&rules)?)?;
        }
        auth.rules = rules;
        auth.secrets.remove(origin);
        if let Some(key) = previous.and_then(|rule| rule.credential_ref)
            && let Some(lease) = auth.leases.remove(&key)
        {
            lease.retire();
        }
        Ok(())
    }

    /// Dependency injection for tests / platform adapters. It never migrates existing secrets.
    pub fn set_credential_vault(&self, vault: Arc<dyn CredentialVault>) {
        let mut network = self.inner.network.lock().unwrap();
        network.auth.vault = vault.clone();
        network.auth.leases = network
            .auth
            .rules
            .iter()
            .filter_map(|rule| rule.credential_ref.as_ref())
            .map(|key| (key.clone(), Arc::new(VaultLease::new(key.clone(), vault.clone()))))
            .collect();
        network.auth.proxy_lease = network
            .policy
            .proxy_auth
            .as_ref()
            .and_then(|rule| rule.credential_ref.as_ref())
            .map(|key| Arc::new(VaultLease::new(key.clone(), vault.clone())));
    }

    pub(super) fn load_site_auth(&self) -> Result<()> {
        if let Some(store) = &self.inner.store
            && let Some(json) = store.get_setting(SETTING)?
        {
            self.inner.network.lock().unwrap().auth.load(&json)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpAuthScheme;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Vault(Mutex<BTreeMap<String, AuthSecret>>);
    impl CredentialVault for Vault {
        fn get(&self, key: &str) -> Result<Option<AuthSecret>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &AuthSecret) -> Result<()> {
            self.0.lock().unwrap().insert(key.into(), value.clone());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
    #[tokio::test]
    async fn active_network_keeps_retired_secret_until_last_snapshot_is_released() {
        let engine = Engine::new(tokio::runtime::Handle::current(), Default::default());
        let vault = Arc::new(Vault::default());
        engine.set_credential_vault(vault.clone());
        let rule =
            SiteAuthRule::new("https://example.com", HttpAuthScheme::Digest, "u".into(), CredentialStorage::System)
                .unwrap();
        engine.set_site_auth(rule.clone(), Some(AuthSecret::new("first".into()).unwrap())).unwrap();
        let active = engine.inner.network.lock().unwrap().clone();
        engine.set_site_auth(rule, Some(AuthSecret::new("second".into()).unwrap())).unwrap();
        assert_eq!(vault.0.lock().unwrap().len(), 2);
        let old = &active.auth.rules[0];
        assert_eq!(active.auth.credentials(old).await.unwrap().unwrap().expose(), "first");
        drop(active);
        assert_eq!(vault.0.lock().unwrap().len(), 1);
        engine.remove_site_auth("https://example.com").unwrap();
        assert!(vault.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn active_proxy_snapshot_keeps_previous_vault_password_until_released() {
        let engine = Engine::new(tokio::runtime::Handle::current(), Default::default());
        let vault = Arc::new(Vault::default());
        engine.set_credential_vault(vault.clone());
        let policy = NetworkPolicy {
            proxy: Some(crate::ProxyConfig { url: "http://127.0.0.1:12345".into(), bypass: vec![] }),
            proxy_auth: Some(
                crate::ProxyAuthRule::new(
                    "http://127.0.0.1:12345",
                    crate::ProxyAuthScheme::Basic,
                    "user".into(),
                    CredentialStorage::System,
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        engine.save_network_policy(policy.clone(), Some(AuthSecret::new("old-password".into()).unwrap())).unwrap();
        let active = engine.inner.network.lock().unwrap().clone();
        engine.save_network_policy(policy, Some(AuthSecret::new("new-password".into()).unwrap())).unwrap();
        assert_eq!(vault.0.lock().unwrap().len(), 2);
        assert_eq!(
            active.auth.proxy_credentials(active.policy.proxy_auth.as_ref().unwrap()).await.unwrap().unwrap().expose(),
            "old-password"
        );
        let current = engine.inner.network.lock().unwrap().clone();
        assert_eq!(
            current
                .auth
                .proxy_credentials(current.policy.proxy_auth.as_ref().unwrap())
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "new-password"
        );
        drop(active);
        assert_eq!(vault.0.lock().unwrap().len(), 1);
        drop(current);
        engine.save_network_policy(NetworkPolicy::default(), None).unwrap();
        assert!(vault.0.lock().unwrap().is_empty());
    }
}
