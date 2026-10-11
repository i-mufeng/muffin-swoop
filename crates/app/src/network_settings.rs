use swoop_core::{NetworkPolicy, Store};

pub fn load(store: &Store) -> NetworkPolicy {
    store
        .get_setting("network_policy")
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<NetworkPolicy>(&json).ok())
        .filter(|policy| policy.validate().is_ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn policy_roundtrips_and_invalid_credential_proxy_is_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.db");
        let store = std::sync::Arc::new(Store::open(&path).unwrap());
        let mut policy = NetworkPolicy {
            retries: 2,
            read_timeout_secs: 60,
            proxy: Some(swoop_core::ProxyConfig {
                url: "socks5h://127.0.0.1:1080".into(),
                bypass: vec!["localhost".into(), "127.0.0.1".into()],
            }),
            ..Default::default()
        };
        let engine =
            swoop_core::Engine::with_store(tokio::runtime::Handle::current(), Default::default(), store.clone());
        engine.save_network_policy(policy.clone(), None).unwrap();
        drop(engine);
        drop(store);
        let store = std::sync::Arc::new(Store::open(&path).unwrap());
        assert_eq!(load(&store), policy);
        policy.proxy.as_mut().unwrap().url = "http://user:secret@127.0.0.1:8080".into();
        let engine =
            swoop_core::Engine::with_store(tokio::runtime::Handle::current(), Default::default(), store.clone());
        assert!(engine.save_network_policy(policy, None).is_err());
        assert!(!store.get_setting("network_policy").unwrap().unwrap().contains("secret"));
    }
}
