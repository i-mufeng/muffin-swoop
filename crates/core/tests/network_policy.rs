use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use swoop_core::{
    AddOptions, Config, Engine, Event, MAIN_QUEUE_ID, NetworkPolicy, ProxyConfig, Store, TaskStatus,
};

#[derive(Clone, Debug)]
struct Request {
    target: String,
    headers: Vec<String>,
}
impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
    }
}
struct HttpServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}
fn serve(handler: impl Fn(Request, &mut TcpStream) + Send + Sync + 'static) -> HttpServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let history = requests.clone();
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (handler, history) = (handler.clone(), history.clone());
            std::thread::spawn(move || {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    return;
                }
                let target = first.split_whitespace().nth(1).unwrap().to_owned();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    headers.push(line.trim_end().to_owned());
                }
                let request = Request { target, headers };
                history.lock().unwrap().push(request.clone());
                handler(request, &mut stream);
            });
        }
    });
    HttpServer { url, requests }
}
fn status(stream: &mut TcpStream, status: &str, extra: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n{extra}\r\n"
    );
}
fn content(request: &Request, stream: &mut TcpStream) {
    let (start, end) = request
        .header("range")
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.split_once('-'))
        .map(|(a, b)| {
            (
                a.parse::<usize>().unwrap(),
                b.parse::<usize>().unwrap_or(32767),
            )
        })
        .unwrap_or((0, 32767));
    let end = end.min(32767);
    let _ = write!(
        stream,
        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/32768\r\nConnection: close\r\n\r\n",
        end - start + 1
    );
    let _ = stream.write_all(&vec![42; end - start + 1]);
}
fn options(url: String, dir: &std::path::Path, name: &str) -> AddOptions {
    AddOptions {
        url,
        dir: dir.to_path_buf(),
        file_name: Some(name.into()),
        ..Default::default()
    }
}
fn wait(label: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "等待 {label} 超时"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn terminal(engine: &Engine, id: swoop_core::TaskId) -> TaskStatus {
    wait("终态", || {
        matches!(
            engine.snapshot_one(id).unwrap().status,
            TaskStatus::Completed | TaskStatus::Failed(_)
        )
    });
    engine.snapshot_one(id).unwrap().status
}
fn policy() -> NetworkPolicy {
    NetworkPolicy {
        use_system_proxy: false,
        retries: 0,
        ..Default::default()
    }
}

#[test]
fn sqlite_delete_failure_preserves_task_membership_and_emits_no_removed_event() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("delete.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(rt.handle().clone(), Config::default(), store.clone());
    engine.stop_queue(MAIN_QUEUE_ID).unwrap();
    let id = engine
        .add(options(
            "http://127.0.0.1:1/test".into(),
            dir.path(),
            "a.bin",
        ))
        .unwrap();
    let mut events = engine.subscribe();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER deny_member_delete BEFORE DELETE ON queue_members BEGIN SELECT RAISE(ABORT, 'test delete blocked'); END;").unwrap();
    let error = engine.remove(id, false).unwrap_err();
    assert!(error.to_string().contains("test delete blocked"), "{error}");
    assert_eq!(engine.snapshot_one(id).unwrap().status, TaskStatus::Queued);
    assert_eq!(engine.queue_for_task(id), Some(MAIN_QUEUE_ID));
    assert_eq!(
        store.load_tasks().unwrap().len(),
        1,
        "DELETE tasks 必须随成员删除失败回滚"
    );
    assert!(!matches!(events.try_recv(), Ok(Event::Removed(_))));
    conn.execute_batch("DROP TRIGGER deny_member_delete")
        .unwrap();
    engine.remove(id, false).unwrap();
    assert!(engine.snapshot_one(id).is_none());
    assert!(store.load_tasks().unwrap().is_empty());
}

#[test]
fn network_policy_validation_is_atomic_and_debug_redacts_proxy_credentials() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = Engine::new(rt.handle().clone(), Config::default());
    let old = engine.network_policy();
    let invalid = ProxyConfig {
        url: "http://alice:secret@localhost:8080".into(),
        bypass: vec![],
    };
    assert!(!format!("{invalid:?}").contains("secret"));
    assert!(!format!("{invalid:?}").contains("alice"));
    assert!(
        engine
            .set_network_policy(NetworkPolicy {
                proxy: Some(invalid),
                ..policy()
            })
            .is_err()
    );
    assert_eq!(engine.network_policy(), old);
    for invalid in [
        NetworkPolicy {
            connect_timeout_secs: 0,
            ..policy()
        },
        NetworkPolicy {
            read_timeout_secs: 86401,
            ..policy()
        },
        NetworkPolicy {
            request_timeout_secs: Some(0),
            ..policy()
        },
        NetworkPolicy {
            retries: 101,
            ..policy()
        },
        NetworkPolicy {
            retry_delay_ms: 100,
            retry_max_delay_ms: 99,
            ..policy()
        },
    ] {
        assert!(engine.set_network_policy(invalid).is_err());
    }
    let valid = NetworkPolicy {
        connect_timeout_secs: 7,
        read_timeout_secs: 8,
        retries: 3,
        ..policy()
    };
    engine.set_network_policy(valid.clone()).unwrap();
    assert_eq!(engine.network_policy(), valid);
    assert_eq!(
        serde_json::from_str::<NetworkPolicy>(&serde_json::to_string(&valid).unwrap()).unwrap(),
        valid
    );
}

