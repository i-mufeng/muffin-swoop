use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use swoop_ftp::{
    ActiveMode, Error, FtpClient, FtpConfig, FtpCredentials, FtpDataMode, FtpProxy, FtpProxyKind,
    FtpRequest, FtpTlsMode,
};

struct Server {
    child: Child,
    root: tempfile::TempDir,
    port: u16,
    proxy_ports: [u16; 3],
}

fn crate_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if manifest.join("tests/server.py").is_file() {
        return manifest.to_path_buf();
    }
    // 独立验证 manifest 的 [[test]].path 是源码绝对路径。
    Path::new(file!())
        .canonicalize()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

impl Server {
    fn start(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let python = std::env::var_os("SWOOP_FTP_TEST_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate_dir().join("../../target/ftp-fixture-venv/bin/python"));
        let mut child = Command::new(python).arg(crate_dir().join("tests/server.py"))
            .arg(root.path()).arg(mode).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .spawn().expect("Install the documented target-local FTP fixture venv before running integration tests");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: serde_json::Value =
            serde_json::from_str(&line).expect("FTP fixture failed to start");
        Self {
            child,
            root,
            port: ready["port"].as_u64().unwrap() as u16,
            proxy_ports: ["http_proxy", "https_proxy", "socks_proxy"]
                .map(|name| ready[name].as_u64().unwrap() as u16),
        }
    }

    fn request(&self, name: &str) -> FtpRequest {
        FtpRequest::new(&format!("ftp://127.0.0.1:{}/{name}", self.port)).unwrap()
    }

    fn config(&self) -> FtpConfig {
        FtpConfig {
            ca_certificate: Some(self.root.path().join("ca.pem")),
            ..FtpConfig::default()
        }
    }

    fn events(&self) -> String {
        std::fs::read_to_string(self.root.path().join("events.jsonl")).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
        }
        for _ in 0..100 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn read_all(
    client: &FtpClient,
    request: FtpRequest,
    offset: u64,
) -> (Vec<u8>, swoop_ftp::Result<swoop_ftp::FtpMetadata>) {
    let mut download = client.download(request, offset).unwrap();
    let mut output = Vec::new();
    while let Some(chunk) = download.next_chunk().await {
        assert!(chunk.len() <= 16 * 1024);
        output.extend_from_slice(&chunk);
    }
    (output, download.finish().await)
}

#[tokio::test]
async fn passive_probe_download_resume_and_empty_file() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig {
        buffer_chunks: 1,
        ..FtpConfig::default()
    })
    .unwrap();
    let mut probe = client.probe(server.request("payload.bin")).unwrap();
    let metadata = probe.finish().await.unwrap();
    assert_eq!(metadata.size, Some(1024 * 1024));
    assert!(metadata.modified_unix.is_some());
    let expected = std::fs::read(server.root.path().join("payload.bin")).unwrap();
    let (body, result) = read_all(&client, server.request("payload.bin"), 0).await;
    result.unwrap();
    assert_eq!(body, expected);
    let (body, result) = read_all(&client, server.request("payload.bin"), 12345).await;
    assert_eq!(result.unwrap().size, Some(expected.len() as u64));
    assert_eq!(body, expected[12345..]);
    let (body, result) = read_all(
        &client,
        server.request("payload.bin"),
        expected.len() as u64,
    )
    .await;
    assert!(body.is_empty());
    assert_eq!(result.unwrap().size, Some(expected.len() as u64));
    let (body, result) = read_all(&client, server.request("empty.bin"), 0).await;
    result.unwrap();
    assert!(body.is_empty());
    let (body, result) = read_all(&client, server.request("中文%20空格.bin"), 0).await;
    result.unwrap();
    assert_eq!(body, b"unicode filename content");
    let events = server.events();
    assert!(events.contains("EPSV"));
    assert!(events.contains("REST"));
}

#[tokio::test]
async fn active_mode_downloads_using_configured_port_range() {
    let server = Server::start("plain");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = FtpClient::new(FtpConfig {
        data_mode: FtpDataMode::Active(ActiveMode {
            address: Some("127.0.0.1".parse().unwrap()),
            port_range: Some((port, port)),
        }),
        ..FtpConfig::default()
    })
    .unwrap();
    let (body, result) = read_all(&client, server.request("payload.bin"), 0).await;
    result.unwrap();
    assert_eq!(body.len(), 1024 * 1024);
    assert!(server.events().contains("EPRT") || server.events().contains("PORT"));
}

