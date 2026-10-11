//! 浏览器桥接：只绑 127.0.0.1 的本地 HTTP 服务，接收浏览器扩展交来的下载。
//!
//! 协议与安全边界见 `docs/browser-extension.md`。扩展只看 HTTP 状态：200 表示 Swoop 已接手，
//! 浏览器取消自己的下载；其它一律回退给浏览器继续。

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HOST, HeaderValue, ORIGIN};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::json;
use swoop_core::{AddOptions, Engine};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_PORT: u16 = 51230;
const MAX_BODY: usize = 64 * 1024;
/// 接管前的探测上限；扩展那边在等，超时就让浏览器自己下
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// 单连接总时长上限（含探测）
const CONN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BridgeMode {
    /// 探测成功直接加入队列，弹吐司
    #[default]
    Silent,
    /// 探测成功后弹出预填好的新建对话框
    Ask,
}

impl BridgeMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "silent" => Some(Self::Silent),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Ask => "ask",
        }
    }
}

pub enum BridgeEvent {
    /// 静默模式：任务已加入引擎
    Added { file_name: String },
    /// 询问模式：探测成功，等 UI 弹新建对话框
    Ask { url: String, file_name: String, size: Option<u64>, headers: Vec<(String, String)> },
}

/// UI 线程与服务共享的状态
struct Shared {
    engine: Arc<Engine>,
    mode: Mutex<BridgeMode>,
    dir: Mutex<PathBuf>,
    last_ping: Mutex<Option<SystemTime>>,
    events: mpsc::Sender<BridgeEvent>,
    /// 有新事件时唤醒 UI（egui 空闲时不会自己重绘）
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn emit(&self, ev: BridgeEvent) {
        let _ = self.events.send(ev);
        (self.wake)();
    }
}

struct Running {
    port: u16,
    cancel: CancellationToken,
}

pub struct Bridge {
    handle: Handle,
    shared: Arc<Shared>,
    events: mpsc::Receiver<BridgeEvent>,
    running: Option<Running>,
}

impl Bridge {
    pub fn new(
        handle: Handle,
        engine: Arc<Engine>,
        mode: BridgeMode,
        dir: PathBuf,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            engine,
            mode: Mutex::new(mode),
            dir: Mutex::new(dir),
            last_ping: Mutex::new(None),
            events: tx,
            wake: Box::new(wake),
        });
        Self { handle, shared, events: rx, running: None }
    }

    /// 在指定端口启动（已在运行则先停掉），返回实际端口；端口被占用等错误返回可读文本。
    /// 绑定在调用线程同步完成，所以失败能立刻反馈给 UI。
    pub fn start(&mut self, port: u16) -> Result<u16, String> {
        self.stop();
        let std_listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse { format!("端口 {port} 已被占用") } else { format!("端口 {port}：{e}") }
        })?;
        std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
        let listener = {
            let _guard = self.handle.enter();
            tokio::net::TcpListener::from_std(std_listener).map_err(|e| e.to_string())?
        };
        let cancel = CancellationToken::new();
        self.handle.spawn(serve(listener, port, self.shared.clone(), cancel.clone()));
        tracing::info!(port, "浏览器桥接已启动");
        self.running = Some(Running { port, cancel });
        Ok(port)
    }

    pub fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.cancel();
            tracing::info!(port = r.port, "浏览器桥接已停止");
        }
    }

    pub fn port(&self) -> Option<u16> {
        self.running.as_ref().map(|r| r.port)
    }

    pub fn set_mode(&self, mode: BridgeMode) {
        *self.shared.mode.lock().unwrap() = mode;
    }

    pub fn set_dir(&self, dir: PathBuf) {
        *self.shared.dir.lock().unwrap() = dir;
    }

    /// 最近一次扩展心跳的时间
    pub fn last_ping(&self) -> Option<SystemTime> {
        *self.shared.last_ping.lock().unwrap()
    }

    pub fn try_recv(&self) -> Option<BridgeEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn serve(listener: tokio::net::TcpListener, port: u16, shared: Arc<Shared>, cancel: CancellationToken) {
    loop {
        let stream = tokio::select! {
            _ = cancel.cancelled() => return,
            r = listener.accept() => match r {
                Ok((stream, _)) => stream,
                Err(e) => {
                    // 文件描述符耗尽等：稍等再试，避免空转
                    tracing::warn!("桥接 accept 失败: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            },
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| handle(shared.clone(), port, req));
            let conn = http1::Builder::new().serve_connection(TokioIo::new(stream), svc);
            let _ = tokio::time::timeout(CONN_TIMEOUT, conn).await;
        });
    }
}

/// 防 DNS rebinding 与网页跨站调用：Host 必须是回环名 + 本端口；
/// Origin 只允许缺省（本机脚本）或扩展页，网页的 http(s) / null 来源一律拒绝。
fn authorize(host: Option<&str>, origin: Option<&str>, port: u16) -> Result<(), &'static str> {
    let host = host.ok_or("缺少 Host")?;
    let (name, host_port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, p.parse::<u16>().ok()),
        None => (host, Some(80)),
    };
    if !matches!(name, "127.0.0.1" | "localhost") || host_port != Some(port) {
        return Err("Host 不是本机地址");
    }
    match origin {
        None => Ok(()),
        Some(o) if o.starts_with("chrome-extension://") || o.starts_with("moz-extension://") => Ok(()),
        Some(_) => Err("不接受网页来源的请求"),
    }
}