#[test]
fn cross_origin_redirects_strip_all_credentials_origin_and_referer_on_every_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let destination = serve(|request, stream| content(&request, stream));
    let location = format!("Location: {}/final\r\n", destination.url);
    let source = serve(move |_, stream| status(stream, "302 Found", &location));
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine.set_network_policy(policy()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options(format!("{}/private", source.url), dir.path(), "safe.bin");
    opts.headers = vec![
        ("Cookie".into(), "session=secret".into()),
        ("Authorization".into(), "Bearer private".into()),
        ("Origin".into(), source.url.clone()),
        ("Referer".into(), format!("{}/secret?token=x", source.url)),
        ("X-Api-Key".into(), "private-key".into()),
        ("Proxy-Authorization".into(), "Basic proxy-secret".into()),
        ("User-Agent".into(), "Swoop-test".into()),
    ];
    let id = engine.add(opts).unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::Completed);
    let sources = source.requests.lock().unwrap();
    assert!(
        sources
            .iter()
            .all(|request| request.header("cookie").is_some())
    );
    assert!(
        sources
            .iter()
            .all(|request| request.header("proxy-authorization").is_none())
    );
    let targets = destination.requests.lock().unwrap();
    assert!(targets.len() >= 2, "探测和区段请求都必须通过跳转安全策略");
    for request in targets.iter() {
        for key in [
            "cookie",
            "authorization",
            "origin",
            "referer",
            "x-api-key",
            "proxy-authorization",
        ] {
            assert!(request.header(key).is_none(), "跨源泄漏 {key}: {request:?}");
        }
        assert_eq!(request.header("user-agent"), Some("Swoop-test"));
        assert!(request.header("range").is_some());
    }
    assert_eq!(
        std::fs::read(dir.path().join("safe.bin")).unwrap(),
        vec![42; 32768]
    );
}

#[test]
fn same_origin_redirects_preserve_required_authentication_and_headers() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = serve(|request, stream| {
        if request.target == "/start" {
            status(stream, "307 Temporary Redirect", "Location: /final\r\n");
        } else if request.header("authorization") == Some("Bearer required")
            && request.header("cookie") == Some("session=required")
        {
            content(&request, stream);
        } else {
            status(stream, "403 Forbidden", "");
        }
    });
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine.set_network_policy(policy()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options(format!("{}/start", server.url), dir.path(), "same.bin");
    opts.headers = vec![
        ("Cookie".into(), "session=required".into()),
        ("Authorization".into(), "Bearer required".into()),
        ("Origin".into(), server.url.clone()),
        ("Referer".into(), server.url.clone()),
    ];
    let id = engine.add(opts).unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::Completed);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.target == "/final")
            .all(
                |request| request.header("origin").is_some() && request.header("referer").is_some()
            )
    );
}

#[test]
fn retry_count_and_backoff_are_frozen_per_run_while_new_runs_use_updated_policy() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let attempts = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let counts = attempts.clone();
    let server = serve(move |request, stream| {
        if request.header("range") == Some("bytes=0-0") {
            content(&request, stream);
            return;
        }
        let mut counts = counts.lock().unwrap();
        let count = counts.entry(request.target.clone()).or_default();
        *count += 1;
        if *count == 1 {
            status(stream, "503 Unavailable", "");
        } else {
            content(&request, stream);
        }
    });
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine
        .set_network_policy(NetworkPolicy {
            retries: 1,
            retry_delay_ms: 300,
            retry_max_delay_ms: 300,
            ..policy()
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let a = engine
        .add(options(
            format!("{}/first", server.url),
            dir.path(),
            "first.bin",
        ))
        .unwrap();
    wait("首次区段失败", || {
        attempts.lock().unwrap().get("/first") == Some(&1)
    });
    engine.set_network_policy(policy()).unwrap();
    assert_eq!(terminal(&engine, a), TaskStatus::Completed);
    assert!(started.elapsed() >= Duration::from_millis(280));
    assert_eq!(attempts.lock().unwrap().get("/first"), Some(&2));
    let b = engine
        .add(options(
            format!("{}/second", server.url),
            dir.path(),
            "second.bin",
        ))
        .unwrap();
    assert!(matches!(terminal(&engine, b), TaskStatus::Failed(_)));
    assert_eq!(attempts.lock().unwrap().get("/second"), Some(&1));
}

#[test]
fn probe_retries_transient_errors_but_never_retries_bad_credentials() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts = count.clone();
    let server = serve(move |request, stream| {
        if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            status(stream, "503 Unavailable", "");
        } else {
            content(&request, stream);
        }
    });
    let engine = Engine::new(rt.handle().clone(), Config::default());
    engine
        .set_network_policy(NetworkPolicy {
            retries: 2,
            retry_delay_ms: 10,
            retry_max_delay_ms: 20,
            ..policy()
        })
        .unwrap();
    assert!(rt.block_on(engine.probe_url(&server.url, &[])).is_ok());
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 3);
    let forbidden = serve(|_, stream| status(stream, "401 Unauthorized", ""));
    assert!(rt.block_on(engine.probe_url(&forbidden.url, &[])).is_err());
    assert_eq!(forbidden.requests.lock().unwrap().len(), 1);
}

