use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use swoop_core::{
    AddOptions, AuthSecret, Config, CredentialStorage, CredentialVault, Engine, Error, NetworkPolicy, ProxyAuthRule,
    ProxyAuthScheme, ProxyConfig, Result, Store, TaskStatus,
};

struct Server {
    child: Child,
    ports: BTreeMap<String, u16>,
}
impl Server {
    fn start() -> Self {
        let mut child = Command::new(std::env::var("SWOOP_FTP_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proxy_auth_server.py"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        Self { child, ports: serde_json::from_str(&line).expect("proxy fixture failed to start") }
    }
    fn endpoint(&self, name: &str) -> String {
        format!("{}://127.0.0.1:{}", if name == "socks" { "socks5h" } else { "http" }, self.ports[name])
    }
    async fn history(&self) -> Vec<serde_json::Value> {
        serde_json::from_str(
            &reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("{}/history", self.endpoint("origin")))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
        )
        .unwrap()
    }
    fn policy(&self, method: &str, storage: CredentialStorage) -> NetworkPolicy {
        let scheme = match method {
            "basic" => ProxyAuthScheme::Basic,
            "digest" => ProxyAuthScheme::Digest,
            "ntlm" => ProxyAuthScheme::Ntlm,
            "socks" => ProxyAuthScheme::Socks5,
            _ => unreachable!(),
        };
        NetworkPolicy {
            proxy: Some(ProxyConfig { url: self.endpoint(method), bypass: vec![] }),
            proxy_auth: Some(
                ProxyAuthRule::new(
                    &self.endpoint(method),
                    scheme,
                    if method == "ntlm" { "TEST\\user" } else { "user" }.into(),
                    storage,
                )
                .unwrap(),
            ),
            retries: 0,
            use_system_proxy: false,
            ..Default::default()
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn secret(password: &str) -> Option<AuthSecret> {
    Some(AuthSecret::new(password.into()).unwrap())
}
fn engine(store: Option<Arc<Store>>) -> Engine {
    let config = Config { connections: 4, min_segment: 32768, retries: 0, ..Default::default() };
    match store {
        Some(store) => Engine::with_store(tokio::runtime::Handle::current(), config, store),
        None => Engine::new(tokio::runtime::Handle::current(), config),
    }
}
async fn completed(engine: &Engine, id: swoop_core::TaskId) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match engine.snapshot_one(id).unwrap().status {
                TaskStatus::Completed => break,
                TaskStatus::Failed(error) => panic!("download failed: {error}"),
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_basic_digest_ntlm_and_socks_proxy_auth_feed_segmented_downloads() {
    let server = Server::start();
    let engine = engine(None);
    let directory = tempfile::tempdir().unwrap();
    let mut methods = vec!["basic", "digest", "socks"];
    if swoop_core::auth_capabilities().ntlm {
        methods.push("ntlm");
    }
    for method in methods {
        engine
            .save_network_policy(server.policy(method, CredentialStorage::Session), secret("wrong-password"))
            .unwrap();
        assert!(engine.probe_url(&format!("{}/data", server.endpoint("origin")), &[]).await.is_err(), "{method}");
        engine.save_network_policy(server.policy(method, CredentialStorage::Session), secret("Password")).unwrap();
        let id = engine
            .add(AddOptions {
                url: format!("{}/data", server.endpoint("origin")),
                dir: directory.path().into(),
                file_name: Some(method.into()),
                headers: vec![],
            })
            .unwrap();
        completed(&engine, id).await;
        assert_eq!(
            std::fs::read(directory.path().join(method)).unwrap(),
            (0..262163).map(|index| (index % 251) as u8).collect::<Vec<_>>()
        );
    }
    let history = server.history().await;
    assert!(history.iter().any(|r| r["kind"] == "proxy" && r["auth_scheme"] == "Digest"));
    assert!(history.iter().any(|r| r["kind"] == "socks" && r["accepted"] == true));
    assert!(history.iter().any(|r| r["kind"] == "socks" && r["accepted"] == false));
    for entry in history.iter().filter(|r| r["kind"] == "origin") {
        assert_eq!(entry["proxy_auth"], "");
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn proxy_identity_survives_redirects_but_origin_secrets_never_do() {
    let server = Server::start();
    let engine = engine(None);
    engine.save_network_policy(server.policy("basic", CredentialStorage::Session), secret("Password")).unwrap();
    for origin in [server.endpoint("origin"), server.endpoint("second")] {
        let mut rule = swoop_core::SiteAuthRule::new(
            &origin,
            swoop_core::HttpAuthScheme::Basic,
            "user".into(),
            CredentialStorage::Session,
        )
        .unwrap();
        rule.allow_insecure_http = true;
        engine.set_site_auth(rule, secret("Password")).unwrap();
    }
    let mut second = url::Url::parse(&format!("{}/redirect", server.endpoint("second"))).unwrap();
    second.query_pairs_mut().append_pair("to", &format!("{}/basic", server.endpoint("origin")));
    let mut first = url::Url::parse(&format!("{}/redirect", server.endpoint("origin"))).unwrap();
    first.query_pairs_mut().append_pair("to", second.as_str());
    let result = engine
        .probe_url(
            first.as_str(),
            &[("Cookie".into(), "cookie-marker".into()), ("X-Private".into(), "private-marker".into())],
        )
        .await;
    assert!(matches!(result, Err(Error::Http(401))), "{result:?}");
    let history = server.history().await;
    for entry in history.iter().filter(|r| r["kind"] == "origin") {
        assert_eq!(entry["proxy_auth"], "");
        if entry["port"] == server.ports["second"] || entry["path"] == "/basic" {
            assert_eq!(entry["auth"], "");
            assert_eq!(entry["cookie"], "");
            assert_eq!(entry["private"], "");
        }
    }
    assert!(history.iter().filter(|r| r["kind"] == "proxy").all(|r| r["auth_scheme"] == "Basic"));
    assert!(
        engine.probe_url(first.as_str(), &[("Proxy-Authorization".into(), "Basic forbidden".into())]).await.is_err()
    );
}
#[derive(Default)]
struct FakeVault(Mutex<BTreeMap<String, AuthSecret>>);
impl CredentialVault for FakeVault {
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
#[tokio::test(flavor = "multi_thread")]
async fn system_proxy_credentials_restore_without_sqlite_secrets_and_session_credentials_do_not() {
    let server = Server::start();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("tasks.db")).unwrap());
    let vault = Arc::new(FakeVault::default());
    let first = engine(Some(store.clone()));
    first.set_credential_vault(vault.clone());
    first.save_network_policy(server.policy("digest", CredentialStorage::System), secret("Password")).unwrap();
    let persisted = store.get_setting("network_policy").unwrap().unwrap();
    assert!(!persisted.contains("Password"));
    assert!(persisted.contains("credential_ref"));
    let second = engine(Some(store.clone()));
    second.set_credential_vault(vault.clone());
    second.set_network_policy(serde_json::from_str(&persisted).unwrap()).unwrap();
    second.load(false).unwrap();
    second.probe_url(&format!("{}/data", server.endpoint("origin")), &[]).await.unwrap();
    // The UI edits a cloned persisted rule, which still contains its private vault reference.
    let mut session_policy = second.network_policy();
    session_policy.proxy_auth.as_mut().unwrap().storage = CredentialStorage::Session;
    second.save_network_policy(session_policy, secret("Password")).unwrap();
    let third = engine(Some(store.clone()));
    third
        .set_network_policy(serde_json::from_str(&store.get_setting("network_policy").unwrap().unwrap()).unwrap())
        .unwrap();
    assert!(matches!(
        third.probe_url(&format!("{}/data", server.endpoint("origin")), &[]).await,
        Err(Error::Authentication(_))
    ));
    let mut changed = server.policy("basic", CredentialStorage::Session);
    assert!(second.save_network_policy(changed.clone(), None).is_err());
    changed.proxy.as_mut().unwrap().url = server.endpoint("socks");
    assert!(changed.validate().is_err());
}
#[tokio::test]
async fn failed_database_commit_preserves_network_and_removes_prepared_secret() {
    let server = Server::start();
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("prefs.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = engine(Some(store.clone()));
    let vault = Arc::new(FakeVault::default());
    engine.set_credential_vault(vault.clone());
    engine.save_network_policy(server.policy("basic", CredentialStorage::System), secret("old-secret-marker")).unwrap();
    let before = engine.network_policy();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_proxy BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL, 'test reject'); END;",
        )
        .unwrap();
    assert!(
        engine
            .save_network_policy(server.policy("digest", CredentialStorage::System), secret("new-secret-marker"))
            .is_err()
    );
    assert_eq!(engine.network_policy(), before);
    assert_eq!(vault.0.lock().unwrap().len(), 1);
    let json = store.get_setting("network_policy").unwrap().unwrap();
    assert_eq!(serde_json::from_str::<NetworkPolicy>(&json).unwrap(), before);
    assert!(!json.contains("secret-marker"));
}
#[tokio::test]
async fn ftp_inherited_authenticated_proxy_is_explicitly_rejected() {
    let server = Server::start();
    let engine = engine(None);
    engine.save_network_policy(server.policy("basic", CredentialStorage::Session), secret("Password")).unwrap();
    assert!(
        matches!(engine.probe_url("ftp://127.0.0.1:1/file", &[]).await, Err(Error::InvalidNetwork(message)) if message.contains("FTP 暂不支持继承代理账号"))
    );
}

#[tokio::test]
async fn bypassed_destinations_do_not_require_or_receive_proxy_credentials() {
    let server = Server::start();
    let engine = engine(None);
    for bypass in ["127.0.0.0/8", "127.0.0.1", "*"] {
        let mut policy = server.policy("basic", CredentialStorage::Session);
        policy.proxy.as_mut().unwrap().bypass = vec![bypass.into()];
        // Simulate a restarted application: a session identity exists, but its password is absent.
        engine.set_network_policy(policy).unwrap();
        engine.probe_url(&format!("{}/data", server.endpoint("origin")), &[]).await.unwrap();
    }
    let history = server.history().await;
    assert!(!history.iter().any(|entry| entry["kind"] == "proxy"));
    assert!(history.iter().filter(|entry| entry["kind"] == "origin").all(|entry| entry["proxy_auth"] == ""));
}

#[test]
fn proxy_metadata_rejects_secret_fields_wrong_protocol_identity_and_invalid_socks_lengths() {
    let mut rule = ProxyAuthRule::new(
        "socks5h://localhost:1080",
        ProxyAuthScheme::Socks5,
        "u".repeat(256),
        CredentialStorage::Session,
    )
    .unwrap();
    assert!(rule.validate().is_err());
    rule.username = "user".into();
    rule.storage = CredentialStorage::CurrentUser;
    assert!(rule.validate().is_err());
    rule.storage = CredentialStorage::Session;
    rule.scheme = ProxyAuthScheme::Basic;
    assert!(rule.validate().is_err());
    let mut json = serde_json::to_value(&rule).unwrap();
    json["password"] = "forbidden-secret".into();
    assert!(serde_json::from_value::<ProxyAuthRule>(json).is_err());
    rule.endpoint = "http://user:private-debug-marker@localhost".into();
    assert!(!format!("{rule:?}").contains("private-debug-marker"));
    assert!(
        ProxyAuthRule::new(
            "http://user:secret@localhost",
            ProxyAuthScheme::Basic,
            "user".into(),
            CredentialStorage::Session
        )
        .is_err()
    );
}
