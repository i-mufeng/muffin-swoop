use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    ChangedEtag,
    ChangedTotal,
    BadRange,
    IgnoreRange,
    Unsatisfied,
    Redirect,
    Weak,
    NoStore,
    Stall,
}
struct Origin {
    url: String,
    mode: Arc<Mutex<Mode>>,
    requests: Arc<Mutex<Vec<String>>>,
    cancel: CancellationToken,
    server: tokio::task::JoinHandle<()>,
    bytes: Vec<u8>,
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.server.abort();
    }
}
impl Origin {
    async fn start(mode: Mode) -> Self {
        Self::with_bytes(mode, (0..16384).map(|i| (i % 239) as u8).collect()).await
    }
    async fn with_bytes(mode: Mode, bytes: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(Mutex::new(mode));
        let cancel = CancellationToken::new();
        let (seen, setting, token, payload) = (requests.clone(), mode.clone(), cancel.clone(), bytes.clone());
        let count = Arc::new(AtomicUsize::new(0));
        let server = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let (seen,setting, payload,count) = (seen.clone(),setting.clone(),payload.clone(),count.clone());
                        clients.spawn(async move {
                            let mut request = Vec::new(); let mut byte = [0];
                            while !request.ends_with(b"\r\n\r\n") {
                                if socket.read(&mut byte).await.unwrap_or(0) == 0 { return; }
                                request.push(byte[0]); assert!(request.len() < 8192);
                            }
                            let request = String::from_utf8(request).unwrap();
                            if request.contains("/vod.m3u8 ") {
                                let body = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nmedia.ts\n#EXT-X-ENDLIST\n";
                                let _ = socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
                                return;
                            }
                            seen.lock().unwrap().push(request.clone());
                            let number = count.fetch_add(1, Ordering::SeqCst);
                            let mode = *setting.lock().unwrap();
                            let lower = request.to_ascii_lowercase();
                            let resume = lower.contains("\r\nif-range:");
                            if mode == Mode::Redirect && resume && !request.contains("/changed.ts ") {
                                let _ = socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /changed.ts\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await; return;
                            }
                            if mode == Mode::Unsatisfied && resume {
                                let _ = socket.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */16384\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await; return;
                            }
                            let mut payload = payload;
                            if mode == Mode::ChangedEtag && number > 0 { payload.fill(b'Z'); }
                            if mode == Mode::ChangedTotal && number > 0 { payload.push(b'!'); }
                            let etag = if mode == Mode::Weak { "W/\"v1\"" } else if mode == Mode::ChangedEtag && number > 0 { "\"v2\"" } else { "\"v1\"" };
                            if lower.contains(&format!("\r\nif-none-match: {etag}\r\n")) {
                                let _ = socket.write_all(format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nConnection: close\r\n\r\n").as_bytes()).await; return;
                            }
                            let range = lower.lines().find_map(|line| line.strip_prefix("range: bytes="))
                                .filter(|_| !(mode == Mode::IgnoreRange && resume));
                            let (start, end) = range.map(|v| {
                                let (start,end) = v.split_once('-').unwrap();
                                (start.parse::<usize>().unwrap(), if end.is_empty() { payload.len()-1 } else { end.parse::<usize>().unwrap().min(payload.len()-1) })
                            }).unwrap_or((0,payload.len()-1));
                            let body = &payload[start..=end];
                            let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
                            let cr = if range.is_some() {
                                let claimed_start = if mode == Mode::BadRange && resume { start-1 } else { start };
                                format!("Content-Range: bytes {claimed_start}-{end}/{}\r\n", payload.len())
                            } else { String::new() };
                            let cache_control = if mode == Mode::NoStore { "Cache-Control: no-store\r\n" } else { "" };
                            let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: video/mp2t\r\nETag: {etag}\r\n{cr}{cache_control}Connection: close\r\n\r\n",body.len());
                            let _ = socket.write_all(header.as_bytes()).await;
                            if number == 0 {
                                let _ = socket.write_all(&body[..2048]).await;
                                if mode == Mode::Stall { let _ = socket.read(&mut byte).await; }
                            } else { let _ = socket.write_all(body).await; }
                        });
                    },
                    _ = clients.join_next(), if !clients.is_empty() => {},
                }
            }
        });
        Self { url, mode, requests, cancel, server, bytes }
    }
    async fn gateway(&self, dir: &std::path::Path, enabled: bool) -> (Gateway, String) {
        self.gateway_auth(dir, enabled, Default::default()).await
    }
    async fn gateway_auth(&self, dir: &std::path::Path, enabled: bool, auth: MediaSessionAuth) -> (Gateway, String) {
        let config = Arc::new(MediaConfig::default());
        let network = crate::MediaNetworkConfig {
            resume_cache_dir: Some(dir.into()),
            resume_partial_segments: enabled,
            partial_flush_bytes: 16 * 1024,
            ..Default::default()
        };
        let transport = NetworkTransport::new(&network, config.network_timeout).unwrap();
        let mut inputs = vec![format!("{}/vod.m3u8", self.url)];
        let gateway =
            Gateway::start(transport, config, HeaderMap::new(), auth, &mut inputs, None, CancellationToken::new())
                .await
                .unwrap();
        let text = client().get(&inputs[0]).send().await.unwrap().text().await.unwrap();
        (gateway, text.lines().find(|line| line.starts_with("http:")).unwrap().into())
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
async fn fetch(url: &str, range: Option<&str>) -> reqwest::Response {
    let request = client().get(url);
    let request = if let Some(range) = range { request.header("range", range) } else { request };
    request.send().await.unwrap()
}

#[tokio::test]
async fn partial_restart_resumes_with_if_range_and_keeps_completed_cache() {
    let origin = Origin::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut first, url) = origin.gateway(dir.path(), true).await;
    assert_eq!(fetch(&url, None).await.status(), StatusCode::BAD_GATEWAY);
    assert!(first.failed());
    first.shutdown().await;
    let (mut second, url) = origin.gateway(dir.path(), true).await;
    let response = fetch(&url, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.content_length(), Some(origin.bytes.len() as u64));
    assert_eq!(response.bytes().await.unwrap(), origin.bytes);
    assert_eq!(fetch(&url, None).await.bytes().await.unwrap(), origin.bytes);
    assert!(!second.failed());
    second.shutdown().await;
    let requests = origin.requests.lock().unwrap();
    assert!(requests[1].to_ascii_lowercase().contains("range: bytes=2048-16383\r\n"));
    assert!(requests[1].to_ascii_lowercase().contains("if-range: \"v1\"\r\n"));
    assert!(requests[2].to_ascii_lowercase().contains("if-none-match: \"v1\"\r\n"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("swoop-media-cache-v1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|f| f.path().extension().is_some_and(|v| v == "part"))
            .count(),
        0
    );
}

#[tokio::test]
async fn resumed_byterange_uses_absolute_offset_and_original_response_span() {
    let origin = Origin::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut first, url) = origin.gateway(dir.path(), true).await;
    assert_eq!(fetch(&url, Some("bytes=1000-8999")).await.status(), StatusCode::BAD_GATEWAY);
    first.shutdown().await;
    let (mut second, url) = origin.gateway(dir.path(), true).await;
    let response = fetch(&url, Some("bytes=1000-8999")).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-range"], "bytes 1000-8999/16384");
    assert_eq!(response.bytes().await.unwrap(), &origin.bytes[1000..9000]);
    second.shutdown().await;
    assert!(origin.requests.lock().unwrap()[1].contains("range: bytes=3048-8999\r\n"));
}

#[tokio::test]
async fn ignored_range_416_or_changed_validator_length_range_and_redirect_restart_safely() {
    for mode in
        [Mode::IgnoreRange, Mode::Unsatisfied, Mode::ChangedEtag, Mode::ChangedTotal, Mode::BadRange, Mode::Redirect]
    {
        let origin = Origin::start(mode).await;
        let dir = tempfile::tempdir().unwrap();
        let (mut first, url) = origin.gateway(dir.path(), true).await;
        assert_eq!(fetch(&url, None).await.status(), StatusCode::BAD_GATEWAY);
        first.shutdown().await;
        let (mut second, url) = origin.gateway(dir.path(), true).await;
        let response = fetch(&url, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut expected = origin.bytes.clone();
        if mode == Mode::ChangedEtag {
            expected.fill(b'Z');
        }
        if mode == Mode::ChangedTotal {
            expected.push(b'!');
        }
        assert_eq!(response.bytes().await.unwrap(), expected);
        second.shutdown().await;
        let requests = origin.requests.lock().unwrap();
        assert!(requests[1].contains("if-range: \"v1\""));
        assert!(!requests.last().unwrap().contains("if-range:"));
        assert!(!requests.last().unwrap().contains("range:"));
    }
}

#[tokio::test]
async fn cancellation_preserves_committed_prefix_and_releases_resource_lock() {
    let origin = Origin::start(Mode::Stall).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut first, url) = origin.gateway(dir.path(), true).await;
    let pending = tokio::spawn(async move { client().get(&url).send().await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let persisted = std::fs::read_dir(dir.path().join("swoop-media-cache-v1")).ok().is_some_and(|files| {
                files.filter_map(Result::ok).any(|f| f.file_name().to_string_lossy().ends_with(".partial.json"))
            });
            if persisted {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), first.shutdown()).await.unwrap();
    pending.abort();
    let _ = pending.await;
    *origin.mode.lock().unwrap() = Mode::Normal;
    let (mut second, url) = origin.gateway(dir.path(), true).await;
    assert_eq!(fetch(&url, None).await.bytes().await.unwrap(), origin.bytes);
    second.shutdown().await;
    assert!(origin.requests.lock().unwrap()[1].contains("range: bytes=2048-16383"));
}

#[tokio::test]
async fn disabled_weak_no_store_and_corrupt_prefixes_never_resume() {
    for (mode, enabled, corrupt) in [
        (Mode::Normal, false, false),
        (Mode::Weak, true, false),
        (Mode::NoStore, true, false),
        (Mode::Normal, true, true),
    ] {
        let origin = Origin::start(mode).await;
        let dir = tempfile::tempdir().unwrap();
        let (mut first, url) = origin.gateway(dir.path(), enabled).await;
        let response = fetch(&url, None).await;
        // Non-cacheable streams fail in the response body rather than before headers.
        if response.status() == StatusCode::OK {
            assert!(response.bytes().await.is_err());
        } else {
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        }
        first.shutdown().await;
        if corrupt {
            for file in std::fs::read_dir(dir.path().join("swoop-media-cache-v1")).unwrap().filter_map(Result::ok) {
                if file.path().extension().is_some_and(|v| v == "part") {
                    std::fs::write(file.path(), b"corrupt").unwrap();
                }
            }
        }
        let (mut second, url) = origin.gateway(dir.path(), enabled).await;
        assert_eq!(fetch(&url, None).await.bytes().await.unwrap(), origin.bytes);
        second.shutdown().await;
        assert!(!origin.requests.lock().unwrap()[1].contains("if-range:"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real ffmpeg and ffprobe executables"]
async fn real_ffmpeg_cancel_restart_resumes_inside_segment_and_decodes_complete_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("fixture.ts");
    let generated = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=12",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:sample_rate=44100",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "12",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
        ])
        .arg(&source)
        .output()
        .expect("real FFmpeg is required");
    assert!(generated.status.success(), "{}", String::from_utf8_lossy(&generated.stderr));
    let origin = Origin::with_bytes(Mode::Stall, std::fs::read(&source).unwrap()).await;
    let cache_dir = dir.path().join("cache");
    let network = crate::MediaNetworkConfig {
        resume_cache_dir: Some(cache_dir.clone()),
        partial_flush_bytes: 16384,
        ..Default::default()
    };
    let request = crate::MediaRequest {
        source: crate::MediaSource::new(format!("{}/vod.m3u8", origin.url)),
        destination: dir.path().join("restored.mp4"),
        duration_limit: None,
        selection: Default::default(),
        overwrite: false,
    };
    let downloader = crate::MediaDownloader::new_with_network(MediaConfig::default(), network.clone()).unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let first_request = request.clone();
    let first = tokio::spawn(async move { downloader.download(first_request, token, |_| {}).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if std::fs::read_dir(cache_dir.join("swoop-media-cache-v1")).ok().is_some_and(|files| {
                files.filter_map(Result::ok).any(|f| f.file_name().to_string_lossy().ends_with(".partial.json"))
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real FFmpeg must start a media request and persist its prefix");
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), first).await.unwrap().unwrap(),
        Err(MediaError::Cancelled)
    ));
    assert!(!request.destination.exists());
    *origin.mode.lock().unwrap() = Mode::Normal;
    let restarted = crate::MediaDownloader::new_with_network(MediaConfig::default(), network).unwrap();
    let result = restarted.download(request.clone(), CancellationToken::new(), |_| {}).await.unwrap();
    assert!(result.bytes > 0);
    let requests = origin.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .skip(1)
            .any(|request| request.contains("if-range: \"v1\"") && request.contains("range: bytes=2048-")),
        "{requests:?}"
    );
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type:format=duration", "-of", "json"])
        .arg(&request.destination)
        .output()
        .expect("real FFprobe is required");
    assert!(probe.status.success(), "{}", String::from_utf8_lossy(&probe.stderr));
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = value["streams"].as_array().unwrap();
    assert!(streams.iter().any(|v| v["codec_type"] == "video"));
    assert!(streams.iter().any(|v| v["codec_type"] == "audio"));
    assert!(value["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap() >= 1.9);
    let decoded = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&request.destination)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
    assert!(decoded.stderr.is_empty(), "{}", String::from_utf8_lossy(&decoded.stderr));
}

#[tokio::test]
async fn concurrent_gateways_serialize_resume_and_completed_validation() {
    let origin = Origin::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut first, url) = origin.gateway(dir.path(), true).await;
    assert_eq!(fetch(&url, None).await.status(), StatusCode::BAD_GATEWAY);
    first.shutdown().await;
    let (mut second, a) = origin.gateway(dir.path(), true).await;
    let (mut third, b) = origin.gateway(dir.path(), true).await;
    let (a, b) = tokio::join!(fetch(&a, None), fetch(&b, None));
    assert_eq!(a.bytes().await.unwrap(), origin.bytes);
    assert_eq!(b.bytes().await.unwrap(), origin.bytes);
    second.shutdown().await;
    third.shutdown().await;
    let requests = origin.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.contains("if-range:")).count(), 1);
    assert_eq!(requests.iter().filter(|r| r.contains("if-none-match:")).count(), 1);
}

