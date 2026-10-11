//! Real loopback origins/proxies: no mocked reqwest or external credentials.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use swoop_media::{
    MediaAuthorization, MediaConfig, MediaDownloader, MediaError, MediaNetworkConfig, MediaProxyConfig,
    MediaSessionAuth, MediaSource, OriginCredentials, ScopedCookie,
};
use tokio_util::sync::CancellationToken;

const VOD: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n";
#[derive(Clone, Debug)]
struct Request {
    target: String,
    headers: Vec<(String, String)>,
}
impl Request {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.as_str())
    }
    fn path(&self) -> &str {
        &self.target
    }
}
struct Reply {
    code: u16,
    location: Option<String>,
    body: String,
}
impl Reply {
    fn ok(body: &str) -> Self {
        Self { code: 200, location: None, body: body.into() }
    }
    fn redirect(location: String) -> Self {
        Self { code: 302, location: Some(location), body: String::new() }
    }
}
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn read_request(stream: &mut TcpStream) -> Request {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let target = line.split_whitespace().nth(1).unwrap().to_owned();
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
            break;
        }
        let (name, value) = line.trim().split_once(':').unwrap();
        headers.push((name.into(), value.trim().into()));
    }
    Request { target, headers }
}
fn send_reply(stream: &mut TcpStream, reply: Reply) {
    let location = reply.location.map(|value| format!("Location: {value}\r\n")).unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",
        reply.code,
        reply.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
}
fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = Server {
        url: format!("http://{}", listener.local_addr().unwrap()),
        requests: Arc::new(Mutex::new(Vec::new())),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let (requests, stop, handler) = (server.requests.clone(), server.stop.clone(), Arc::new(handler));
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let (requests, handler) = (requests.clone(), handler.clone());
                    std::thread::spawn(move || {
                        let request = read_request(&mut stream);
                        requests.lock().unwrap().push(request.clone());
                        send_reply(&mut stream, handler(&request));
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(_) => break,
            }
        }
    });
    server
}
fn downloader(network: MediaNetworkConfig) -> MediaDownloader {
    MediaDownloader::new_with_network(MediaConfig::default(), network).unwrap()
}
fn bearer(origin: &str, token: &str) -> OriginCredentials {
    OriginCredentials::new(origin, Some(MediaAuthorization::Bearer(token.into())), vec![]).unwrap()
}

#[tokio::test]
async fn origins_cookie_paths_secure_expiry_and_redirect_chains_are_isolated() {
    let return_origin = Arc::new(Mutex::new(String::new()));
    let back = return_origin.clone();
    let other = serve(move |_| Reply::redirect(format!("{}/private/returned.m3u8", back.lock().unwrap())));
    let other_url = other.url.clone();
    let origin = serve(move |request| match request.path() {
        "/same.m3u8" => Reply::redirect("/private/same.m3u8".into()),
        "/cross.m3u8" => Reply::redirect(format!("{other_url}/bounce.m3u8")),
        "/master.m3u8" => Reply::ok(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nprivate/a.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2\nprivate2/b.m3u8\n",
        ),
        _ => Reply::ok(VOD),
    });
    *return_origin.lock().unwrap() = origin.url.clone();
    let cookies = vec![
        ScopedCookie {
            name: "path_cookie".into(),
            value: "test-only-cookie".into(),
            path: "/private".into(),
            secure: false,
            expires_at: None,
        },
        ScopedCookie {
            name: "secure_cookie".into(),
            value: "must-not-travel-over-http".into(),
            path: "/".into(),
            secure: true,
            expires_at: None,
        },
        ScopedCookie {
            name: "expired_cookie".into(),
            value: "expired".into(),
            path: "/".into(),
            secure: false,
            expires_at: Some(SystemTime::UNIX_EPOCH),
        },
    ];
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(&origin.url, Some(MediaAuthorization::Bearer("origin-a-test-token".into())), cookies)
            .unwrap(),
        bearer(&other.url, "origin-b-test-token"),
    ])
    .unwrap();
    let media = downloader(Default::default());
    for path in ["master.m3u8", "same.m3u8", "cross.m3u8"] {
        media
            .inspect_with_auth(&MediaSource::new(format!("{}/{path}", origin.url)), &auth, &CancellationToken::new())
            .await
            .unwrap();
    }
    let requests = origin.requests.lock().unwrap();
    for request in requests.iter() {
        let returned = request.path() == "/private/returned.m3u8";
        assert_eq!(request.header("authorization"), if returned { None } else { Some("Bearer origin-a-test-token") });
        let path_cookie = request.path().starts_with("/private/") && !returned;
        assert_eq!(request.header("cookie"), path_cookie.then_some("path_cookie=test-only-cookie"));
    }
    assert!(
        other
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.header("authorization").is_none() && request.header("cookie").is_none())
    );
}