#[test]
fn manual_http_proxy_and_host_bypass_route_real_http_downloads() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let proxy = serve(|request, stream| content(&request, stream));
    let direct = serve(|request, stream| content(&request, stream));
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine
        .set_network_policy(NetworkPolicy {
            proxy: Some(ProxyConfig {
                url: proxy.url.clone(),
                bypass: vec!["127.0.0.1".into()],
            }),
            ..policy()
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let proxied = engine
        .add(options(
            "http://download.invalid/proxy.bin".into(),
            dir.path(),
            "proxy.bin",
        ))
        .unwrap();
    assert_eq!(terminal(&engine, proxied), TaskStatus::Completed);
    assert!(
        proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.target == "http://download.invalid/proxy.bin")
    );
    let proxy_count = proxy.requests.lock().unwrap().len();
    let direct_id = engine
        .add(options(
            format!("{}/direct", direct.url),
            dir.path(),
            "direct.bin",
        ))
        .unwrap();
    assert_eq!(terminal(&engine, direct_id), TaskStatus::Completed);
    assert_eq!(proxy.requests.lock().unwrap().len(), proxy_count);
    assert!(direct.requests.lock().unwrap().len() >= 2);
}

#[test]
fn socks5h_proxy_resolves_the_origin_remotely_and_transfers_bytes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let origin = serve(|request, stream| content(&request, stream));
    let origin_addr = origin.url.strip_prefix("http://").unwrap().to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("socks5h://{}", listener.local_addr().unwrap());
    let hosts = Arc::new(Mutex::new(Vec::new()));
    let received_hosts = hosts.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut client) = stream else { break };
            let (origin_addr, hosts) = (origin_addr.clone(), received_hosts.clone());
            std::thread::spawn(move || {
                let mut hello = [0; 2];
                client.read_exact(&mut hello).unwrap();
                assert_eq!(hello[0], 5);
                let mut methods = vec![0; hello[1] as usize];
                client.read_exact(&mut methods).unwrap();
                client.write_all(&[5, 0]).unwrap();
                let mut command = [0; 4];
                client.read_exact(&mut command).unwrap();
                assert_eq!(&command[..3], &[5, 1, 0]);
                assert_eq!(command[3], 3, "socks5h 必须把域名发给代理");
                let mut length = [0];
                client.read_exact(&mut length).unwrap();
                let mut host = vec![0; length[0] as usize];
                client.read_exact(&mut host).unwrap();
                let mut port = [0; 2];
                client.read_exact(&mut port).unwrap();
                hosts.lock().unwrap().push(String::from_utf8(host).unwrap());
                let mut target = TcpStream::connect(origin_addr).unwrap();
                client
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                    .unwrap();
                let (mut incoming, mut outbound) =
                    (client.try_clone().unwrap(), target.try_clone().unwrap());
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut incoming, &mut outbound);
                });
                let _ = std::io::copy(&mut target, &mut client);
            });
        }
    });
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine
        .set_network_policy(NetworkPolicy {
            proxy: Some(ProxyConfig {
                url: proxy_url,
                bypass: vec![],
            }),
            ..policy()
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = engine
        .add(options(
            "http://private.invalid/file.bin".into(),
            dir.path(),
            "socks.bin",
        ))
        .unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::Completed);
    assert!(hosts.lock().unwrap().len() >= 2);
    assert!(
        hosts
            .lock()
            .unwrap()
            .iter()
            .all(|host| host == "private.invalid")
    );
    assert_eq!(
        std::fs::read(dir.path().join("socks.bin")).unwrap(),
        vec![42; 32768]
    );
}

