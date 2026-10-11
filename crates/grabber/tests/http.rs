use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use swoop_grabber::{Grabber, GrabberConfig, PageState, ProjectStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ResponseGate {
    started: CancellationToken,
    release: CancellationToken,
    disconnected: CancellationToken,
}
impl ResponseGate {
    fn new() -> Self {
        Self {
            started: CancellationToken::new(),
            release: CancellationToken::new(),
            disconnected: CancellationToken::new(),
        }
    }
}

#[derive(Clone)]
struct Reply {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    location: Option<String>,
    slow: bool,
    gate: Option<ResponseGate>,
}
impl Reply {
    fn html(body: &str) -> Self {
        Self::data("text/html", body.as_bytes())
    }
    fn data(mime: &'static str, body: &[u8]) -> Self {
        Self {
            status: 200,
            mime,
            body: body.into(),
            location: None,
            slow: false,
            gate: None,
        }
    }
    fn redirect(url: String) -> Self {
        Self {
            status: 302,
            mime: "text/plain",
            body: vec![],
            location: Some(url),
            slow: false,
            gate: None,
        }
    }
}
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    peak: Arc<AtomicUsize>,
    stop: CancellationToken,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
async fn serve(mut routes: HashMap<String, Reply>) -> Server {
    routes.entry("/robots.txt".into()).or_insert(Reply {
        status: 404,
        ..Reply::data("text/plain", b"")
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let routes = Arc::new(routes);
    let (log, active2, peak2, stop2) = (requests.clone(), active, peak.clone(), stop.clone());
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! { _ = stop2.cancelled() => break, result = listener.accept() => result };
            let Ok((mut socket, _)) = accepted else {
                break;
            };
            let (routes, log, active, peak) =
                (routes.clone(), log.clone(), active2.clone(), peak2.clone());
            let connection_stop = stop2.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut block = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let Ok(read) = socket.read(&mut block).await else {
                        return;
                    };
                    if read == 0 || request.len() > 65536 {
                        return;
                    }
                    request.extend_from_slice(&block[..read]);
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let path = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_string();
                log.lock().unwrap().push(request);
                let reply = routes.get(&path).cloned().unwrap_or(Reply {
                    status: 404,
                    ..Reply::data("text/plain", b"missing")
                });
                let running = active.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(running, Ordering::AcqRel);
                let location = reply
                    .location
                    .as_ref()
                    .map(|url| format!("Location: {url}\r\n"))
                    .unwrap_or_default();
                let headers = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                    reply.status,
                    reply.mime,
                    reply.body.len(),
                    location
                );
                let _ = socket.write_all(headers.as_bytes()).await;
                let mut remaining = reply.body.as_slice();
                if let Some(gate) = reply.gate {
                    let prefix = remaining.len().min(128);
                    if socket.write_all(&remaining[..prefix]).await.is_err() {
                        active.fetch_sub(1, Ordering::AcqRel);
                        return;
                    }
                    remaining = &remaining[prefix..];
                    gate.started.cancel();
                    let mut byte = [0; 1];
                    let closed = tokio::select! {
                        _ = connection_stop.cancelled() => true,
                        _ = gate.release.cancelled() => false,
                        result = socket.read(&mut byte) => {
                            assert!(!matches!(result, Ok(1..)), "unexpected pipelined request");
                            gate.disconnected.cancel();
                            true
                        }
                    };
                    if closed {
                        active.fetch_sub(1, Ordering::AcqRel);
                        return;
                    }
                }
                for chunk in remaining.chunks(128) {
                    if reply.slow {
                        tokio::time::sleep(Duration::from_millis(15)).await;
                    }
                    if socket.write_all(chunk).await.is_err() {
                        break;
                    }
                }
                active.fetch_sub(1, Ordering::AcqRel);
            });
        }
    });
    Server {
        base,
        requests,
        peak,
        stop,
    }
}
fn config(server: &Server, output: &std::path::Path) -> GrabberConfig {
    GrabberConfig {
        start_url: format!("{}/", server.base),
        output_dir: output.into(),
        ..GrabberConfig::default()
    }
}
fn paths(server: &Server) -> Vec<String> {
    server
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|req| {
            req.lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string()
        })
        .collect()
}

fn attr(html: &str, selector: &str, attribute: &str) -> String {
    let mut value = None;
    lol_html::rewrite_str(
        html,
        lol_html::RewriteStrSettings::new().append_element_content_handler(lol_html::element!(
            selector,
            |element| {
                value = element.get_attribute(attribute);
                Ok(())
            }
        )),
    )
    .unwrap();
    html_escape::decode_html_entities(&value.unwrap()).into_owned()
}

