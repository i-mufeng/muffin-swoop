use super::*;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use swoop_core::{AuthSecret, CredentialStorage, CredentialVault, Engine, ProxyAuthRule, ProxyAuthScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct ProxyFixture {
    child: Child,
    ports: BTreeMap<String, u16>,
}
impl ProxyFixture {
    fn start() -> Self {
        let mut child = Command::new(std::env::var("SWOOP_FTP_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/proxy_auth_server.py"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        Self { child, ports: serde_json::from_str(&line).expect("loopback proxy fixture startup") }
    }
    fn endpoint(&self, name: &str) -> String {
        format!("{}://127.0.0.1:{}", if name == "socks" { "socks5h" } else { "http" }, self.ports[name])
    }
    fn config(&self, name: &str) -> MediaNetworkConfig {
        MediaNetworkConfig {
            proxy: Some(MediaProxyConfig { url: self.endpoint(name), bypass: vec![] }),
            proxy_auth: Some(
                ProxyAuthRule::new(
                    &self.endpoint(name),
                    match name {
                        "basic" => ProxyAuthScheme::Basic,
                        "digest" => ProxyAuthScheme::Digest,
                        "ntlm" => ProxyAuthScheme::Ntlm,
                        "socks" => ProxyAuthScheme::Socks5,
                        _ => unreachable!(),
                    },
                    if name == "ntlm" { "TEST\\user" } else { "user" }.into(),
                    CredentialStorage::Session,
                )
                .unwrap(),
            ),
            request_retries: 0,
            ..Default::default()
        }
    }
    fn transport(&self, name: &str, password: &str) -> NetworkTransport {
        let config = self.config(name);
        let credentials = MediaProxyCredentials::session(
            config.proxy_auth.clone().unwrap(),
            AuthSecret::new(password.into()).unwrap(),
        )
        .unwrap();
        NetworkTransport::new_with_proxy(&config, Duration::from_secs(5), Some(credentials)).unwrap()
    }
    async fn history(&self) -> Vec<serde_json::Value> {
        Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{}/history", self.endpoint("origin")))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .map(|body| serde_json::from_slice(&body).unwrap())
            .unwrap()
    }
}
impl Drop for ProxyFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
async fn collect(transport: &NetworkTransport, url: &str, cancel: &CancellationToken) -> Result<Vec<u8>, MediaError> {
    let mut response =
        transport.get(Url::parse(url).unwrap(), &HeaderMap::new(), &MediaSessionAuth::default(), cancel).await?;
    if !response.status().is_success() {
        return Err(MediaError::Network);
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        data.extend(chunk);
    }
    Ok(data)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_proxy_challenges_and_socks_keep_credentials_out_of_origin() {
    let server = ProxyFixture::start();
    let mut methods = vec!["basic", "digest", "socks"];
    if swoop_core::auth_capabilities().ntlm {
        methods.push("ntlm");
    }
    for method in methods {
        let target = format!("{}/data", server.endpoint("origin"));
        assert!(collect(&server.transport(method, "wrong"), &target, &CancellationToken::new()).await.is_err());
        assert_eq!(
            collect(&server.transport(method, "Password"), &target, &CancellationToken::new()).await.unwrap(),
            (0..262163).map(|n| (n % 251) as u8).collect::<Vec<_>>()
        );
    }
    let history = server.history().await;
    assert!(history.iter().any(|entry| entry["kind"] == "proxy" && entry["auth_scheme"] == "Digest"));
    assert!(history.iter().any(|entry| entry["kind"] == "socks" && entry["accepted"] == false));
    assert!(history.iter().any(|entry| entry["kind"] == "socks" && entry["accepted"] == true));
    assert!(history.iter().filter(|entry| entry["kind"] == "origin").all(|entry| entry["proxy_auth"] == ""));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_origin_redirects_never_restore_site_secrets_but_keep_proxy_auth() {
    use crate::{MediaAuthorization, OriginCredentials, ScopedCookie};
    let server = ProxyFixture::start();
    let transport = server.transport("basic", "Password");
    let auth = MediaSessionAuth::new(
        ["origin", "second"]
            .into_iter()
            .map(|name| {
                OriginCredentials::new(
                    &server.endpoint(name),
                    Some(MediaAuthorization::Basic { username: "user".into(), password: "Password".into() }),
                    vec![ScopedCookie {
                        name: "session".into(),
                        value: "marker".into(),
                        path: "/".into(),
                        secure: false,
                        expires_at: None,
                    }],
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap();
    let mut second = Url::parse(&format!("{}/redirect", server.endpoint("second"))).unwrap();
    second.query_pairs_mut().append_pair("to", &format!("{}/basic", server.endpoint("origin")));
    let mut first = Url::parse(&format!("{}/redirect", server.endpoint("origin"))).unwrap();
    first.query_pairs_mut().append_pair("to", second.as_str());
    let response = transport.get(first, &HeaderMap::new(), &auth, &CancellationToken::new()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for entry in server.history().await.iter().filter(|entry| entry["kind"] == "origin") {
        assert_eq!(entry["proxy_auth"], "");
        if entry["port"] == server.ports["second"] || entry["path"] == "/basic" {
            assert_eq!(entry["auth"], "");
            assert_eq!(entry["cookie"], "");
        } else {
            assert_ne!(entry["auth"], "");
        }
    }
}

// Real loopback HTTP with separate socket reads, so read deadlines and limiter
// waits are exercised together, not merely token-bucket arithmetic.
async fn origin(bytes: usize, stall: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/body", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = vec![0; 4096];
                let _ = socket.read(&mut request).await;
                let _ = socket
                    .write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n").as_bytes(),
                    )
                    .await;
                if stall {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                let _ = socket.write_all(&vec![7; bytes]).await;
            });
        }
    });
    (url, task)
}
fn limited(global: u64) -> NetworkTransport {
    NetworkTransport::new(
        &MediaNetworkConfig { speed_limit_bps: global, read_timeout_secs: 1, request_retries: 0, ..Default::default() },
        Duration::from_secs(3),
    )
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_requests_share_global_and_task_limits() {
    let (url, origin) = origin(12_000, false).await;
    for (global, task_rate, separate_tasks) in [(24_000, 0, true), (0, 24_000, false), (48_000, 24_000, false)] {
        let transport = limited(global).for_task(task_rate);
        let second = if separate_tasks { transport.for_task(task_rate) } else { transport.clone() };
        let cancel = CancellationToken::new();
        let start = tokio::time::Instant::now();
        let (first, second) = tokio::join!(collect(&transport, &url, &cancel), collect(&second, &url, &cancel));
        assert_eq!(first.unwrap().len(), 12_000);
        assert_eq!(second.unwrap().len(), 12_000);
        assert!(start.elapsed() >= Duration::from_millis(850), "combined limit bypass: {:?}", start.elapsed());
        assert!(start.elapsed() < Duration::from_secs(4));
    }
    origin.abort();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn throttling_waits_do_not_count_as_read_timeout_and_updates_wake_waiters() {
    let (url, origin) = origin(4, false).await;
    let transport = limited(0).for_task(1);
    let start = tokio::time::Instant::now();
    assert_eq!(collect(&transport, &url, &CancellationToken::new()).await.unwrap(), vec![7; 4]);
    assert!(start.elapsed() >= Duration::from_millis(3800));
    let slow = transport.for_task(1);
    let cancel = CancellationToken::new();
    let mut response =
        slow.get(Url::parse(&url).unwrap(), &HeaderMap::new(), &MediaSessionAuth::default(), &cancel).await.unwrap();
    let waiting = tokio::spawn(async move { response.chunk().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    slow.set_task_speed_limit(0);
    assert!(tokio::time::timeout(Duration::from_millis(300), waiting).await.unwrap().unwrap().unwrap().is_some());
    origin.abort();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_waits_cancel_promptly_and_real_network_stalls_timeout() {
    let (url, origin_task) = origin(4, false).await;
    let transport = limited(1);
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move { collect(&transport, &url, &worker_cancel).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(300), worker).await.unwrap().unwrap(),
        Err(MediaError::Cancelled)
    ));
    origin_task.abort();
    let (url, origin_task) = origin(4, true).await;
    let start = tokio::time::Instant::now();
    assert!(matches!(collect(&limited(0), &url, &CancellationToken::new()).await, Err(MediaError::Network)));
    assert!(start.elapsed() >= Duration::from_millis(950) && start.elapsed() < Duration::from_secs(2));
    origin_task.abort();
}
#[derive(Default)]
struct FakeVault(Mutex<BTreeMap<String, AuthSecret>>);
impl CredentialVault for FakeVault {
    fn get(&self, key: &str) -> swoop_core::Result<Option<AuthSecret>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &AuthSecret) -> swoop_core::Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.clone());
        Ok(())
    }
    fn delete(&self, key: &str) -> swoop_core::Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inherited_secure_snapshots_are_stable_and_keep_old_vault_entry_alive() {
    let server = ProxyFixture::start();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(swoop_core::Store::open(&directory.path().join("tasks.db")).unwrap());
    let engine = Engine::with_store(tokio::runtime::Handle::current(), Default::default(), store.clone());
    let vault = Arc::new(FakeVault::default());
    engine.set_credential_vault(vault.clone());
    let mut config = server.config("basic");
    config.proxy_auth.as_mut().unwrap().storage = CredentialStorage::System;
    engine.save_network_policy(config.core_policy(), Some(AuthSecret::new("Password".into()).unwrap())).unwrap();
    let first = engine.proxy_credentials_snapshot().unwrap();
    assert_eq!(first.cache_scope(), engine.proxy_credentials_snapshot().unwrap().cache_scope());
    // The engine's persisted rule owns a newly-created opaque reference.
    config.proxy_auth = Some(first.rule().clone());
    let transport = NetworkTransport::new_with_proxy(&config, Duration::from_secs(5), Some(first.clone())).unwrap();
    let mut edited = engine.network_policy();
    edited.read_timeout_secs += 1;
    engine.save_network_policy(edited, None).unwrap();
    assert_eq!(first.cache_scope(), engine.proxy_credentials_snapshot().unwrap().cache_scope());
    engine.save_network_policy(config.core_policy(), Some(AuthSecret::new("changed".into()).unwrap())).unwrap();
    assert_ne!(first.cache_scope(), engine.proxy_credentials_snapshot().unwrap().cache_scope());
    assert_eq!(vault.0.lock().unwrap().len(), 2);
    assert_eq!(
        collect(&transport, &format!("{}/data", server.endpoint("origin")), &CancellationToken::new())
            .await
            .unwrap()
            .len(),
        262163
    );
    assert!(!store.get_setting("network_policy").unwrap().unwrap().contains("Password"));
    drop(first);
    drop(transport);
    assert_eq!(vault.0.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn endpoint_mismatch_fails_and_range_headers_stay_allowed() {
    let server = ProxyFixture::start();
    let config = server.config("basic");
    let wrong = MediaProxyCredentials::session(
        server.config("digest").proxy_auth.unwrap(),
        AuthSecret::new("Password".into()).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        NetworkTransport::new_with_proxy(&config, Duration::from_secs(5), Some(wrong)),
        Err(MediaError::ProxyAuthentication(_))
    ));
    let transport = server.transport("basic", "Password");
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, "bytes=3-5".parse().unwrap());
    headers.insert(header::IF_RANGE, "\"proxy-test-v1\"".parse().unwrap());
    let mut response = transport
        .get(
            Url::parse(&format!("{}/data", server.endpoint("origin"))).unwrap(),
            &headers,
            &MediaSessionAuth::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), [3, 4, 5]);
    headers.insert(header::PROXY_AUTHORIZATION, "Basic forbidden".parse().unwrap());
    assert!(matches!(
        transport
            .get(
                Url::parse(&format!("{}/data", server.endpoint("origin"))).unwrap(),
                &headers,
                &MediaSessionAuth::default(),
                &CancellationToken::new()
            )
            .await,
        Err(MediaError::UnsupportedHeader)
    ));
}
