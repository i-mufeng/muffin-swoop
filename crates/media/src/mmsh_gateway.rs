//! MMSH describe/play relay. FFmpeg only receives a loopback capability URL;
//! credentials and shared throughput control stay in the normal media transport.
use crate::{MediaError, MediaSessionAuth, network::NetworkTransport};
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
    header,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use reqwest::header::HeaderMap;
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;
use url::Url;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;
struct Shared {
    transport: NetworkTransport,
    source: Url,
    headers: HeaderMap,
    auth: MediaSessionAuth,
    cancel: CancellationToken,
    host: String,
    path: String,
    failed: AtomicBool,
}
pub(crate) struct MmshGateway {
    shared: Arc<Shared>,
    input: String,
    server: Option<JoinHandle<()>>,
}
impl MmshGateway {
    pub async fn start(
        transport: NetworkTransport,
        mut source: Url,
        headers: HeaderMap,
        auth: MediaSessionAuth,
        cancel: CancellationToken,
    ) -> Result<Self, MediaError> {
        // url::Url cannot switch a non-special scheme to a special scheme in-place.
        if source.scheme() == "mmsh" {
            source = Url::parse(&format!("http:{}", &source.as_str()[5..])).map_err(|_| MediaError::InvalidUrl)?;
        }
        let source = crate::network_url(source.as_str())?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let host = listener.local_addr()?.to_string();
        let path = format!("/{}/stream", uuid::Uuid::new_v4().simple());
        let input = format!("mmsh://{host}{path}");
        let requests = Arc::new(Semaphore::new(transport.max_concurrent_requests));
        let shared =
            Arc::new(Shared { transport, source, headers, auth, cancel, host, path, failed: AtomicBool::new(false) });
        let state = shared.clone();
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((socket, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Ok(permit) = requests.clone().try_acquire_owned() else { continue; };
                        let shared = state.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            let service_state = shared.clone();
                            let connection = hyper::server::conn::http1::Builder::new()
                                .timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(10))
                                .keep_alive(false).max_buf_size(16 * 1024)
                                .serve_connection(TokioIo::new(socket), hyper::service::service_fn(move |request| serve(request, service_state.clone())));
                            tokio::select! { _ = shared.cancel.cancelled() => {}, _ = connection => {} }
                        });
                    },
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self { shared, input, server: Some(server) })
    }
    pub fn input_url(&self) -> &str {
        &self.input
    }
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }
    pub async fn shutdown(&mut self) {
        self.shared.cancel.cancel();
        if let Some(server) = self.server.take() {
            let _ = server.await;
        }
    }
}
impl Drop for MmshGateway {
    fn drop(&mut self) {
        self.shared.cancel.cancel();
    }
}
fn empty(status: StatusCode) -> Response<Body> {
    let mut response =
        Response::new(Full::new(Bytes::new()).map_err(|never: Infallible| match never {}).boxed_unsync());
    *response.status_mut() = status;
    response
}
async fn serve(request: Request<Incoming>, shared: Arc<Shared>) -> Result<Response<Body>, Infallible> {
    if request.method() != Method::GET
        || request.uri().path() != shared.path
        || request.uri().query().is_some()
        || request.headers().get(header::HOST).and_then(|v| v.to_str().ok()) != Some(shared.host.as_str())
    {
        return Ok(empty(StatusCode::NOT_FOUND));
    }
    let mut headers = shared.headers.clone();
    // The upstream address is fixed, and only FFmpeg's public MMSH negotiation
    // fields are forwarded. Authorization/Cookie/Host never come from loopback.
    for key in [header::USER_AGENT, header::ACCEPT, header::PRAGMA, header::RANGE] {
        if request.headers().contains_key(&key) {
            headers.remove(&key);
            for value in request.headers().get_all(&key) {
                headers.append(key.clone(), value.clone());
            }
        }
    }
    let upstream = match shared.transport.get_mmsh(shared.source.clone(), &headers, &shared.auth, &shared.cancel).await
    {
        Ok(response) if response.status().is_success() => response,
        _ => {
            if !shared.cancel.is_cancelled() {
                shared.failed.store(true, Ordering::Release);
            }
            return Ok(empty(StatusCode::BAD_GATEWAY));
        }
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let stream_state = shared.clone();
    let stream = upstream.bytes_stream().map(move |chunk| match chunk {
        Ok(bytes) => Ok(Frame::data(bytes)),
        Err(_) => {
            if !stream_state.cancel.is_cancelled() {
                stream_state.failed.store(true, Ordering::Release);
            }
            Err(std::io::Error::other("MMSH upstream failed"))
        }
    });
    let mut response = Response::new(StreamBody::new(stream).boxed_unsync());
    *response.status_mut() = status;
    for key in [header::CONTENT_TYPE, header::CONTENT_LENGTH, header::CONTENT_RANGE, header::PRAGMA] {
        for value in upstream_headers.get_all(&key) {
            response.headers_mut().append(key.clone(), value.clone());
        }
    }
    Ok(response)
}
