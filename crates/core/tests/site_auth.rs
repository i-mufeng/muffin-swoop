use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use swoop_core::{
    AddOptions, AuthSecret, Config, CredentialStorage, CredentialVault, Engine, Error, HttpAuthScheme, NetworkPolicy,
    Result, SiteAuthRule, Store, TaskStatus, auth_capabilities,
};

struct Server {
    child: Child,
    origin: String,
}
impl Server {
    fn start() -> Self {
        let python = std::env::var("SWOOP_FTP_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
        let mut child = Command::new(python)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/auth_server.py"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let port = line.trim().parse::<u16>().expect("local authentication fixture failed to start");
        Self { child, origin: format!("http://127.0.0.1:{port}") }
    }
    async fn history(&self) -> Vec<serde_json::Value> {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{}/history", self.origin))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn engine(store: Option<Arc<Store>>) -> Engine {
    let handle = tokio::runtime::Handle::current();
    let config = Config { retries: 0, connections: 4, min_segment: 65536, ..Default::default() };
    let engine = match store {
        Some(store) => Engine::with_store(handle, config, store),
        None => Engine::new(handle, config),
    };
    engine.set_network_policy(NetworkPolicy { use_system_proxy: false, retries: 0, ..Default::default() }).unwrap();
    engine
}
fn rule(origin: &str, scheme: HttpAuthScheme, storage: CredentialStorage) -> SiteAuthRule {
    let user = if scheme == HttpAuthScheme::Ntlm { "TEST\\user" } else { "user" };
    let mut rule = SiteAuthRule::new(origin, scheme, user.into(), storage).unwrap();
    rule.allow_insecure_http = true;
    rule
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
async fn basic_digest_and_ntlm_real_challenges_feed_segmented_engine() {
    let server = Server::start();
    let engine = engine(None);
    let dir = tempfile::tempdir().unwrap();
    let capabilities = auth_capabilities();
    #[cfg(any(target_os = "macos", windows))]
    assert!(
        capabilities.ntlm && capabilities.negotiate,
        "packaged desktop curl must support NTLM and SPNEGO: {capabilities:?}"
    );
    eprintln!("HTTP auth transport capabilities: {capabilities:?}");
    let mut methods = vec![(HttpAuthScheme::Basic, "basic"), (HttpAuthScheme::Digest, "digest")];
    if auth_capabilities().ntlm {
        methods.push((HttpAuthScheme::Ntlm, "ntlm"));
    }
    for (method, name) in methods {
        engine
            .set_site_auth(
                rule(&server.origin, method, CredentialStorage::Session),
                Some(AuthSecret::new("Password".into()).unwrap()),
            )
            .unwrap();
        let id = engine
            .add(AddOptions {
                url: format!("{}/{name}", server.origin),
                dir: dir.path().to_owned(),
                file_name: Some(name.into()),
                headers: vec![],
            })
            .unwrap();
        completed(&engine, id).await;
        let bytes = std::fs::read(dir.path().join(name)).unwrap();
        assert_eq!(bytes, (0..1048595).map(|index| (index % 251) as u8).collect::<Vec<_>>());
    }
    let history = server.history().await;
    assert!(history.iter().any(|r| r["path"] == "/digest" && r["auth"] == ""));
    assert!(history.iter().any(|r| r["auth"].as_str().unwrap().starts_with("Digest ")));
    if auth_capabilities().ntlm {
        assert!(history.iter().any(|r| r["auth"].as_str().unwrap().starts_with("NTLM TlRMTVNTUAAD")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_is_rejected_and_cross_origin_round_trip_never_restores_secrets() {
    let first = Server::start();
    let second = Server::start();
    let engine = engine(None);
    engine
        .set_site_auth(
            rule(&first.origin, HttpAuthScheme::Digest, CredentialStorage::Session),
            Some(AuthSecret::new("wrong-password".into()).unwrap()),
        )
        .unwrap();
    assert!(matches!(engine.probe_url(&format!("{}/digest", first.origin), &[]).await, Err(Error::Http(401))));
    engine
        .set_site_auth(
            rule(&first.origin, HttpAuthScheme::Basic, CredentialStorage::Session),
            Some(AuthSecret::new("Password".into()).unwrap()),
        )
        .unwrap();
    // The second origin also has a saved rule, but redirects must never activate it.
    engine
        .set_site_auth(
            rule(&second.origin, HttpAuthScheme::Basic, CredentialStorage::Session),
            Some(AuthSecret::new("Password".into()).unwrap()),
        )
        .unwrap();
    let mut second_url = url::Url::parse(&format!("{}/redirect", second.origin)).unwrap();
    second_url.query_pairs_mut().append_pair("to", &format!("{}/basic", first.origin));
    let mut first_url = url::Url::parse(&format!("{}/redirect", first.origin)).unwrap();
    first_url.query_pairs_mut().append_pair("to", second_url.as_str());
    let result = engine
        .probe_url(
            first_url.as_str(),
            &[("Cookie".into(), "session-marker".into()), ("X-Private".into(), "private-marker".into())],
        )
        .await;
    assert!(matches!(result, Err(Error::Http(401))));
    for entry in second.history().await {
        assert_eq!(entry["auth"], "");
        assert_eq!(entry["cookie"], "");
        assert_eq!(entry["secret"], "");
    }
    let history = first.history().await;
    let back = history.iter().find(|entry| entry["path"] == "/basic").unwrap();
    assert_eq!(back["auth"], "");
    assert_eq!(back["cookie"], "");
    assert!(matches!(
        engine.probe_url(&format!("{}/basic", first.origin), &[("Authorization".into(), "Bearer marker".into())]).await,
        Err(Error::Authentication(_))
    ));
}

#[derive(Default)]
struct FakeVault(Mutex<BTreeMap<String, AuthSecret>>);
impl CredentialVault for FakeVault {
    fn get(&self, key: &str) -> Result<Option<AuthSecret>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, secret: &AuthSecret) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), secret.clone());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vault_restores_but_session_password_never_enters_sqlite() {
    let server = Server::start();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("tasks.db")).unwrap());
    let vault = Arc::new(FakeVault::default());
    let first = engine(Some(store.clone()));
    first.set_credential_vault(vault.clone());
    first
        .set_site_auth(
            rule(&server.origin, HttpAuthScheme::Digest, CredentialStorage::System),
            Some(AuthSecret::new("Password".into()).unwrap()),
        )
        .unwrap();
    let persisted = store.get_setting("site_auth_rules").unwrap().unwrap();
    assert!(!persisted.contains("Password"));
    assert!(persisted.contains("credential_ref"));
    let second = engine(Some(store.clone()));
    second.set_credential_vault(vault.clone());
    second.load(false).unwrap();
    second.probe_url(&format!("{}/digest", server.origin), &[]).await.unwrap();
    second
        .set_site_auth(
            rule(&server.origin, HttpAuthScheme::Digest, CredentialStorage::Session),
            Some(AuthSecret::new("Password".into()).unwrap()),
        )
        .unwrap();
    assert!(vault.0.lock().unwrap().is_empty());
    let third = engine(Some(store));
    third.load(false).unwrap();
    assert!(matches!(third.probe_url(&format!("{}/digest", server.origin), &[]).await, Err(Error::Authentication(_))));
    third.remove_site_auth(&server.origin).unwrap();
    assert!(third.site_auth_rules().is_empty());
}

#[tokio::test]
async fn sqlite_failure_preserves_rules_and_cleans_prepared_vault_secret() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tasks.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = engine(Some(store.clone()));
    let vault = Arc::new(FakeVault::default());
    engine.set_credential_vault(vault.clone());
    let mut site = rule("http://localhost:12345", HttpAuthScheme::Digest, CredentialStorage::System);
    engine.set_site_auth(site.clone(), Some(AuthSecret::new("old-private-value".into()).unwrap())).unwrap();
    let before = store.get_setting("site_auth_rules").unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_auth BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL, 'test reject'); END;",
        )
        .unwrap();
    site.username = "changed-user".into();
    assert!(engine.set_site_auth(site, Some(AuthSecret::new("new-private-value".into()).unwrap())).is_err());
    assert_eq!(store.get_setting("site_auth_rules").unwrap(), before);
    assert_eq!(engine.site_auth_rules()[0].username, "user");
    assert_eq!(vault.0.lock().unwrap().len(), 1);
}