#[test]
fn configured_request_and_read_timeouts_bound_real_stalled_transfers() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for request_timeout in [true, false] {
        let server = serve(move |request, stream| {
            if request.header("range") == Some("bytes=0-0") {
                content(&request, stream);
                return;
            }
            let _=stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 32768\r\nContent-Range: bytes 0-32767/32768\r\nConnection: close\r\n\r\n");
            for _ in 0..4 {
                if stream.write_all(&[42; 8192]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(if request_timeout {
                    450
                } else {
                    1500
                }));
            }
        });
        let engine = Engine::new(
            rt.handle().clone(),
            Config {
                connections: 1,
                ..Config::default()
            },
        );
        engine
            .set_network_policy(NetworkPolicy {
                request_timeout_secs: request_timeout.then_some(1),
                read_timeout_secs: if request_timeout { 5 } else { 1 },
                ..policy()
            })
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let id = engine
            .add(options(server.url.clone(), dir.path(), "timeout.bin"))
            .unwrap();
        assert!(matches!(terminal(&engine, id), TaskStatus::Failed(_)));
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert!(engine.snapshot_one(id).unwrap().downloaded < 32768);
    }
}

#[test]
fn a_partial_non_range_response_retries_from_zero_without_corrupting_the_file() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bytes = Arc::new((0..32768).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    let body = bytes.clone();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = attempts.clone();
    let server = serve(move |request, stream| {
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 32768\r\nConnection: close\r\n\r\n");
        if request.header("range").is_some() {
            return;
        }
        let attempt = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = stream.write_all(if attempt == 0 { &body[..8192] } else { &body });
    });
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine
        .set_network_policy(NetworkPolicy {
            retries: 1,
            retry_delay_ms: 10,
            retry_max_delay_ms: 10,
            ..policy()
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = engine
        .add(options(server.url, dir.path(), "non-range.bin"))
        .unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::Completed);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(dir.path().join("non-range.bin")).unwrap(),
        *bytes
    );
    assert_eq!(
        engine.quota().used_bytes,
        32768 + 8192,
        "实际重试写入同样计费"
    );
}

#[test]
fn refreshing_to_an_unknown_length_entity_truncates_the_old_tail() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let old = serve(|request, stream| {
        if request.header("range") == Some("bytes=0-0") {
            content(&request, stream);
            return;
        }
        let _=stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 32768\r\nContent-Range: bytes 0-32767/32768\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(&[42; 8192]);
        std::thread::sleep(Duration::from_millis(300));
        let _ = stream.write_all(&[42; 24576]);
    });
    let new = serve(|_, stream| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(&[7; 1024]);
    });
    let engine = Engine::new(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
    );
    engine.set_network_policy(policy()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = engine
        .add(options(old.url, dir.path(), "unknown.bin"))
        .unwrap();
    wait("旧实体部分写入", || {
        engine.snapshot_one(id).unwrap().downloaded >= 8192
    });
    engine.pause(id).unwrap();
    wait("旧运行退出", || engine.queues()[0].active_count == 0);
    assert_eq!(
        std::fs::metadata(dir.path().join("unknown.bin"))
            .unwrap()
            .len(),
        32768
    );
    engine.update_task_url(id, &new.url, None).unwrap();
    engine.resume(id).unwrap();
    assert_eq!(terminal(&engine, id), TaskStatus::Completed);
    assert_eq!(
        std::fs::read(dir.path().join("unknown.bin")).unwrap(),
        vec![7; 1024]
    );
    assert_eq!(engine.snapshot_one(id).unwrap().total, Some(1024));
}

#[test]
fn failed_sqlite_quota_reservation_prevents_unbilled_writes() {
    use swoop_core::QuotaPolicy;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = serve(|request, stream| content(&request, stream));
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quota-failure.db");
    let store = Arc::new(Store::open(&db).unwrap());
    let engine = Engine::with_store(
        rt.handle().clone(),
        Config {
            connections: 1,
            ..Config::default()
        },
        store.clone(),
    );
    engine.set_network_policy(policy()).unwrap();
    engine
        .set_quota_policy(QuotaPolicy {
            max_bytes: 4096,
            window_secs: 3600,
        })
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER deny_quota BEFORE INSERT ON settings WHEN NEW.key='core.traffic_quota' BEGIN SELECT RAISE(ABORT, 'quota write blocked'); END;").unwrap();
    let id = engine
        .add(options(server.url, dir.path(), "quota-failure.bin"))
        .unwrap();
    let state = terminal(&engine, id);
    assert!(
        matches!(state,TaskStatus::Failed(ref error) if error.contains("quota write blocked")),
        "{state:?}"
    );
    assert_eq!(engine.snapshot_one(id).unwrap().downloaded, 0);
    assert_eq!(engine.quota().used_bytes, 0);
    assert_eq!(engine.quota().remaining_bytes, Some(4096));
    assert!(
        std::fs::read(dir.path().join("quota-failure.bin"))
            .unwrap()
            .iter()
            .all(|byte| *byte == 0),
        "只允许预分配，拒绝未计费写入"
    );
}