#[tokio::test]
async fn captures_recursive_css_inline_styles_and_srcset_without_splitting_data_commas() {
    let outside = serve(HashMap::from([(
        "/external.png".into(),
        Reply::data("image/png", b"external"),
    )]))
    .await;
    let html = r##"<html><head><base href="/site/">
        <link rel="stylesheet" href="main.css?theme=1">
        <style>@import "inline.css"; .hero { background:url(hero.png?x=1&y=2#tile); mask:url(#local); a:url(data:image/png,a,b); }</style></head>
        <body><img srcset="data:image/svg+xml,%3Csvg%3E,a,b 1x, /hi.png?q=1,2 2x, missing.png 3x">
        <picture><source srcset="/wide.png 640w, /invalid.png 1x 2x"></picture>
        <div style="background:url(&quot;inline.png?x=1&amp;y=2#tile&quot;)"></div></body></html>"##;
    let main_css = format!(
        r##"@import "nested/a.css"; @import url("nested/a.css#again");
        .a {{background:url(../assets/bg.png?v=1#first)}} .b {{background:url(../assets/bg.png?v=2)}}
        .missing {{background:url(../missing.png)}} .outside {{background:url({}/external.png)}}
        /* url(comment.png) */ .string {{content:"url(string.png)"}}
        .escape {{background:url(../weird\(x\).png)}} @font-face {{src:url(../font.woff2)}}"##,
        outside.base
    );
    let mut routes = HashMap::from([
        ("/".into(), Reply::html(html)),
        ("/site/main.css?theme=1".into(), Reply::data("text/css", main_css.as_bytes())),
        ("/site/nested/a.css".into(), Reply::data("text/css", br##"@import "../main.css?theme=1"; @import "../../more.css"; .svg{mask:url(/assets/icon.svg#mask)} .data{background:url(data:image/png,a,b)} .blob{background:url(blob:opaque)}"##)),
        ("/more.css".into(), Reply::data("text/css", b"/*last*/ @media screen { .leaf{background:url(/leaf.png)} }")),
        ("/site/inline.css".into(), Reply::data("text/css", b".inline {background:url(/inline-leaf.png)}")),
    ]);
    for path in [
        "/site/hero.png?x=1&y=2",
        "/hi.png?q=1,2",
        "/wide.png",
        "/site/inline.png?x=1&y=2",
        "/assets/bg.png?v=1",
        "/assets/bg.png?v=2",
        "/weird(x).png",
        "/font.woff2",
        "/assets/icon.svg",
        "/leaf.png",
        "/inline-leaf.png",
    ] {
        routes.insert(
            path.into(),
            Reply::data("application/octet-stream", path.as_bytes()),
        );
    }
    let server = serve(routes).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.max_depth = 0;
    let result = Grabber::create(cfg)
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.downloaded(), 16, "{:?}", result.pages);
    assert_eq!(result.failed(), 2);
    assert!(result.pages.iter().all(|page| page.depth == 0));
    let requested = paths(&server);
    for path in [
        "/site/main.css?theme=1",
        "/site/nested/a.css",
        "/more.css",
        "/assets/bg.png?v=1",
        "/assets/bg.png?v=2",
    ] {
        assert_eq!(
            requested.iter().filter(|value| *value == path).count(),
            1,
            "{requested:?}"
        );
    }
    assert!(paths(&outside).is_empty());
    assert!(!requested.iter().any(|path| path.contains("comment.png")
        || path.contains("string.png")
        || path.contains("invalid.png")
        || path.contains("data:")
        || path.contains("blob:")));
    let mirror = result.mirror_dir.unwrap();
    let index = std::fs::read_to_string(result.entry_path.unwrap()).unwrap();
    let sheet = attr(&index, "link", "href");
    assert!(mirror.join(&sheet).is_file());
    let sheet = std::fs::read_to_string(mirror.join(sheet)).unwrap();
    assert!(sheet.contains(".png#first\""));
    assert!(sheet.contains(&format!("url(\"{}/missing.png\")", server.base)));
    assert!(sheet.contains(&format!("url(\"{}/external.png\")", outside.base)));
    assert!(!sheet.contains("../assets/"));
    let source = attr(&index, "img", "srcset");
    assert!(source.starts_with("data:image/svg+xml,%3Csvg%3E,a,b 1x, "));
    assert!(source.contains(&format!("{}/site/missing.png 3x", server.base)));
    let high = source
        .split(" 1x, ")
        .nth(1)
        .unwrap()
        .split(" 2x")
        .next()
        .unwrap();
    assert!(mirror.join(high).is_file());
    let wide = attr(&index, "source", "srcset");
    assert!(
        mirror
            .join(wide.split_whitespace().next().unwrap())
            .is_file()
    );
    assert!(wide.contains("/invalid.png 1x 2x"));
    assert!(index.contains("mask:url(#local)"));
    assert!(index.contains("url(data:image/png,a,b)"));
    assert!(attr(&index, "div", "style").contains(".png#tile\""));
    let nested = std::fs::read_dir(&mirror)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "css") {
                std::fs::read_to_string(path).ok()
            } else {
                None
            }
        })
        .find(|text| text.contains(".blob"))
        .unwrap();
    assert!(nested.contains("url(blob:opaque)"));
    assert!(nested.contains("url(data:image/png,a,b)"));
    assert!(nested.contains(".svg#mask\""));
    assert!(nested.contains(".css\""));
}

#[tokio::test]
async fn css_resume_uses_saved_stylesheets_and_enforces_robots_extension_and_origin_rules() {
    let outside = serve(HashMap::new()).await;
    let server = serve(HashMap::from([
        ("/robots.txt".into(), Reply::data("text/plain", b"User-agent: *\nDisallow: /blocked.png\n")),
        ("/".into(), Reply::html(r#"<link rel=stylesheet href=/main.css>"#)),
        ("/main.css".into(), Reply::data("text/css", format!(r#"@import "/nested.css"; .ok{{background:url(/ok.png)}} .blocked{{background:url(/blocked.png)}} .font{{src:url(/skip.woff)}} .outside{{background:url({}/external.png)}}"#, outside.base).as_bytes())),
        ("/nested.css".into(), Reply::data("text/css", b"@import '/main.css'; .last{background:url(/last.png)}")),
        ("/ok.png".into(), Reply::data("image/png", b"ok")),
        ("/last.png".into(), Reply::data("image/png", b"last")),
    ])).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.max_pages = 2;
    cfg.resource_extensions = vec!["css".into(), "png".into()];
    let first = Grabber::create(cfg)
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.downloaded(), 2);
    assert_eq!(first.status, ProjectStatus::Limited);
    let mut resumed = Grabber::open(dir.path()).unwrap();
    let mut cfg = resumed.snapshot().config;
    cfg.max_pages = 20;
    resumed.update_config(cfg).unwrap();
    let result = resumed.run(CancellationToken::new()).await.unwrap();
    assert_eq!(result.downloaded(), 5);
    assert_eq!(result.status, ProjectStatus::Complete);
    assert!(
        result
            .pages
            .iter()
            .any(|page| page.url.ends_with("/blocked.png")
                && matches!(page.state, PageState::Skipped(_)))
    );
    let requested = paths(&server);
    for path in ["/", "/main.css", "/nested.css", "/ok.png", "/last.png"] {
        assert_eq!(
            requested.iter().filter(|value| *value == path).count(),
            1,
            "{requested:?}"
        );
    }
    assert!(!requested.contains(&"/skip.woff".into()));
    assert!(!requested.contains(&"/blocked.png".into()));
    assert!(paths(&outside).is_empty());
}

#[tokio::test]
async fn declared_legacy_encodings_produce_utf8_mirrors_and_survive_resume() {
    let html = r#"<meta charset="utf-8"><title>中文页面</title><img src="/图.png"><a href="/meta">café</a><a href="/bad">bad</a><link rel="stylesheet" href="/legacy.css">"#;
    let (html, _, errors) = encoding_rs::GBK.encode(html);
    assert!(!errors);
    let server = serve(HashMap::from([
        ("/".into(), Reply::data("text/html; charset=\"gbk\"", &html)),
        ("/meta".into(), Reply::data("text/html", b"<meta http-equiv=Content-Type content='text/html; charset=windows-1252'><p>caf\xe9</p><img src=/caf\xe9.png>")),
        ("/legacy.css".into(), Reply::data("text/css", b"@charset \"windows-1252\"; /*caf\xe9*/ .a{background:url('/caf\xe9.png')}")),
        ("/bad".into(), Reply::data("text/html; charset=utf-8", b"broken\xff")),
        ("/%E5%9B%BE.png".into(), Reply::data("image/png", b"cn")),
        ("/caf%C3%A9.png".into(), Reply::data("image/png", b"cafe")),
    ])).await;
    let dir = tempfile::tempdir().unwrap();
    let first = Grabber::create(config(&server, dir.path()))
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.downloaded(), 5, "{:?}", first.pages);
    assert_eq!(first.failed(), 1);
    let index = std::fs::read(first.entry_path.as_ref().unwrap()).unwrap();
    assert!(index.starts_with(b"\xef\xbb\xbf"));
    let index = String::from_utf8(index).unwrap();
    assert!(index.contains("中文页面"));
    assert!(index.contains(&format!("{}/bad", server.base)));
    let mirror = first.mirror_dir.as_ref().unwrap();
    let css = std::fs::read(mirror.join(attr(&index, "link", "href"))).unwrap();
    assert!(css.starts_with(b"\xef\xbb\xbf"));
    assert!(String::from_utf8(css).unwrap().contains("café"));
    assert!(
        first
            .pages
            .iter()
            .find(|page| page.url == format!("{}/", server.base))
            .unwrap()
            .encoding
            .as_deref()
            == Some("GBK")
    );
    let old_bytes = first.received_bytes;
    let result = Grabber::open(dir.path())
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.downloaded(), 5);
    assert!(result.received_bytes >= old_bytes);
    for path in [
        "/",
        "/meta",
        "/legacy.css",
        "/%E5%9B%BE.png",
        "/caf%C3%A9.png",
    ] {
        assert_eq!(
            paths(&server).iter().filter(|value| *value == path).count(),
            1
        );
    }
}

#[tokio::test]
async fn captures_cycles_resources_queries_and_rewrites_only_successful_files() {
    let server = serve(HashMap::from([
        ("/".into(), Reply::html(r#"<html><head><base href="/docs/"></head><body><a href="a.html#part">A</a><a href="../query?v=1&amp;x=2">Q1</a><a href="../query?v=2">Q2</a><img src="../img.png"><img src="../missing.png"><form action="/submit"><input name="x"></form><script src="../code.js"></script></body></html>"#)),
        ("/docs/a.html".into(), Reply::html(r#"<a href="../">loop</a><a href="deep.html">deep</a>"#)),
        ("/docs/deep.html".into(), Reply::html("deep")),
        ("/query?v=1&x=2".into(), Reply::html("one")),
        ("/query?v=2".into(), Reply::html("two")),
        ("/img.png".into(), Reply::data("image/png", b"image bytes")),
        ("/code.js".into(), Reply::data("application/javascript", b"fetch('/never-run')")),
    ])).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("existing.txt"), b"original").unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.max_depth = 1;
    let mut grabber = Grabber::create(cfg).unwrap();
    let result = grabber.run(CancellationToken::new()).await.unwrap();
    assert_eq!(
        result.downloaded(),
        6,
        "pages={:?}; paths={:?}",
        result.pages,
        paths(&server)
    );
    assert_eq!(result.failed(), 1);
    let requests = paths(&server);
    assert_eq!(requests.iter().filter(|path| *path == "/").count(), 1);
    assert!(!requests.contains(&"/docs/deep.html".into()));
    assert!(!requests.contains(&"/submit".into()));
    assert!(!requests.contains(&"/never-run".into()));
    assert!(requests.contains(&"/query?v=1&x=2".into()));
    let mirror = std::fs::read_to_string(result.entry_path.unwrap()).unwrap();
    assert!(!mirror.contains("<base"));
    assert!(mirror.contains(&format!("{}/missing.png", server.base)));
    assert!(!mirror.contains("../img.png"));
    assert!(mirror.contains(".html#part"));
    assert_eq!(
        std::fs::read(dir.path().join("existing.txt")).unwrap(),
        b"original"
    );
    let html_files = std::fs::read_dir(result.mirror_dir.unwrap())
        .unwrap()
        .filter_map(|file| file.ok())
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "html"))
        .count();
    assert_eq!(html_files, 4);
    for request in server.requests.lock().unwrap().iter() {
        assert!(request.starts_with("GET "));
        assert!(!request.to_lowercase().contains("\r\ncookie:"));
        assert!(!request.to_lowercase().contains("\r\nauthorization:"));
    }
}

#[tokio::test]
async fn external_redirects_robots_and_resource_filters_are_enforced_before_network() {
    let outside = serve(HashMap::from([(
        "/secret".into(),
        Reply::html("must not fetch"),
    )]))
    .await;
    let server = serve(HashMap::from([
        ("/robots.txt".into(), Reply::data("text/plain", b"User-agent: *\nDisallow: /blocked\nAllow: /blocked-ok\n")),
        ("/".into(), Reply::html(&format!(r#"<a href="/blocked">blocked</a><a href="/jump">jump</a><a href="{}/secret">outside</a><img src="/a.png"><img src="/a.jpg">"#, outside.base))),
        ("/jump".into(), Reply::redirect(format!("{}/secret", outside.base))),
        ("/blocked".into(), Reply::html("not allowed")),
        ("/a.png".into(), Reply::data("image/png", b"png")),
        ("/a.jpg".into(), Reply::data("image/jpeg", b"jpg")),
    ])).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.resource_extensions = vec!["png".into()];
    let result = Grabber::create(cfg)
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.downloaded(), 2);
    assert!(
        result
            .pages
            .iter()
            .any(|page| page.url.ends_with("/blocked")
                && matches!(page.state, PageState::Skipped(_)))
    );
    assert!(!paths(&server).contains(&"/blocked".into()));
    assert!(!paths(&server).contains(&"/a.jpg".into()));
    assert!(paths(&outside).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_is_prompt_and_restart_reuses_verified_completed_objects() {
    let gate = ResponseGate::new();
    let mut slow = Reply::data("application/octet-stream", &[7; 4096]);
    slow.gate = Some(gate.clone());
    let server = serve(HashMap::from([
        ("/".into(), Reply::html(r#"<a href="/slow.bin">slow</a>"#)),
        ("/slow.bin".into(), slow),
    ]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut grabber = Grabber::create(config(&server, dir.path())).unwrap();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let setup_started = std::time::Instant::now();
    let running = tokio::spawn(async move { grabber.run(cancel).await });
    // 先确认根页面已保存且慢响应已发出首块；不把初始化和 fsync 算进网络取消期限。
    tokio::time::timeout(Duration::from_secs(10), gate.started.cancelled())
        .await
        .expect("slow response did not start");
    let setup_elapsed = setup_started.elapsed();
    let cancel_started = std::time::Instant::now();
    signal.cancel();
    // 响应余下部分被 gate 永久阻塞，只有真正放弃请求才会断开，不能自然完成而假绿。
    tokio::time::timeout(Duration::from_millis(600), gate.disconnected.cancelled())
        .await
        .expect("cancel did not close the active HTTP response within 600 ms");
    let transport_elapsed = cancel_started.elapsed();
    // 取消仍需保存可恢复快照；让文件落盘有独立的有限期限。
    let result = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("cancelled project did not finish saving its snapshot")
        .unwrap()
        .unwrap();
    eprintln!(
        "cancel phases: setup={setup_elapsed:?}, transport={transport_elapsed:?}, persisted={:?}",
        cancel_started.elapsed()
    );
    assert_eq!(result.status, ProjectStatus::Cancelled);
    assert_eq!(result.downloaded(), 1);
    assert!(
        result
            .pages
            .iter()
            .any(|page| page.state == PageState::Pending)
    );
    let first_mirror = result.entry_path.unwrap();
    let old_bytes = std::fs::read(&first_mirror).unwrap();
    // 部分落盘的最后快照不会破坏前一个完整快照。
    std::fs::write(
        dir.path()
            .join(".swoop-grabber/state-00000000009999999999.json"),
        b"{partial",
    )
    .unwrap();
    gate.release.cancel();
    let mut resumed = Grabber::open(dir.path()).unwrap();
    let done = resumed.run(CancellationToken::new()).await.unwrap();
    assert_eq!(done.downloaded(), 2);
    assert_eq!(done.status, ProjectStatus::Complete);
    assert_eq!(paths(&server).iter().filter(|path| *path == "/").count(), 1);
    assert_ne!(done.entry_path.as_ref().unwrap(), &first_mirror);
    assert_eq!(std::fs::read(first_mirror).unwrap(), old_bytes);
}

#[tokio::test]
async fn page_and_byte_limits_bound_work_with_parallel_responses() {
    let index = (0..12)
        .map(|number| format!(r#"<img src="/{number}.bin">"#))
        .collect::<String>();
    let mut routes = HashMap::from([("/".into(), Reply::html(&index))]);
    for number in 0..12 {
        let mut reply = Reply::data("application/octet-stream", &[1; 2048]);
        reply.slow = true;
        routes.insert(format!("/{number}.bin"), reply);
    }
    let server = serve(routes).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.max_pages = 5;
    cfg.concurrency = 2;
    cfg.max_total_bytes = 1800;
    let result = Grabber::create(cfg)
        .unwrap()
        .run(CancellationToken::new())
        .await
        .unwrap();
    assert!(result.pages.len() <= 5);
    assert!(result.received_bytes <= 1800);
    assert_eq!(result.status, ProjectStatus::Limited);
    assert!(server.peak.load(Ordering::Acquire) <= 2);
    assert!(
        result
            .pages
            .iter()
            .any(|page| page.state == PageState::Pending)
    );
}

#[tokio::test]
async fn hostile_paths_and_existing_project_are_never_overwritten() {
    let server = serve(HashMap::from([
        (
            "/".into(),
            Reply::html(r#"<img src="/../../outside.png?x=1"><img src="/outside.png?x=2">"#),
        ),
        ("/outside.png?x=1".into(), Reply::data("image/png", b"one")),
        ("/outside.png?x=2".into(), Reply::data("image/png", b"two")),
    ]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(&server, dir.path());
    let mut grabber = Grabber::create(cfg.clone()).unwrap();
    assert!(Grabber::create(cfg).is_err());
    let result = grabber.run(CancellationToken::new()).await.unwrap();
    let mirror = result.mirror_dir.unwrap();
    let files = std::fs::read_dir(&mirror)
        .unwrap()
        .map(|file| file.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 3);
    assert!(
        files
            .iter()
            .all(|file| file.parent() == Some(mirror.as_path()))
    );
    assert!(!dir.path().join("outside.png").exists());
}

#[tokio::test]
async fn raising_page_limit_rebuilds_frontier_without_redownloading_successes() {
    let server = serve(HashMap::from([
        (
            "/".into(),
            Reply::html(r#"<a href="/a">a</a><a href="/b">b</a>"#),
        ),
        ("/a".into(), Reply::html("a")),
        ("/b".into(), Reply::html("b")),
    ]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, dir.path());
    cfg.max_pages = 2;
    let mut grabber = Grabber::create(cfg).unwrap();
    let first = grabber.run(CancellationToken::new()).await.unwrap();
    assert_eq!(first.status, ProjectStatus::Limited);
    assert_eq!(first.downloaded(), 2);
    let mut resumed = Grabber::open(dir.path()).unwrap();
    let mut next = resumed.snapshot().config;
    next.max_pages = 3;
    resumed.update_config(next).unwrap();
    let second = resumed.run(CancellationToken::new()).await.unwrap();
    assert_eq!(second.status, ProjectStatus::Complete);
    assert_eq!(second.downloaded(), 3);
    assert_eq!(paths(&server).iter().filter(|path| *path == "/").count(), 1);
    assert_eq!(
        paths(&server).iter().filter(|path| *path == "/a").count(),
        1
    );
}

#[tokio::test]
async fn concurrent_project_instances_fail_without_overwriting_state() {
    let mut slow = Reply::html(&"x".repeat(4096));
    slow.slow = true;
    let server = serve(HashMap::from([("/".into(), slow)])).await;
    let dir = tempfile::tempdir().unwrap();
    let mut first = Grabber::create(config(&server, dir.path())).unwrap();
    let mut updates = first.subscribe();
    let stop = CancellationToken::new();
    let signal = stop.clone();
    let running = tokio::spawn(async move { first.run(signal).await });
    updates.changed().await.unwrap();
    let mut second = Grabber::open(dir.path()).unwrap();
    let error = second.run(CancellationToken::new()).await.unwrap_err();
    assert!(error.to_string().contains("独占锁"));
    stop.cancel();
    running.await.unwrap().unwrap();
    // 锁随 run 返回释放，项目仍可恢复。
    assert!(second.run(CancellationToken::new()).await.is_ok());
}

#[tokio::test]
async fn captures_svg_manifest_media_lazy_images_and_css_image_set() {
    let mut routes = HashMap::from([
        ("/".into(), Reply::html(r##"<html><head><link rel="manifest" href="/app.json"><style>.photo{background:image-set("/one.png" 1x, "/two.png" 2x type("image/png"))}</style></head><body background="/background.png">
        <svg><use href="/sprite.svg#logo"/><use xlink:href="#local"/></svg>
        <video src="/clip.mp4" poster="/poster.png"><track src="/captions.vtt"></video>
        <audio><source src="/sound.ogg"></audio><object data="/manual.pdf"></object><embed src="/embedded.pdf">
        <input type="image" src="/submit.png"><img data-src="/lazy.png" data-original="/original.png" data-srcset="/lazy2.png 2x">
        </body></html>"##)),
        ("/sprite.svg".into(), Reply::data("image/svg+xml", br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="/nested.png"/><use href="#local"/><image href="/other.png"/><style>.s{fill:url(/texture.svg#fill)}</style></svg>"##)),
        ("/texture.svg".into(), Reply::data("image/svg+xml", b"<svg><defs><pattern id=\"fill\"/></defs></svg>")),
        ("/app.json".into(), Reply::data("application/json", br#"{"name":"offline","start_url":"/home","scope":"/","icons":[{"src":"/icon.png"}],"screenshots":[{"src":"/screen.png"}],"shortcuts":[{"url":"/shortcut","icons":[{"src":"/shortcut.png"}]}],"unrecognized":"/must-not-fetch"}"#)),
        ("/home".into(), Reply::html("<p>home")),
        ("/shortcut".into(), Reply::html("<p>shortcut")),
    ]);
    for path in [
        "/one.png",
        "/two.png",
        "/background.png",
        "/clip.mp4",
        "/poster.png",
        "/captions.vtt",
        "/sound.ogg",
        "/manual.pdf",
        "/embedded.pdf",
        "/submit.png",
        "/lazy.png",
        "/original.png",
        "/lazy2.png",
        "/nested.png",
        "/other.png",
        "/icon.png",
        "/screen.png",
        "/shortcut.png",
    ] {
        routes.insert(
            path.into(),
            Reply::data("application/octet-stream", path.as_bytes()),
        );
    }
    let server = serve(routes).await;
    let root = tempfile::tempdir().unwrap();
    let mut project = Grabber::create(config(&server, root.path())).unwrap();
    let snapshot = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(snapshot.failed(), 0, "{:?}", snapshot.pages);
    assert_eq!(snapshot.downloaded(), 24);
    let mirror = snapshot.mirror_dir.as_ref().unwrap();
    let index = std::fs::read_to_string(snapshot.entry_path.as_ref().unwrap()).unwrap();
    assert!(!index.contains("image/png\".png"));
    let svg_path = attr(&index, "use[href]", "href");
    let svg = std::fs::read_to_string(mirror.join(svg_path.split('#').next().unwrap())).unwrap();
    let nested = attr(&svg, "image[xlink\\:href]", "xlink:href");
    assert_eq!(std::fs::read(mirror.join(nested)).unwrap(), b"/nested.png");
    assert!(svg.contains("href=\"#local\""));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(mirror.join(attr(&index, "link", "href"))).unwrap())
            .unwrap();
    assert_eq!(
        std::fs::read(mirror.join(manifest["icons"][0]["src"].as_str().unwrap())).unwrap(),
        b"/icon.png"
    );
    assert!(
        mirror
            .join(manifest["start_url"].as_str().unwrap())
            .exists()
    );
    assert_eq!(
        manifest["scope"].as_str().unwrap(),
        format!("{}/", server.base)
    );
    assert!(
        !paths(&server)
            .iter()
            .any(|path| path.contains("must-not-fetch") || path == "/image/png")
    );
    let count = paths(&server).len();
    let mut recovered = Grabber::open(root.path()).unwrap();
    let again = recovered.run(CancellationToken::new()).await.unwrap();
    assert_eq!(again.downloaded(), snapshot.downloaded());
    assert_eq!(
        paths(&server).len(),
        count,
        "successful resources should be reused"
    );
}

#[tokio::test]
async fn resource_switches_and_url_exclusions_apply_to_discovery_and_redirects() {
    let server = serve(HashMap::from([
        ("/".into(), Reply::html(r#"<script src="/app.js"></script><script>inline()</script><link rel="modulepreload" href="/module.js"><link rel="preload" as="video" href="/preload.mp4"><link rel="manifest" href="/app.webmanifest"><video src="/clip.mp4" poster="/poster.png"></video><iframe src="/frame"></iframe><img data-src="/lazy.png"><img src="/allowed.png"><a href="/skip/direct">excluded</a><a href="/redirect">redirect</a>"#)),
        ("/allowed.png".into(), Reply::data("image/png", b"image")),
        ("/redirect".into(), Reply::redirect("/skip/secret".into())),
        ("/skip/secret".into(), Reply::html("never fetched")),
    ])).await;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, root.path());
    cfg.capture = swoop_grabber::CaptureOptions {
        scripts: false,
        media: false,
        embedded_documents: false,
        manifests: false,
        lazy_images: false,
        strip_scripts_from_mirror: true,
    };
    cfg.excluded_url_patterns = vec!["/skip/*".into()];
    let mut project = Grabber::create(cfg).unwrap();
    let snapshot = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(snapshot.downloaded(), 2);
    assert!(
        snapshot
            .pages
            .iter()
            .any(|page| page.url.ends_with("/redirect")
                && matches!(page.state, PageState::Skipped(_)))
    );
    let fetched = paths(&server);
    assert!(
        fetched
            .iter()
            .all(|path| ["/", "/robots.txt", "/allowed.png", "/redirect"].contains(&path.as_str())),
        "{fetched:?}"
    );
    let index = std::fs::read_to_string(snapshot.entry_path.unwrap()).unwrap();
    assert!(!index.contains("<script"));
    assert!(
        index.contains(&format!("{}/clip.mp4", server.base)),
        "skipped resource must retain absolute fallback"
    );
    let saved = Grabber::open(root.path()).unwrap().snapshot().config;
    assert!(!saved.capture.media);
    assert_eq!(saved.excluded_url_patterns, ["/skip/*"]);
}

#[tokio::test]
async fn rendered_dom_snapshot_discovers_generated_nodes_and_survives_restart() {
    let server = serve(HashMap::from([
        (
            "/".into(),
            Reply::html("<div id=app></div><script>/* builds nodes in browser */</script>"),
        ),
        (
            "/generated.png".into(),
            Reply::data("image/png", b"generated-image"),
        ),
        ("/generated-page".into(), Reply::html("<p>generated page")),
    ]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let mut project = Grabber::create(config(&server, root.path())).unwrap();
    let initial = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(initial.downloaded(), 1);
    let first_mirror = initial.entry_path.unwrap();
    let dom = "<div id=app><img src='/generated.png'><a href='/generated-page'>rendered</a></div>";
    project
        .import_rendered_html(&format!("{}/", server.base), dom)
        .unwrap();
    let imported = project.snapshot();
    assert_eq!(
        imported.pages[0].source,
        swoop_grabber::PageSource::RenderedSnapshot
    );
    assert!(imported.entry_path.is_none());
    let bytes = imported.received_bytes;
    project
        .import_rendered_html(&format!("{}/", server.base), dom)
        .unwrap();
    assert_eq!(
        project.snapshot().received_bytes,
        bytes,
        "identical reimport must not charge twice"
    );
    drop(project);
    let mut project = Grabber::open(root.path()).unwrap();
    let result = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(result.downloaded(), 3);
    assert_eq!(
        result.pages[0].source,
        swoop_grabber::PageSource::RenderedSnapshot
    );
    assert_eq!(
        paths(&server)
            .iter()
            .filter(|path| path.as_str() == "/")
            .count(),
        1,
        "must use actual imported DOM, not original server HTML"
    );
    let index = std::fs::read_to_string(result.entry_path.unwrap()).unwrap();
    assert!(index.contains("rendered"));
    assert_eq!(
        std::fs::read(result.mirror_dir.unwrap().join(attr(&index, "img", "src"))).unwrap(),
        b"generated-image"
    );
    assert!(first_mirror.exists(), "old snapshot is retained");
}

#[tokio::test]
async fn rendered_snapshot_rejects_unknown_origin_budget_and_stale_instance() {
    let server = serve(HashMap::from([])).await;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, root.path());
    cfg.max_file_bytes = 20;
    cfg.max_total_bytes = 25;
    let mut project = Grabber::create(cfg).unwrap();
    let mut stale = Grabber::open(root.path()).unwrap();
    let url = format!("{}/", server.base);
    assert!(
        project
            .import_rendered_html("https://outside.invalid/", "<p>x")
            .is_err()
    );
    assert!(
        project
            .import_rendered_html(&format!("{}/unknown", server.base), "<p>x")
            .is_err()
    );
    assert!(project.import_rendered_html(&url, &"x".repeat(21)).is_err());
    assert!(project.import_rendered_html(&url, "").is_err());
    project
        .import_rendered_html(&url, "<p>first snapshot")
        .unwrap();
    assert!(stale.import_rendered_html(&url, "<p>stale").is_err());
    assert!(
        project
            .import_rendered_html(&url, "<p>second snapshot")
            .is_err()
    );
    assert_eq!(project.snapshot().pages[0].bytes, 17);
}

#[tokio::test]
async fn malformed_manifest_fails_without_ending_project_and_redirect_limit_is_configurable() {
    let server = serve(HashMap::from([
        ("/".into(), Reply::html("<link rel=manifest href=/bad.webmanifest><a href=/redirect>go</a><img src=/ok.png>")),
        ("/bad.webmanifest".into(), Reply::data("application/manifest+json", b"{not json")),
        ("/redirect".into(), Reply::redirect("/end".into())),
        ("/ok.png".into(), Reply::data("image/png", b"ok")),
    ]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, root.path());
    cfg.max_redirects = 0;
    let mut project = Grabber::create(cfg).unwrap();
    let result = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(result.failed(), 2);
    assert_eq!(result.downloaded(), 2);
    assert!(!paths(&server).iter().any(|path| path == "/end"));
    let index = std::fs::read_to_string(result.entry_path.unwrap()).unwrap();
    assert_eq!(
        attr(&index, "link", "href"),
        format!("{}/bad.webmanifest", server.base)
    );
}

fn renderer_binary() -> std::path::PathBuf {
    std::env::var_os("SWOOP_GRABBER_CHROMIUM")
        .expect(
            "set SWOOP_GRABBER_CHROMIUM to an explicitly chosen Chrome/Chromium/Edge executable",
        )
        .into()
}

#[tokio::test]
#[ignore = "requires an explicitly configured local Chromium executable"]
async fn real_chromium_renders_javascript_and_blocks_outside_origin() {
    let outside = serve(HashMap::from([(
        "/outside.png".into(),
        Reply::data("image/png", b"outside"),
    )]))
    .await;
    let source = format!(
        r#"<!doctype html><html><head><title>fixture</title></head><body><div id="app"></div><script>
        setTimeout(() => {{ const img = document.createElement('img'); img.src='/generated.png'; document.querySelector('#app').append(img);
        const outside = new Image(); outside.src='{}/outside.png'; }}, 20);
        </script></body></html>"#,
        outside.base
    );
    let server = serve(HashMap::from([
        ("/".into(), Reply::html(&source)),
        (
            "/generated.png".into(),
            Reply::data("image/png", b"rendered-node"),
        ),
    ]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, root.path());
    cfg.render.enabled = true;
    cfg.render.executable = renderer_binary();
    cfg.render.virtual_time_ms = 500;
    cfg.render.timeout_secs = 20;
    let mut project = Grabber::create(cfg).unwrap();
    let result = project.run(CancellationToken::new()).await.unwrap();
    assert_eq!(result.failed(), 0, "{:?}", result.pages);
    assert_eq!(result.downloaded(), 2);
    assert_eq!(
        result.pages[0].source,
        swoop_grabber::PageSource::BrowserRendered
    );
    assert!(result.rendered_network_bytes > 0);
    assert!(
        paths(&outside).is_empty(),
        "renderer must not reach another origin: {:?}",
        paths(&outside)
    );
    let index = std::fs::read_to_string(result.entry_path.unwrap()).unwrap();
    assert_eq!(
        std::fs::read(
            result
                .mirror_dir
                .unwrap()
                .join(attr(&index, "#app img", "src"))
        )
        .unwrap(),
        b"rendered-node"
    );
    let before = paths(&server).len();
    let mut recovered = Grabber::open(root.path()).unwrap();
    let again = recovered.run(CancellationToken::new()).await.unwrap();
    assert_eq!(again.rendered_network_bytes, result.rendered_network_bytes);
    assert_eq!(paths(&server).len(), before);
}

#[tokio::test]
#[ignore = "requires an explicitly configured local Chromium executable"]
async fn real_chromium_timeout_dom_and_network_limits_fail_explicitly() {
    for case in ["timeout", "dom", "network"] {
        let server = serve(HashMap::from([(
            "/".into(),
            Reply::html(match case {
                "timeout" => "<html><script>while(true){}</script></html>",
                _ => "<html><body><script>document.body.textContent='x'.repeat(50000)</script></body></html>",
            }),
        )]))
        .await;
        let root = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, root.path());
        cfg.render.enabled = true;
        cfg.render.executable = renderer_binary();
        cfg.render.virtual_time_ms = 500;
        cfg.render.timeout_secs = if case == "timeout" { 1 } else { 15 };
        if case == "dom" {
            cfg.render.max_dom_bytes = 100;
        }
        if case == "network" {
            cfg.render.max_network_bytes = 1;
        }
        let mut project = Grabber::create(cfg).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            project.run(CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.failed(), 1, "{case}: {:?}", result.pages);
        let PageState::Failed(error) = &result.pages[0].state else {
            unreachable!()
        };
        assert!(
            error.contains(match case {
                "timeout" => "超时",
                "dom" => "DOM",
                _ => "流量",
            }),
            "{case}: {error}"
        );
        assert!(result.entry_path.is_none());
    }
}

#[tokio::test]
#[ignore = "requires an explicitly configured local Chromium executable"]
async fn real_chromium_cancellation_stops_browser_and_releases_project() {
    let server = serve(HashMap::from([(
        "/".into(),
        Reply::html("<html><script>while(true){}</script></html>"),
    )]))
    .await;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(&server, root.path());
    cfg.render.enabled = true;
    cfg.render.executable = renderer_binary();
    cfg.render.timeout_secs = 30;
    let mut project = Grabber::create(cfg).unwrap();
    let stop = CancellationToken::new();
    let run_stop = stop.clone();
    let mut run = tokio::spawn(async move { project.run(run_stop).await.unwrap() });
    for _ in 0..100 {
        if paths(&server)
            .iter()
            .filter(|path| path.as_str() == "/")
            .count()
            >= 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        paths(&server)
            .iter()
            .filter(|path| path.as_str() == "/")
            .count()
            >= 2,
        "browser never loaded fixture"
    );
    let start = std::time::Instant::now();
    stop.cancel();
    let snapshot = tokio::time::timeout(Duration::from_secs(3), &mut run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.status, ProjectStatus::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(3));
    let mut reopened = Grabber::open(root.path()).unwrap();
    let mut config = reopened.snapshot().config;
    config.render.enabled = false;
    reopened.update_config(config).unwrap();
    assert_eq!(
        reopened
            .run(CancellationToken::new())
            .await
            .unwrap()
            .downloaded(),
        1
    );
}
