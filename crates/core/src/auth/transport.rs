//! Bounded streaming libcurl transport. Redirects remain owned by NetworkRuntime.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use curl::easy::{Auth, Easy2, Handler, List, WriteError};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::{mpsc, oneshot};
use url::Url;

use super::{AuthSecret, CredentialStorage, HttpAuthScheme, ProxyAuthScheme, SiteAuthRule};
use crate::network::NetworkPolicy;
use crate::{Error, Result};

type Head = (StatusCode, HeaderMap);
pub(crate) struct CurlResponse {
    status: StatusCode,
    headers: HeaderMap,
    url: Url,
    chunks: mpsc::Receiver<Bytes>,
    done: Option<oneshot::Receiver<Result<()>>>,
    cancel: Arc<AtomicBool>,
}
impl CurlResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        if let Some(chunk) = self.chunks.recv().await {
            return Ok(Some(chunk));
        }
        if let Some(done) = self.done.take() {
            done.await.map_err(|_| Error::Authentication("认证请求工作线程异常退出".into()))??;
        }
        Ok(None)
    }
}
impl Drop for CurlResponse {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
struct CancelOnDrop(Option<Arc<AtomicBool>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.store(true, Ordering::Release);
        }
    }
}

struct Receiver {
    head: Option<oneshot::Sender<Result<Head>>>,
    chunks: mpsc::Sender<Bytes>,
    status: StatusCode,
    headers: HeaderMap,
    header_bytes: usize,
    cancel: Arc<AtomicBool>,
    last_activity: Instant,
    read_timeout: Option<Duration>,
    read_timed_out: bool,
}
impl Receiver {
    fn send_head(&mut self) -> bool {
        self.head.take().is_none_or(|sender| sender.send(Ok((self.status, self.headers.clone()))).is_ok())
    }
}
impl Handler for Receiver {
    fn header(&mut self, data: &[u8]) -> bool {
        self.last_activity = Instant::now();
        if self.cancel.load(Ordering::Acquire) {
            return false;
        }
        if data.starts_with(b"HTTP/") {
            self.headers.clear();
            self.header_bytes = 0;
            let Some(status) = data
                .split(|b| *b == b' ')
                .nth(1)
                .and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.parse::<u16>().ok())
                .and_then(|s| StatusCode::from_u16(s).ok())
            else {
                return false;
            };
            self.status = status;
        } else {
            self.header_bytes += data.len();
            if self.header_bytes > 64 * 1024 {
                return false;
            }
            if let Some(colon) = data.iter().position(|b| *b == b':') {
                let Ok(name) = HeaderName::from_bytes(&data[..colon]) else {
                    return false;
                };
                let value = data[colon + 1..].trim_ascii();
                let Ok(value) = HeaderValue::from_bytes(value) else {
                    return false;
                };
                self.headers.append(name, value);
            }
        }
        true
    }
    fn write(&mut self, data: &[u8]) -> std::result::Result<usize, WriteError> {
        // Authentication challenges may carry bodies. Only the final response is exposed.
        if self.status == StatusCode::UNAUTHORIZED || self.status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
            self.last_activity = Instant::now();
            return Ok(data.len());
        }
        if self.cancel.load(Ordering::Acquire)
            || !self.send_head()
            || self.chunks.blocking_send(Bytes::copy_from_slice(data)).is_err()
        {
            return Ok(0);
        }
        self.last_activity = Instant::now();
        Ok(data.len())
    }
    fn progress(&mut self, _: f64, _: f64, _: f64, _: f64) -> bool {
        self.read_timed_out = self.read_timeout.is_some_and(|timeout| self.last_activity.elapsed() >= timeout);
        !self.cancel.load(Ordering::Acquire) && !self.chunks.is_closed() && !self.read_timed_out
    }
}

pub(crate) async fn get(
    policy: NetworkPolicy,
    user_agent: String,
    url: Url,
    headers: HeaderMap,
    site: Option<(SiteAuthRule, Option<Arc<AuthSecret>>)>,
    proxy_secret: Option<Arc<AuthSecret>>,
) -> Result<CurlResponse> {
    get_with_idle_timeout(policy, user_agent, url, headers, site, proxy_secret, true).await
}

pub(crate) async fn get_with_idle_timeout(
    policy: NetworkPolicy,
    user_agent: String,
    url: Url,
    headers: HeaderMap,
    site: Option<(SiteAuthRule, Option<Arc<AuthSecret>>)>,
    proxy_secret: Option<Arc<AuthSecret>>,
    idle_timeout: bool,
) -> Result<CurlResponse> {
    let (head_tx, head_rx) = oneshot::channel();
    let (chunks_tx, chunks_rx) = mpsc::channel(2);
    let (done_tx, done_rx) = oneshot::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut guard = CancelOnDrop(Some(cancel.clone()));
    let receiver = Receiver {
        head: Some(head_tx),
        chunks: chunks_tx,
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        header_bytes: 0,
        cancel: cancel.clone(),
        last_activity: Instant::now(),
        read_timeout: idle_timeout.then(|| Duration::from_secs(policy.read_timeout_secs)),
        read_timed_out: false,
    };
    let target = url.clone();
    tokio::task::spawn_blocking(move || {
        let mut easy = Easy2::new(receiver);
        let result =
            configure(&mut easy, &policy, &user_agent, &target, &headers, site.as_ref(), proxy_secret.as_deref())
                .and_then(|()| easy.perform().map_err(curl_error));
        let receiver = easy.get_mut();
        let result = if receiver.read_timed_out { Err(Error::AuthTransport(28)) } else { result };
        if let Err(error) = result {
            // The head and body paths are exclusive; no server error buffer / URL is disclosed.
            if let Some(head) = receiver.head.take() {
                let _ = head.send(Err(error));
            } else {
                let _ = done_tx.send(Err(error));
            }
        } else {
            receiver.send_head();
            let _ = done_tx.send(Ok(()));
        }
    });
    let (status, headers) = head_rx.await.map_err(|_| Error::Authentication("认证请求工作线程异常退出".into()))??;
    // Do not cancel the live response when transferring ownership from the pre-header guard.
    guard.0 = None;
    Ok(CurlResponse { status, headers, url, chunks: chunks_rx, done: Some(done_rx), cancel })
}