async fn handle(shared: Arc<Shared>, port: u16, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let header = |name| req.headers().get(name).and_then(|v: &HeaderValue| v.to_str().ok());
    if let Err(reason) = authorize(header(HOST), header(ORIGIN), port) {
        tracing::warn!(host = ?header(HOST), origin = ?header(ORIGIN), "拒绝桥接请求: {reason}");
        return Ok(error(StatusCode::FORBIDDEN, reason));
    }
    let resp = match (req.method(), req.uri().path()) {
        (&Method::GET, "/ping") => {
            *shared.last_ping.lock().unwrap() = Some(SystemTime::now());
            reply(StatusCode::OK, json!({ "app": "swoop", "version": env!("CARGO_PKG_VERSION") }))
        }
        (&Method::POST, "/tasks") => post_task(&shared, req).await,
        (_, "/ping" | "/tasks") => error(StatusCode::METHOD_NOT_ALLOWED, "不支持的方法"),
        _ => error(StatusCode::NOT_FOUND, "未知路径"),
    };
    Ok(resp)
}

#[derive(Deserialize)]
struct TaskRequest {
    url: String,
    #[serde(default)]
    referrer: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    /// 浏览器看到的 MIME 与大小（来自 downloads 事件）；有就用来核对重新探测的结果
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    // suggested_name：协议里有，但 onCreated 时浏览器通常还没定文件名，v0.3 以探测结果为准
}

/// 浏览器那次下载可能是 POST 表单或一次性链接，Swoop 用 GET 重新请求拿到的未必是同一个东西。
/// 拿到网页而浏览器下的不是网页、或大小对不上，就判定不是同一份文件，交回浏览器。
fn mismatch(expected_mime: Option<&str>, expected_size: Option<u64>, info: &swoop_core::ProbeInfo) -> Option<String> {
    let got_html = info.content_type.as_deref().is_some_and(|m| m == "text/html");
    let want_html = expected_mime.is_some_and(|m| m.to_ascii_lowercase().starts_with("text/html"));
    if expected_mime.is_some_and(|m| !m.is_empty()) && got_html && !want_html {
        return Some("链接返回的是网页而不是文件（可能需要登录或是表单提交）".into());
    }
    match (expected_size.filter(|s| *s > 0), info.size) {
        (Some(want), Some(got)) if want != got => Some(format!("文件大小不一致：浏览器 {want} 字节，Swoop 探测到 {got} 字节")),
        _ => None,
    }
}

async fn post_task(shared: &Shared, req: Request<Incoming>) -> Response<Full<Bytes>> {
    // 要求 JSON：网页的"简单请求"（text/plain 表单）无法带这个类型，多一道防线
    let is_json = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "需要 application/json");
    }
    let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "请求体过大或读取失败"),
    };
    let task: TaskRequest = match serde_json::from_slice(&body) {
        Ok(t) => t,
        Err(e) => return error(StatusCode::BAD_REQUEST, &format!("请求体无效：{e}")),
    };

    let mut headers = task.headers;
    if let Some(r) = task.referrer.filter(|r| !r.is_empty())
        && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("referer"))
    {
        headers.push(("Referer".into(), r));
    }

    let info = match tokio::time::timeout(PROBE_TIMEOUT, shared.engine.probe_url(&task.url, &headers)).await {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => return error(StatusCode::UNPROCESSABLE_ENTITY, &format!("探测失败：{e}")),
        Err(_) => return error(StatusCode::UNPROCESSABLE_ENTITY, "探测超时"),
    };
    if let Some(reason) = mismatch(task.mime.as_deref(), task.size, &info) {
        tracing::info!(url = %task.url, "不接管: {reason}");
        return error(StatusCode::UNPROCESSABLE_ENTITY, &reason);
    }
    tracing::info!(url = %task.url, name = %info.file_name, size = ?info.size, "收到浏览器下载");

    let mode = *shared.mode.lock().unwrap();
    match mode {
        BridgeMode::Silent => {
            let dir = shared.dir.lock().unwrap().clone();
            match shared.engine.add(AddOptions { url: task.url, dir, file_name: None, headers }) {
                Ok(id) => {
                    shared.emit(BridgeEvent::Added { file_name: info.file_name.clone() });
                    reply(StatusCode::OK, json!({ "id": id.0, "file_name": info.file_name, "size": info.size }))
                }
                Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()),
            }
        }
        BridgeMode::Ask => {
            let resp = json!({ "id": null, "file_name": info.file_name, "size": info.size });
            shared.emit(BridgeEvent::Ask { url: task.url, file_name: info.file_name, size: info.size, headers });
            reply(StatusCode::OK, resp)
        }
    }
}