#[tokio::test]
async fn authenticated_clones_resume_but_new_sessions_do_not_reuse_other_credentials_cache() {
    let origin = Origin::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let auth = |token: &str| {
        MediaSessionAuth::new(vec![
            crate::OriginCredentials::new(&origin.url, Some(crate::MediaAuthorization::Bearer(token.into())), vec![])
                .unwrap(),
        ])
        .unwrap()
    };
    let session = auth("token-A");
    let (mut first, url) = origin.gateway_auth(dir.path(), true, session.clone()).await;
    assert_eq!(fetch(&url, None).await.status(), StatusCode::BAD_GATEWAY);
    first.shutdown().await;
    let (mut second, url) = origin.gateway_auth(dir.path(), true, session).await;
    assert_eq!(fetch(&url, None).await.bytes().await.unwrap(), origin.bytes);
    second.shutdown().await;
    let (mut third, url) = origin.gateway_auth(dir.path(), true, auth("token-B")).await;
    assert_eq!(fetch(&url, None).await.bytes().await.unwrap(), origin.bytes);
    third.shutdown().await;
    let requests = origin.requests.lock().unwrap();
    assert!(requests[1].contains("authorization: Bearer token-A"));
    assert!(requests[1].contains("if-range:"));
    assert!(requests[2].contains("authorization: Bearer token-B"));
    assert!(!requests[2].contains("if-range:"));
    assert!(!requests[2].contains("if-none-match:"));
    for file in std::fs::read_dir(dir.path().join("swoop-media-cache-v1")).unwrap().filter_map(Result::ok) {
        if file.path().extension().is_some_and(|v| v == "json") {
            let text = std::fs::read_to_string(file.path()).unwrap();
            assert!(!text.contains("token-"));
            assert!(!text.contains(&origin.url));
        }
    }
}