#[tokio::test]
async fn independent_credentials_fail_without_anonymous_fallback_and_errors_are_redacted() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let request = server
        .request("payload.bin")
        .with_credentials(FtpCredentials::new("fixture-user", "fixture-password").unwrap());
    read_all(&client, request, 0).await.1.unwrap();
    let request = server
        .request("payload.bin")
        .with_credentials(FtpCredentials::new("fixture-user", "incorrect-password").unwrap());
    let (body, result) = read_all(&client, request, 0).await;
    assert!(body.is_empty());
    let error = result.unwrap_err();
    assert!(matches!(error, Error::Transfer { status: 530, .. }));
    assert!(!error.to_string().contains("server-secret-leak"));
    assert!(!format!("{error:?}").contains("incorrect-password"));
}

#[tokio::test]
async fn full_payload_with_failed_final_response_is_not_success() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let (body, result) = read_all(&client, server.request("final-error.bin"), 0).await;
    assert_eq!(body.len(), 1024 * 1024);
    let error = result.unwrap_err();
    assert!(matches!(error, Error::Transfer { status: 426, .. }));
    assert!(!error.to_string().contains("server-secret-leak"));
}

#[tokio::test]
async fn truncated_payload_is_failure_even_if_server_says_success() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let (body, result) = read_all(&client, server.request("short.bin"), 0).await;
    assert_eq!(body, b"short");
    assert!(result.is_err());
}

#[tokio::test]
async fn cancel_joins_worker_while_bounded_queue_is_full() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig {
        buffer_chunks: 1,
        ..FtpConfig::default()
    })
    .unwrap();
    let mut download = client.download(server.request("payload.bin"), 0).unwrap();
    assert!(download.next_chunk().await.is_some());
    assert_eq!(download.finish().await.unwrap_err(), Error::NotDrained);
    tokio::time::sleep(Duration::from_millis(150)).await;
    tokio::time::timeout(Duration::from_millis(700), download.cancel())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(download.finish().await.unwrap_err(), Error::Cancelled);
    assert!(download.next_chunk().await.is_none());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !server.events().contains("disconnect") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancel_probe_joins_worker_while_waiting_for_server() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let mut probe = client.probe(server.request("delayed-probe.bin")).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !server.events().contains("probe-waiting") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // Dropping a pending finish future must leave its JoinHandle owned by Probe.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), probe.finish())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_millis(700), probe.cancel())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(probe.finish().await.unwrap_err(), Error::Cancelled);
}

#[tokio::test]
async fn consumer_backpressure_does_not_trigger_network_stall_timeout() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig {
        buffer_chunks: 1,
        read_timeout_secs: 1,
        ..FtpConfig::default()
    })
    .unwrap();
    let mut download = client.download(server.request("payload.bin"), 0).unwrap();
    let mut body = download.next_chunk().await.unwrap().to_vec();
    // Simulates a shared quota wait longer than the network read timeout.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    while let Some(chunk) = download.next_chunk().await {
        body.extend_from_slice(&chunk);
    }
    download.finish().await.unwrap();
    assert_eq!(
        body,
        std::fs::read(server.root.path().join("payload.bin")).unwrap()
    );
}

#[tokio::test]
async fn explicit_ftps_requires_valid_certificate_and_encrypts_data() {
    let server = Server::start("explicit");
    let request = server
        .request("payload.bin")
        .with_tls(FtpTlsMode::Explicit)
        .unwrap();
    let untrusted = FtpClient::new(FtpConfig::default()).unwrap();
    assert!(read_all(&untrusted, request.clone(), 0).await.1.is_err());
    let trusted = FtpClient::new(server.config()).unwrap();
    let (body, result) = read_all(&trusted, request, 0).await;
    result.unwrap();
    assert_eq!(body.len(), 1024 * 1024);
    assert!(server.events().contains("AUTH"));
    assert!(server.events().contains("PROT"));
}