fn reply(status: StatusCode, body: serde_json::Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("静态响应头")
}

fn error(status: StatusCode, msg: &str) -> Response<Full<Bytes>> {
    reply(status, json!({ "error": msg }))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    use swoop_core::{Config, TaskStatus};

    use super::*;

    #[test]
    fn authorize_rules() {
        let p = 51230;
        assert!(authorize(Some("127.0.0.1:51230"), None, p).is_ok());
        assert!(authorize(Some("localhost:51230"), Some("chrome-extension://abc"), p).is_ok());
        assert!(authorize(Some("localhost:51230"), Some("moz-extension://abc"), p).is_ok());
        assert!(authorize(None, None, p).is_err());
        assert!(authorize(Some("evil.com:51230"), None, p).is_err(), "DNS rebinding");
        assert!(authorize(Some("127.0.0.1:8080"), None, p).is_err());
        assert!(authorize(Some("127.0.0.1"), None, p).is_err());
        assert!(authorize(Some("127.0.0.1:51230"), Some("https://evil.com"), p).is_err());
        assert!(authorize(Some("127.0.0.1:51230"), Some("null"), p).is_err());
    }

    #[test]
    fn mismatch_rules() {
        let info = |ct: Option<&str>, size: Option<u64>| swoop_core::ProbeInfo {
            final_url: "http://a/b".parse().unwrap(),
            file_name: "b".into(),
            size,
            resumable: false,
            etag: None,
            last_modified: None,
            content_type: ct.map(str::to_string),
        };
        let html = info(Some("text/html"), Some(100));
        assert!(mismatch(Some("application/pdf"), None, &html).is_some());
        assert!(mismatch(Some("text/html; charset=utf-8"), None, &html).is_none(), "本来就是网页");
        assert!(mismatch(None, None, &html).is_none(), "右键菜单没有 MIME，信任用户");
        assert!(mismatch(Some("application/zip"), Some(99), &info(Some("application/zip"), Some(100))).is_some());
        assert!(mismatch(Some("application/zip"), Some(100), &info(Some("application/zip"), Some(100))).is_none());
        assert!(mismatch(Some("application/zip"), Some(100), &info(None, None)).is_none(), "大小未知不拦");
        assert!(mismatch(Some(""), Some(0), &html).is_none());
    }

    #[test]
    fn mode_round_trip() {
        for m in [BridgeMode::Silent, BridgeMode::Ask] {
            assert_eq!(BridgeMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(BridgeMode::parse("x"), None);
    }

    /// 只认 `cookie: token=1` 的最小文件服务器，返回文件 URL
    fn file_server(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f/report.pdf", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let (mut line, mut authed) = (String::new(), false);
                    while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                        authed |= line.eq_ignore_ascii_case("cookie: token=1\r\n");
                        line.clear();
                    }
                    let head = if authed {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    } else {
                        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
                    };
                    let _ = stream.write_all(head.as_bytes());
                    if authed {
                        let _ = stream.write_all(body);
                    }
                });
            }
        });
        url
    }

    struct Fixture {
        rt: tokio::runtime::Runtime,
        engine: Arc<Engine>,
        bridge: Bridge,
        base: String,
        dir: tempfile::TempDir,
    }

    fn fixture(mode: BridgeMode) -> Fixture {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = Arc::new(Engine::new(rt.handle().clone(), Config::default()));
        let dir = tempfile::tempdir().unwrap();
        let mut bridge = Bridge::new(rt.handle().clone(), engine.clone(), mode, dir.path().to_path_buf(), || {});
        let port = bridge.start(0).unwrap();
        Fixture { rt, engine, bridge, base: format!("http://127.0.0.1:{port}"), dir }
    }

    fn post(f: &Fixture, body: serde_json::Value, origin: Option<&str>) -> (u16, serde_json::Value) {
        f.rt.block_on(async {
            let mut req = reqwest::Client::new()
                .post(format!("{}/tasks", f.base))
                .header("content-type", "application/json")
                .body(body.to_string());
            if let Some(o) = origin {
                req = req.header("origin", o);
            }
            let resp = req.send().await.unwrap();
            let status = resp.status().as_u16();
            (status, serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap_or_default())
        })
    }

    #[test]
    fn ping_records_heartbeat() {
        let f = fixture(BridgeMode::Silent);
        assert!(f.bridge.last_ping().is_none());
        let bytes = f.rt.block_on(async { reqwest::get(format!("{}/ping", f.base)).await.unwrap().bytes().await.unwrap() });
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["app"], "swoop");
        assert!(f.bridge.last_ping().is_some());
    }

    #[test]
    fn silent_mode_adds_task_with_cookie() {
        static BODY: &[u8] = b"%PDF-1.7 hello swoop";
        let url = file_server(BODY);
        let f = fixture(BridgeMode::Silent);

        // 没带 Cookie：探测 403 → 422，浏览器应回退
        let (status, body) = post(&f, json!({ "url": url }), Some("chrome-extension://abc"));
        assert_eq!(status, 422, "{body}");
        assert!(body["error"].as_str().unwrap().contains("403"));
        assert!(f.engine.snapshot().is_empty());

        let (status, body) = post(
            &f,
            json!({ "url": url, "referrer": "https://example.com/", "headers": [["Cookie", "token=1"]] }),
            Some("chrome-extension://abc"),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["file_name"], "report.pdf");
        assert_eq!(body["size"], BODY.len());
        assert!(matches!(f.bridge.try_recv(), Some(BridgeEvent::Added { file_name }) if file_name == "report.pdf"));

        let id = swoop_core::TaskId(body["id"].as_u64().unwrap());
        let t0 = Instant::now();
        while f.engine.snapshot_one(id).unwrap().status != TaskStatus::Completed {
            assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", f.engine.snapshot_one(id).unwrap().status);
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(std::fs::read(f.dir.path().join("report.pdf")).unwrap(), BODY);
    }

    #[test]
    fn ask_mode_hands_probe_to_ui() {
        static BODY: &[u8] = b"0123456789";
        let url = file_server(BODY);
        let f = fixture(BridgeMode::Ask);
        let (status, body) = post(&f, json!({ "url": url, "headers": [["Cookie", "token=1"]] }), None);
        assert_eq!(status, 200, "{body}");
        assert!(body["id"].is_null());
        match f.bridge.try_recv() {
            Some(BridgeEvent::Ask { url: u, file_name, size, headers }) => {
                assert_eq!(u, url);
                assert_eq!(file_name, "report.pdf");
                assert_eq!(size, Some(10));
                assert_eq!(headers, vec![("Cookie".to_string(), "token=1".to_string())]);
            }
            _ => panic!("应收到 Ask 事件"),
        }
        assert!(f.engine.snapshot().is_empty(), "询问模式不应直接建任务");
    }

    #[test]
    fn rejects_web_origin_and_bad_requests() {
        let f = fixture(BridgeMode::Silent);
        let (status, _) = post(&f, json!({ "url": "http://127.0.0.1:1/x" }), Some("https://evil.com"));
        assert_eq!(status, 403);
        let (status, _) = post(&f, json!({ "nope": 1 }), None);
        assert_eq!(status, 400);
        let (status, _) = post(&f, json!({ "url": "ftp://example.com/a" }), None);
        assert_eq!(status, 422);

        let status = f.rt.block_on(async {
            let client = reqwest::Client::new();
            let text_plain = client
                .post(format!("{}/tasks", f.base))
                .header("content-type", "text/plain")
                .body(r#"{"url":"http://a/b"}"#)
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(text_plain, 415);
            let huge = client
                .post(format!("{}/tasks", f.base))
                .header("content-type", "application/json")
                .body(vec![b' '; MAX_BODY + 1])
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(huge, 413);
            client.get(format!("{}/nope", f.base)).send().await.unwrap().status()
        });
        assert_eq!(status, 404);
    }

    #[test]
    fn port_in_use_and_restart() {
        let f = fixture(BridgeMode::Silent);
        let port = f.bridge.port().unwrap();
        let mut other = Bridge::new(f.rt.handle().clone(), f.engine.clone(), BridgeMode::Silent, PathBuf::new(), || {});
        let err = other.start(port).unwrap_err();
        assert!(err.contains("占用"), "{err}");
        assert!(other.port().is_none());

        // 停掉后端口释放，可以重新绑定
        let mut f = f;
        f.bridge.stop();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(other.start(port).unwrap(), port);
    }
}
