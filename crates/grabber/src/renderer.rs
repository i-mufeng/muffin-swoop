//! Explicit, isolated Chromium execution with a project-scoped local proxy.
use std::process::Stdio;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{Context, FetchError, allowed, canonical, robots_allowed};

pub(super) async fn render(context: &Arc<Context>, url: &Url) -> Result<Vec<u8>, FetchError> {
    let options = &context.config.render;
    let profile = tempfile::Builder::new()
        .prefix("swoop-render-")
        .tempdir()
        .map_err(|error| FetchError::Failed(format!("无法创建隔离浏览器目录：{error}")))?;
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(failed)?;
    let address = listener.local_addr().map_err(failed)?;
    let stop = context.stop.child_token();
    let proxy_stop = stop.clone();
    let proxy_context = context.clone();
    let proxy = tokio::spawn(async move { serve_proxy(listener, proxy_context, proxy_stop).await });
    let mut command = Command::new(&options.executable);
    command
        .args([
            "--headless=new",
            "--dump-dom",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-sync",
            "--disable-extensions",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-breakpad",
            "--disable-quic",
            "--disable-features=MediaRouter,OptimizationHints,AutofillServerCommunication,WebTransport,DirectSockets",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
            "--proxy-bypass-list=<-loopback>",
            "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(format!("--proxy-server=http://{address}"))
        .arg(format!("--virtual-time-budget={}", options.virtual_time_ms))
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let child = command.spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            stop.cancel();
            let _ = proxy.await;
            return Err(FetchError::Failed(format!("无法启动动态渲染浏览器：{error}")));
        }
    };
    let mut output = child
        .stdout
        .take()
        .expect("piped browser stdout")
        .take(options.max_dom_bytes.min(context.config.max_file_bytes) + 1);
    let mut error_output = child.stderr.take().expect("piped browser stderr");
    let stderr = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut error_output, &mut tokio::io::sink()).await;
    });
    let operation = async {
        let mut dom = Vec::new();
        let read = async {
            output.read_to_end(&mut dom).await.map_err(failed)?;
            if dom.len() as u64 > options.max_dom_bytes.min(context.config.max_file_bytes) {
                return Err(FetchError::Failed("动态渲染 DOM 超过配置上限".into()));
            }
            Ok(())
        };
        let wait = async { child.wait().await.map_err(failed) };
        let (_, status) = tokio::try_join!(read, wait)?;
        if !status.success() {
            return Err(FetchError::Failed(format!("动态渲染浏览器退出：{status}")));
        }
        let text = std::str::from_utf8(&dom).map_err(|_| FetchError::Failed("动态渲染输出不是 UTF-8 HTML".into()))?;
        if !text.to_ascii_lowercase().contains("<html") || text.contains("chrome-error://chromewebdata") {
            return Err(FetchError::Failed("动态渲染未取得有效页面 DOM".into()));
        }
        Ok(dom)
    };
    let result = tokio::select! {
        biased;
        _ = context.stop.cancelled() => Err(FetchError::Cancelled),
        _ = stop.cancelled() => Err(FetchError::Failed("动态渲染浏览器流量超过配置上限".into())),
        result = tokio::time::timeout(Duration::from_secs(options.timeout_secs), operation) => {
            result.unwrap_or_else(|_| Err(FetchError::Failed("动态渲染超时".into())))
        }
    };
    // Terminate our own isolated process group/tree and wait for the direct child;
    // this also covers cancellation while stdout or a tunnel is still open.
    if result.is_err() {
        terminate(&mut child).await;
    }
    stop.cancel();
    let _ = proxy.await;
    stderr.abort();
    let _ = stderr.await;
    result
}

async fn terminate(child: &mut Child) {
    if let Some(id) = child.id() {
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &id.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
fn failed(error: impl std::fmt::Display) -> FetchError {
    FetchError::Failed(format!("动态渲染：{error}"))
}
fn charge(context: &Context, count: usize, stop: &CancellationToken) -> std::io::Result<()> {
    if context
        .rendered_used
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(count as u64).filter(|used| *used <= context.config.render.max_network_bytes)
        })
        .is_err()
    {
        stop.cancel();
        return Err(std::io::Error::other("browser response budget exceeded"));
    }
    Ok(())
}
async fn serve_proxy(listener: TcpListener, context: Arc<Context>, stop: CancellationToken) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            connection = listener.accept(), if connections.len() < 32 => {
                let Ok((socket, _)) = connection else { break; };
                let context = context.clone();
                let stop = stop.clone();
                connections.spawn(async move {
                    tokio::select! {
                        _ = stop.cancelled() => {},
                        _ = tokio::time::timeout(Duration::from_secs(context.config.render.timeout_secs), handle(socket, &context, &stop)) => {},
                    }
                });
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}
async fn deny(socket: &mut TcpStream) -> std::io::Result<()> {
    socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await
}
async fn handle(mut socket: TcpStream, context: &Context, stop: &CancellationToken) -> std::io::Result<()> {
    let mut headers = Vec::new();
    let mut byte = [0];
    while !headers.ends_with(b"\r\n\r\n") {
        if headers.len() >= 16384 || socket.read(&mut byte).await? == 0 {
            return Ok(());
        }
        headers.push(byte[0]);
    }
    let headers = std::str::from_utf8(&headers).map_err(std::io::Error::other)?;
    let mut first = headers.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let target = first.next().unwrap_or("");
    if method == "CONNECT" {
        let Ok(url) = canonical(&format!("https://{target}/")) else {
            return deny(&mut socket).await;
        };
        if !allowed(context, &url) {
            return deny(&mut socket).await;
        }
        let mut upstream =
            TcpStream::connect((url.host_str().unwrap_or(""), url.port_or_known_default().unwrap_or(443))).await?;
        socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        let (mut browser_read, mut browser_write) = socket.split();
        let (mut server_read, mut server_write) = upstream.split();
        let upload = tokio::io::copy(&mut browser_read, &mut server_write);
        let download = async {
            let mut buffer = [0u8; 16384];
            loop {
                let size = server_read.read(&mut buffer).await?;
                if size == 0 {
                    break;
                }
                charge(context, size, stop)?;
                browser_write.write_all(&buffer[..size]).await?;
            }
            Ok::<_, std::io::Error>(())
        };
        tokio::select! { _ = upload => {}, _ = download => {} }
        return Ok(());
    }
    if !matches!(method, "GET" | "HEAD") {
        return deny(&mut socket).await;
    }
    let Ok(url) = canonical(target) else {
        return deny(&mut socket).await;
    };
    if url.scheme() != "http" || !allowed(context, &url) || robots_allowed(context, &url).await.is_err() {
        return deny(&mut socket).await;
    }
    // Browser cookies/auth are not forwarded over HTTP. HTTPS runs only in the
    // independent profile; CONNECT can constrain origin but not encrypted paths.
    let request = if method == "HEAD" { context.client.head(url) } else { context.client.get(url) };
    let mut response = request.send().await.map_err(std::io::Error::other)?;
    let mut response_headers = format!("HTTP/1.1 {} Render\r\nConnection: close\r\n", response.status().as_u16());
    for (name, value) in response.headers() {
        if ["connection", "transfer-encoding", "set-cookie", "proxy-authenticate"].contains(&name.as_str()) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            response_headers.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    response_headers.push_str("\r\n");
    charge(context, response_headers.len(), stop)?;
    socket.write_all(response_headers.as_bytes()).await?;
    while let Some(bytes) = response.chunk().await.map_err(std::io::Error::other)? {
        charge(context, bytes.len(), stop)?;
        socket.write_all(&bytes).await?;
    }
    Ok(())
}