#[tokio::test]
async fn implicit_ftps_has_encrypted_welcome_and_verified_data() {
    let server = Server::start("implicit");
    let client = FtpClient::new(server.config()).unwrap();
    let request =
        FtpRequest::new(&format!("ftps://127.0.0.1:{}/payload.bin", server.port)).unwrap();
    let (body, result) = read_all(&client, request, 0).await;
    result.unwrap();
    assert_eq!(body.len(), 1024 * 1024);
    assert!(!server.events().contains("AUTH"));
    assert!(server.events().contains("PROT"));
}

#[tokio::test]
async fn explicit_tls_never_falls_back_to_plaintext() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let request = server
        .request("payload.bin")
        .with_tls(FtpTlsMode::Explicit)
        .unwrap();
    assert!(read_all(&client, request, 0).await.1.is_err());
    assert!(!server.events().contains("RETR"));
    assert!(!server.events().contains("USER"));
}

#[tokio::test]
async fn matching_ca_does_not_disable_hostname_verification() {
    let server = Server::start("explicit-mismatch");
    let client = FtpClient::new(server.config()).unwrap();
    let request = server
        .request("payload.bin")
        .with_tls(FtpTlsMode::Explicit)
        .unwrap();
    assert!(read_all(&client, request, 0).await.1.is_err());
    assert!(!server.events().contains("USER"));
}

#[tokio::test]
async fn rest_rejection_does_not_silently_restart_or_deliver_wrong_offset() {
    let server = Server::start("no-rest");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let (body, result) = read_all(&client, server.request("payload.bin"), 1024).await;
    assert!(body.is_empty());
    assert!(result.is_err());
    assert!(!server.events().contains("RETR"));
}

#[tokio::test]
async fn pasv_fallback_ignores_server_supplied_third_party_address() {
    let server = Server::start("pasv");
    let client = FtpClient::new(FtpConfig::default()).unwrap();
    let (body, result) = read_all(&client, server.request("payload.bin"), 0).await;
    result.unwrap();
    assert_eq!(body.len(), 1024 * 1024);
    assert!(server.events().contains("EPSV"));
    assert!(server.events().contains("PASV"));
}

#[tokio::test]
async fn http_https_and_socks_proxies_tunnel_control_and_data() {
    let server = Server::start("plain");
    for (kind, index) in [
        (FtpProxyKind::HttpTunnel, 0),
        (FtpProxyKind::HttpsTunnel, 1),
        (FtpProxyKind::Socks5, 2),
        (FtpProxyKind::Socks5Hostname, 2),
    ] {
        let config = FtpConfig {
            proxy: Some(FtpProxy {
                kind,
                host: "127.0.0.1".into(),
                port: server.proxy_ports[index],
                bypass_hosts: Vec::new(),
            }),
            ..server.config()
        };
        let client = FtpClient::new(config).unwrap();
        let request =
            FtpRequest::new(&format!("ftp://localhost:{}/payload.bin", server.port)).unwrap();
        let (body, result) = read_all(&client, request, 0).await;
        result.unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        assert_eq!(body.len(), 1024 * 1024);
    }
    let events = server.events();
    assert!(events.matches("http-tunnel").count() >= 4);
    assert!(events.contains("socks-ip"));
    assert!(events.contains("socks-domain"));
}

#[tokio::test]
async fn proxy_exclusions_use_direct_connection() {
    let server = Server::start("plain");
    let config = FtpConfig {
        proxy: Some(FtpProxy {
            host: "127.0.0.1".into(),
            port: 1,
            bypass_hosts: vec!["127.0.0.1".into()],
            ..FtpProxy::default()
        }),
        ..FtpConfig::default()
    };
    let client = FtpClient::new(config).unwrap();
    read_all(&client, server.request("payload.bin"), 0)
        .await
        .1
        .unwrap();
    assert!(!server.events().contains("http-tunnel"));
}

#[tokio::test]
async fn configured_timeout_applies_to_probe_control_responses() {
    let server = Server::start("plain");
    let client = FtpClient::new(FtpConfig {
        read_timeout_secs: 1,
        ..FtpConfig::default()
    })
    .unwrap();
    let mut probe = client.probe(server.request("delayed-probe.bin")).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), probe.finish())
        .await
        .unwrap();
    assert!(matches!(result, Err(Error::Transfer { code: 28, .. })));
}