fn configure(
    easy: &mut Easy2<Receiver>,
    policy: &NetworkPolicy,
    agent: &str,
    url: &Url,
    headers: &HeaderMap,
    site: Option<&(SiteAuthRule, Option<Arc<AuthSecret>>)>,
    proxy_secret: Option<&AuthSecret>,
) -> Result<()> {
    easy.url(url.as_str()).map_err(curl_error)?;
    easy.get(true).map_err(curl_error)?;
    easy.follow_location(false).map_err(curl_error)?;
    easy.unrestricted_auth(false).map_err(curl_error)?;
    if let Some((rule, secret)) = site {
        easy.http_auth(&http_auth(rule.scheme)).map_err(curl_error)?;
        if rule.storage == CredentialStorage::CurrentUser {
            easy.username("").map_err(curl_error)?;
            easy.password("").map_err(curl_error)?;
        } else {
            let secret = secret.as_ref().ok_or_else(|| Error::Authentication("等待站点认证密码".into()))?;
            easy.username(&rule.username).map_err(curl_error)?;
            easy.password(secret.expose()).map_err(curl_error)?;
        }
    }
    easy.useragent(agent).map_err(curl_error)?;
    easy.connect_timeout(Duration::from_secs(policy.connect_timeout_secs)).map_err(curl_error)?;
    if let Some(seconds) = policy.request_timeout_secs {
        easy.timeout(Duration::from_secs(seconds)).map_err(curl_error)?;
    }
    easy.progress(true).map_err(curl_error)?;
    easy.ssl_verify_peer(true).map_err(curl_error)?;
    easy.ssl_verify_host(true).map_err(curl_error)?;
    easy.proxy_ssl_verify_peer(true).map_err(curl_error)?;
    easy.proxy_ssl_verify_host(true).map_err(curl_error)?;
    if let Some(proxy) = &policy.proxy {
        easy.proxy(proxy.url.trim()).map_err(curl_error)?;
        easy.noproxy(&proxy.bypass.join(",")).map_err(curl_error)?;
        if let Some(rule) = &policy.proxy_auth {
            let scheme = match rule.scheme {
                ProxyAuthScheme::Basic | ProxyAuthScheme::Socks5 => HttpAuthScheme::Basic,
                ProxyAuthScheme::Digest => HttpAuthScheme::Digest,
                ProxyAuthScheme::Ntlm => HttpAuthScheme::Ntlm,
                ProxyAuthScheme::Negotiate => HttpAuthScheme::Negotiate,
            };
            easy.proxy_auth(&http_auth(scheme)).map_err(curl_error)?;
            if rule.storage == CredentialStorage::CurrentUser {
                easy.proxy_username("").map_err(curl_error)?;
                easy.proxy_password("").map_err(curl_error)?;
            } else {
                let secret = proxy_secret.ok_or_else(|| Error::Authentication("等待代理认证密码".into()))?;
                easy.proxy_username(&rule.username).map_err(curl_error)?;
                easy.proxy_password(secret.expose()).map_err(curl_error)?;
            }
        }
    } else if !policy.use_system_proxy {
        easy.proxy("").map_err(curl_error)?;
    }
    let mut list = List::new();
    for (key, value) in headers {
        let value = value.to_str().map_err(|_| Error::InvalidHeader(key.to_string()))?;
        list.append(&format!("{}: {}", key.as_str(), value)).map_err(curl_error)?;
    }
    easy.http_headers(list).map_err(curl_error)?;
    Ok(())
}
fn http_auth(scheme: HttpAuthScheme) -> Auth {
    let mut auth = Auth::new();
    match scheme {
        HttpAuthScheme::Basic => {
            auth.basic(true);
        }
        HttpAuthScheme::Digest => {
            auth.digest(true);
        }
        HttpAuthScheme::Ntlm => {
            auth.ntlm(true);
        }
        HttpAuthScheme::Negotiate => {
            auth.gssnegotiate(true);
        }
    }
    auth
}
fn curl_error(error: curl::Error) -> Error {
    // CURLcode 在 MSVC 上是 i32；对外保持既有的非负错误码类型。
    let code = error.code();
    #[cfg(target_env = "msvc")]
    let code = code as u32;
    Error::AuthTransport(code)
}