#[tokio::test]
async fn basic_auth_is_explicit_and_secrets_never_enter_debug_or_public_headers() {
    let server = serve(|_| Reply::ok(VOD));
    let credentials = OriginCredentials::new(
        &server.url,
        Some(MediaAuthorization::Basic { username: "user".into(), password: "pass".into() }),
        vec![],
    )
    .unwrap();
    let auth = MediaSessionAuth::new(vec![credentials]).unwrap();
    assert!(!format!("{auth:?}").contains("user"));
    assert!(!format!("{auth:?}").contains("pass"));
    let media = downloader(Default::default());
    let mut source = MediaSource::new(format!("{}/vod.m3u8", server.url));
    media.inspect_with_auth(&source, &auth, &CancellationToken::new()).await.unwrap();
    assert_eq!(server.requests.lock().unwrap()[0].header("authorization"), Some("Basic dXNlcjpwYXNz"));
    source.headers.push(("Cookie".into(), "still=forbidden".into()));
    assert!(matches!(
        media.inspect_with_auth(&source, &auth, &CancellationToken::new()).await,
        Err(MediaError::UnsupportedHeader)
    ));
    assert!(
        OriginCredentials::new(&server.url, Some(MediaAuthorization::Bearer("x\r\ninjected: true".into())), vec![])
            .is_err()
    );
    assert!(OriginCredentials::new(&format!("{}/private", server.url), None, vec![]).is_err());
}

#[tokio::test]
async fn manual_http_proxy_and_bypass_use_real_servers() {
    let proxy = serve(|_| Reply::ok(VOD));
    let origin = serve(|_| Reply::ok(VOD));
    let media = downloader(MediaNetworkConfig {
        proxy: Some(MediaProxyConfig { url: proxy.url.clone(), bypass: vec!["127.0.0.1".into()] }),
        ..Default::default()
    });
    media.inspect(&MediaSource::new("http://media.invalid/vod.m3u8"), &CancellationToken::new()).await.unwrap();
    assert_eq!(proxy.requests.lock().unwrap()[0].target, "http://media.invalid/vod.m3u8");
    media.inspect(&MediaSource::new(format!("{}/vod.m3u8", origin.url)), &CancellationToken::new()).await.unwrap();
    assert_eq!(origin.requests.lock().unwrap().len(), 1);
    assert_eq!(proxy.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn socks5h_sends_hostname_to_proxy_instead_of_local_dns() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("socks5h://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut hello = [0; 2];
        stream.read_exact(&mut hello).unwrap();
        assert_eq!(hello[0], 5);
        let mut methods = vec![0; hello[1] as usize];
        stream.read_exact(&mut methods).unwrap();
        stream.write_all(&[5, 0]).unwrap();
        let mut command = [0; 4];
        stream.read_exact(&mut command).unwrap();
        assert_eq!(command, [5, 1, 0, 3]);
        let mut length = [0];
        stream.read_exact(&mut length).unwrap();
        let mut host = vec![0; length[0] as usize];
        stream.read_exact(&mut host).unwrap();
        assert_eq!(host, b"media.invalid");
        let mut port = [0; 2];
        stream.read_exact(&mut port).unwrap();
        assert_eq!(u16::from_be_bytes(port), 80);
        stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]).unwrap();
        let request = read_request(&mut stream);
        assert_eq!(request.target, "/vod.m3u8");
        send_reply(&mut stream, Reply::ok(VOD));
    });
    let media = downloader(MediaNetworkConfig {
        proxy: Some(MediaProxyConfig { url: proxy_url, bypass: vec![] }),
        ..Default::default()
    });
    media.inspect(&MediaSource::new("http://media.invalid/vod.m3u8"), &CancellationToken::new()).await.unwrap();
    worker.join().unwrap();
}

#[tokio::test]
async fn gateway_modes_inspect_dash_and_live_and_cancel_pending_requests() {
    let server = serve(|request| match request.path() {
        "/dash.mpd" => Reply::ok(
            "<MPD type=\"static\"><Period><AdaptationSet><Representation id=\"1\"><BaseURL>segment.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>",
        ),
        "/live.m3u8" => Reply::ok("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na.ts\n"),
        _ => {
            std::thread::sleep(Duration::from_millis(300));
            Reply::ok(VOD)
        }
    });
    let media = downloader(Default::default());
    for path in ["dash.mpd", "live.m3u8"] {
        assert!(
            media.inspect(&MediaSource::new(format!("{}/{path}", server.url)), &CancellationToken::new()).await.is_ok()
        );
    }
    let cancel = CancellationToken::new();
    let source = MediaSource::new(format!("{}/slow.m3u8", server.url));
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move { media.inspect(&source, &worker_cancel).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    assert!(matches!(worker.await.unwrap(), Err(MediaError::Cancelled)));
}

#[tokio::test]
async fn retry_budget_is_configurable_and_authentication_failures_are_not_retried() {
    use std::sync::atomic::AtomicUsize;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = serve(move |request| {
        if request.path() == "/private" {
            return Reply { code: 401, location: None, body: String::new() };
        }
        if count.fetch_add(1, Ordering::SeqCst) < 2 {
            return Reply { code: 503, location: None, body: String::new() };
        }
        Reply::ok(VOD)
    });
    let media = downloader(MediaNetworkConfig { request_retries: 2, retry_delay_ms: 1, ..Default::default() });
    media.inspect(&MediaSource::new(format!("{}/retry", server.url)), &CancellationToken::new()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(matches!(
        media.inspect(&MediaSource::new(format!("{}/private", server.url)), &CancellationToken::new()).await,
        Err(MediaError::HttpStatus(401))
    ));
    assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path() == "/private").count(), 1);
    let refusing = serve(|_| Reply { code: 503, location: None, body: String::new() });
    let media = downloader(MediaNetworkConfig { request_retries: 0, ..Default::default() });
    assert!(matches!(
        media.inspect(&MediaSource::new(refusing.url.clone()), &CancellationToken::new()).await,
        Err(MediaError::HttpStatus(503))
    ));
    assert_eq!(refusing.requests.lock().unwrap().len(), 1);
}