#[tokio::test]
async fn pinned_live_manifest_is_consumed_by_one_get_then_refreshes_normally() {
    let origin = Origin::start(Mode::Normal).await;
    let config = Arc::new(MediaConfig::default());
    let transport = NetworkTransport::new(&Default::default(),config.network_timeout).unwrap();
    let upstream = format!("{}/vod.m3u8",origin.url); let mut inputs = vec![upstream.clone()];
    let mut gateway = Gateway::start(transport,config,HeaderMap::new(),Default::default(),&mut inputs,None,CancellationToken::new()).await.unwrap();
    gateway.pin_hls_manifest(&upstream,"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:42\n#EXTINF:1,\nmedia.ts\n").unwrap();
    assert_eq!(client().head(&inputs[0]).send().await.unwrap().status(),StatusCode::OK);
    let pinned = fetch(&inputs[0],None).await.text().await.unwrap();
    assert!(pinned.contains("#EXT-X-MEDIA-SEQUENCE:42")); assert!(!pinned.contains(&origin.url));
    let refreshed = fetch(&inputs[0],None).await.text().await.unwrap();
    assert!(!refreshed.contains("#EXT-X-MEDIA-SEQUENCE:42")); assert!(refreshed.contains("#EXT-X-ENDLIST"));
    gateway.shutdown().await;
}
