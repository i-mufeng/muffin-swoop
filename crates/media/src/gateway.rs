//! Per-download loopback capability server. Upstream URLs/credentials never
//! appear in FFmpeg arguments or the served playlists.
use crate::{MediaConfig, MediaError, MediaSessionAuth, manifest, network::NetworkTransport, network_url};
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
    collections::{HashMap, HashSet},
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;
use url::Url;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Manifest,
    Key,
    Media,
}
#[derive(Clone)]
struct Resource {
    url: Url,
    kind: Kind,
    depth: usize,
    route: String,
    body: Option<Bytes>,
    template: Option<crate::dash_gateway::Template>,
    refresh: bool,
    pinned_once: bool,
    content_type: &'static str,
}
pub(crate) struct Catalog {
    base: String,
    resources: Vec<Resource>,
    urls: HashMap<String, usize>,
    max_resources: usize,
    max_playlists: usize,
    expanded: HashSet<String>,
    pub(crate) dash_timelines: HashMap<String, crate::dash_gateway::LiveTimeline>,
    pub(crate) max_timeline_segments: usize,
    pub(crate) timeline_segments: usize,
}
impl Catalog {
    pub(crate) fn register(&mut self, url: Url, kind: Kind, depth: usize) -> Result<String, MediaError> {
        self.register_inner(url, kind, depth, false)
    }
    pub(crate) fn register_dash(&mut self, url: Url) -> Result<String, MediaError> {
        self.register_inner(url, Kind::Media, 0, true)
    }
    fn register_inner(
        &mut self,
        url: Url,
        kind: Kind,
        depth: usize,
        allow_template: bool,
    ) -> Result<String, MediaError> {
        network_url(url.as_str())?;
        let key = format!("{allow_template}:{url}");
        if let Some(index) = self.urls.get(&key) {
            let resource = &self.resources[*index];
            if resource.kind != kind {
                return Err(MediaError::UnsupportedGatewayMedia);
            }
            let suffix = if allow_template {
                crate::dash_gateway::Template::from_url(&url)?.map(|(_, suffix)| suffix).unwrap_or_default()
            } else {
                String::new()
            };
            return Ok(format!("{}{}{suffix}", self.base, resource.route));
        }
        if self.resources.len().saturating_add(self.expanded.len()) >= self.max_resources
            || depth > 4
            || (kind == Kind::Manifest
                && self.resources.iter().filter(|resource| resource.kind == Kind::Manifest).count()
                    >= self.max_playlists)
        {
            return Err(MediaError::ManifestLimit);
        }
        let suffix = match kind {
            Kind::Manifest => {
                if url.path().ends_with(".mpd") {
                    "mpd"
                } else {
                    "m3u8"
                }
            }
            Kind::Key => "key",
            Kind::Media => url
                .path()
                .rsplit('.')
                .next()
                .filter(|extension| {
                    matches!(*extension, "ts" | "m4s" | "mp4" | "aac" | "ac3" | "ec3" | "mp3" | "vtt" | "webvtt")
                })
                .unwrap_or("ts"),
        };
        let route = format!("/{}/asset.{suffix}", uuid::Uuid::new_v4().simple());
        let (template, template_suffix) = if allow_template {
            match crate::dash_gateway::Template::from_url(&url)? {
                Some((template, suffix)) => (Some(template), suffix),
                None => (None, String::new()),
            }
        } else {
            (None, String::new())
        };
        let local = format!("{}{route}{template_suffix}", self.base);
        self.urls.insert(key, self.resources.len());
        self.resources.push(Resource {
            url,
            kind,
            depth,
            route,
            body: None,
            template,
            refresh: false,
            pinned_once: false,
            content_type: "application/vnd.apple.mpegurl",
        });
        Ok(local)
    }
}

fn resolve(base: &Url, raw: &str) -> Result<Url, MediaError> {
    if raw.contains(['\r', '\n', '\0']) || raw.contains("{$") {
        return Err(MediaError::UnsupportedGatewayMedia);
    }
    let url = base.join(raw).map_err(|_| MediaError::InvalidUrl)?;
    network_url(url.as_str())
}

fn rewrite(text: &str, base: &Url, depth: usize, catalog: &mut Catalog) -> Result<Bytes, MediaError> {
    manifest::parse_hls(text, base)?;
    let mut output = String::new();
    let mut variant_next = false;
    for line in text.lines().map(str::trim) {
        let rewritten = if line.starts_with("#EXT-X-") {
            let (tag, raw) = line.split_once(':').unwrap_or((line, ""));
            match tag {
                "#EXT-X-KEY" | "#EXT-X-SESSION-KEY" | "#EXT-X-MAP" | "#EXT-X-MEDIA" | "#EXT-X-I-FRAME-STREAM-INF" => {
                    let attributes = manifest::attributes(raw)?;
                    if let Some((_, uri)) = attributes.iter().find(|(name, _)| *name == "URI") {
                        let kind = match tag {
                            "#EXT-X-KEY" | "#EXT-X-SESSION-KEY" => Kind::Key,
                            "#EXT-X-MAP" => Kind::Media,
                            _ => Kind::Manifest,
                        };
                        let local =
                            catalog.register(resolve(base, uri)?, kind, depth + usize::from(kind == Kind::Manifest))?;
                        // Preserve every non-URI attribute exactly, including
                        // quoting and commas in names/codecs.
                        let mut quoted = false;
                        let mut start = 0;
                        let mut fields = Vec::new();
                        for (index, character) in raw.char_indices().chain(std::iter::once((raw.len(), ','))) {
                            if character == '"' {
                                quoted = !quoted;
                            }
                            if character == ',' && !quoted {
                                let field = raw[start..index].trim();
                                fields.push(if field.split_once('=').is_some_and(|(name, _)| name.trim() == "URI") {
                                    format!("URI=\"{local}\"")
                                } else {
                                    field.to_owned()
                                });
                                start = index + 1;
                            }
                        }
                        format!("{tag}:{}", fields.join(","))
                    } else {
                        line.to_owned()
                    }
                }
                "#EXT-X-STREAM-INF" => {
                    variant_next = true;
                    line.to_owned()
                }
                "#EXT-X-VERSION"
                | "#EXT-X-TARGETDURATION"
                | "#EXT-X-MEDIA-SEQUENCE"
                | "#EXT-X-DISCONTINUITY-SEQUENCE"
                | "#EXT-X-ENDLIST"
                | "#EXT-X-PLAYLIST-TYPE"
                | "#EXT-X-BYTERANGE"
                | "#EXT-X-DISCONTINUITY"
                | "#EXT-X-PROGRAM-DATE-TIME"
                | "#EXT-X-INDEPENDENT-SEGMENTS"
                | "#EXT-X-START"
                | "#EXT-X-I-FRAMES-ONLY" => line.to_owned(),
                // Unknown tags may introduce fetches (steering, variables,
                // session data, LL-HLS parts). Never pass them through unchecked.
                _ => return Err(MediaError::UnsupportedGatewayMedia),
            }
        } else if !line.is_empty() && !line.starts_with('#') {
            let kind = if variant_next { Kind::Manifest } else { Kind::Media };
            variant_next = false;
            catalog.register(resolve(base, line)?, kind, depth + usize::from(kind == Kind::Manifest))?
        } else {
            line.to_owned()
        };
        output.push_str(&rewritten);
        output.push('\n');
    }
    Ok(Bytes::from(output))
}

fn rewrite_manifest(
    text: &str,
    base: &Url,
    depth: usize,
    catalog: &mut Catalog,
) -> Result<(Bytes, bool, &'static str), MediaError> {
    if text.starts_with("#EXTM3U") {
        let (info, _) = manifest::parse_hls(text, base)?;
        Ok((rewrite(text, base, depth, catalog)?, info.is_live, "application/vnd.apple.mpegurl"))
    } else {
        let text = crate::subtitle::filter_dash_for_ffmpeg(text)?;
        let (bytes, live) = crate::dash_gateway::rewrite(&text, base, catalog)?;
        Ok((Bytes::from(bytes), live, "application/dash+xml"))
    }
}

struct Shared {
    transport: NetworkTransport,
    headers: HeaderMap,
    auth: MediaSessionAuth,
    catalog: Mutex<Catalog>,
    config: Arc<MediaConfig>,
    prefix: String,
    live_deadline: Option<std::time::Instant>,
    host: String,
    cancel: CancellationToken,
    failed: AtomicBool,
    requests: Arc<Semaphore>,
    next_manifest_refresh: tokio::sync::Mutex<HashMap<String, tokio::time::Instant>>,
}

pub(crate) struct Gateway {
    shared: Arc<Shared>,
    server: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.shared.cancel.cancel();
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

impl Gateway {
    /// Serve this verified live window once before normal refreshes resume.
    /// Used to align independently collected text subtitles with FFmpeg's
    /// first video segment. The source text remains only in memory.
    #[allow(dead_code)] // 字幕转换接入前只有测试调用
    pub fn pin_hls_manifest(&mut self, upstream: &str, text: &str) -> Result<(), MediaError> {
        if !text.trim_start().starts_with("#EXTM3U") {
            return Err(MediaError::InvalidManifest);
        }
        let url = network_url(upstream)?;
        let mut catalog = self.shared.catalog.lock().unwrap();
        let index = catalog
            .resources
            .iter()
            .position(|resource| resource.kind == Kind::Manifest && resource.url == url)
            .ok_or(MediaError::InvalidManifest)?;
        let depth = catalog.resources[index].depth;
        let (body, refresh, content_type) = rewrite_manifest(text.trim(), &url, depth, &mut catalog)?;
        let resource = &mut catalog.resources[index];
        resource.body = Some(body);
        resource.refresh = refresh;
        resource.content_type = content_type;
        resource.pinned_once = true;
        Ok(())
    }
    pub async fn start(
        transport: NetworkTransport,
        config: Arc<MediaConfig>,
        headers: HeaderMap,
        auth: MediaSessionAuth,
        inputs: &mut [String],
        recording_duration: Option<std::time::Duration>,
        cancel: CancellationToken,
    ) -> Result<Self, MediaError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let host = listener.local_addr()?.to_string();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut catalog = Catalog {
            base: format!("http://{host}/{token}"),
            resources: Vec::new(),
            urls: HashMap::new(),
            max_resources: transport.max_resources,
            max_playlists: config.max_playlists,
            expanded: HashSet::new(),
            dash_timelines: HashMap::new(),
            max_timeline_segments: transport.max_resources,
            timeline_segments: 0,
        };
        for input in inputs {
            *input = catalog.register(network_url(input)?, Kind::Manifest, 0)?;
        }
        let mut index = 0;
        let mut playlists = 0;
        while index < catalog.resources.len() {
            let resource = &catalog.resources[index];
            if resource.kind == Kind::Manifest {
                playlists += 1;
                if playlists > config.max_playlists {
                    return Err(MediaError::ManifestLimit);
                }
                let mut response = transport.get(resource.url.clone(), &headers, &auth, &cancel).await?;
                if !response.status().is_success() {
                    return Err(MediaError::HttpStatus(response.status().as_u16()));
                }
                let final_url = network_url(response.url().as_str())?;
                let bytes = bounded_body(&mut response, config.max_manifest_bytes, &cancel).await?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| MediaError::InvalidManifest)?
                    .trim_start_matches('\u{feff}')
                    .trim();
                let depth = resource.depth;
                let (body, refresh, content_type) = rewrite_manifest(text, &final_url, depth, &mut catalog)?;
                catalog.resources[index].body = Some(body);
                catalog.resources[index].refresh = refresh;
                catalog.resources[index].content_type = content_type;
            }
            index += 1;
        }
        let live_deadline = recording_duration.map(|duration| {
            std::time::Instant::now() + duration + std::time::Duration::from_secs(config.live_finalize_grace_secs)
        });
        let shared = Arc::new(Shared {
            requests: Arc::new(Semaphore::new(transport.max_concurrent_requests)),
            next_manifest_refresh: tokio::sync::Mutex::new(HashMap::new()),
            transport,
            headers,
            auth,
            host,
            cancel,
            failed: AtomicBool::new(false),
            live_deadline,
            catalog: Mutex::new(catalog),
            config,
            prefix: format!("/{token}"),
        });
        let server_state = shared.clone();
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = server_state.cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Ok(permit) = server_state.requests.clone().try_acquire_owned() else { continue; };
                        let state = server_state.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            let service_state = state.clone();
                            let connection = hyper::server::conn::http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(std::time::Duration::from_secs(10))
                                .keep_alive(false).max_buf_size(16 * 1024)
                                .serve_connection(TokioIo::new(stream), hyper::service::service_fn(move |request| {
                                    serve(request, service_state.clone())
                                }));
                            tokio::select! { _ = state.cancel.cancelled() => {}, _ = connection => {} }
                        });
                    },
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self { shared, server: Some(server) })
    }
    pub async fn shutdown(&mut self) {
        self.shared.cancel.cancel();
        if let Some(server) = self.server.take() {
            let _ = server.await;
        }
    }
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }
}

async fn bounded_body(
    response: &mut crate::network::NetworkResponse,
    max: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, MediaError> {
    if response.content_length().is_some_and(|size| size > max as u64) {
        return Err(MediaError::ManifestLimit);
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(MediaError::Cancelled),
            result = response.chunk() => result.map_err(|_| MediaError::Network)?,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(MediaError::ManifestLimit);
        }
        bytes.extend_from_slice(&chunk);
    }
}

fn full(status: StatusCode, bytes: Bytes) -> Response<Body> {
    let mut response = Response::new(Full::new(bytes.clone()).map_err(|never| match never {}).boxed_unsync());
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_LENGTH, bytes.len().into());
    response
}

async fn serve(request: Request<Incoming>, shared: Arc<Shared>) -> Result<Response<Body>, Infallible> {
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        || request.uri().authority().is_some()
        || request.uri().query().is_some()
        || request.headers().get(header::HOST).and_then(|value| value.to_str().ok()) != Some(&shared.host)
        || request.headers().contains_key(header::ORIGIN)
        || request.headers().contains_key(header::TRANSFER_ENCODING)
        || request.headers().get(header::CONTENT_LENGTH).is_some_and(|value| value != "0")
    {
        return Ok(full(StatusCode::FORBIDDEN, Bytes::new()));
    }
    let resource = {
        let mut catalog = shared.catalog.lock().unwrap();
        let found = request.uri().path().strip_prefix(&shared.prefix).and_then(|path| {
            catalog.resources.iter().find_map(|resource| {
                if path == resource.route && resource.template.is_none() {
                    return Some(resource.clone());
                }
                let suffix = path.strip_prefix(&resource.route)?.strip_prefix('/')?;
                let mut found = resource.clone();
                found.url = resource.template.as_ref()?.resolve(suffix).ok()?;
                Some(found)
            })
        });
        if let Some(resource) = &found
            && resource.pinned_once
            && request.method() == Method::GET
            && let Some(stored) = catalog.resources.iter_mut().find(|stored| stored.route == resource.route)
        {
            stored.pinned_once = false;
        }
        if let Some(resource) = &found
            && resource.template.is_some()
            && !catalog.expanded.contains(resource.url.as_str())
        {
            if catalog.resources.len().saturating_add(catalog.expanded.len()) >= catalog.max_resources {
                shared.failed.store(true, Ordering::Release);
                return Ok(full(StatusCode::SERVICE_UNAVAILABLE, Bytes::new()));
            }
            catalog.expanded.insert(resource.url.to_string());
        }
        found
    };
    let Some(resource) = resource else {
        return Ok(full(StatusCode::NOT_FOUND, Bytes::new()));
    };
    let result = respond(&request, &resource, shared.clone()).await;
    Ok(match result {
        Ok(response) => response,
        Err(_) => {
            shared.failed.store(true, Ordering::Release);
            full(StatusCode::BAD_GATEWAY, Bytes::new())
        }
    })
}

async fn respond(
    request: &Request<Incoming>,
    resource: &Resource,
    shared: Arc<Shared>,
) -> Result<Response<Body>, MediaError> {
    if resource.kind == Kind::Manifest {
        let empty = Bytes::new();
        let bytes = resource.body.as_ref().unwrap_or(&empty);
        let refreshed;
        let bytes = if !resource.pinned_once && (resource.refresh || resource.body.is_none()) {
            if resource.refresh {
                // FFmpeg's SegmentList reader otherwise busy-polls an unchanged
                // MPD and exhausts max_reload before the next segment exists.
                // Pace each live source without hiding any network errors.
                let mut refreshes = shared.next_manifest_refresh.lock().await;
                if let Some(next) = refreshes.get(&resource.route) {
                    tokio::select! { biased;
                        _ = shared.cancel.cancelled() => return Err(MediaError::Cancelled),
                        _ = tokio::time::sleep_until(*next) => {},
                    }
                }
                refreshes.insert(
                    resource.route.clone(),
                    tokio::time::Instant::now() + shared.transport.live_manifest_refresh,
                );
            }
            let mut upstream =
                shared.transport.get(resource.url.clone(), &shared.headers, &shared.auth, &shared.cancel).await?;
            if !upstream.status().is_success() {
                return Err(MediaError::HttpStatus(upstream.status().as_u16()));
            }
            let base = network_url(upstream.url().as_str())?;
            let body = bounded_body(&mut upstream, shared.config.max_manifest_bytes, &shared.cancel).await?;
            let text = std::str::from_utf8(&body).map_err(|_| MediaError::InvalidManifest)?.trim();
            let finalized;
            let text = if text.starts_with("#EXTM3U")
                && shared.live_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
                let (info, _) = manifest::parse_hls(text, &base)?;
                if info.is_live {
                    finalized = format!("{text}\n#EXT-X-ENDLIST\n");
                    &finalized
                } else {
                    text
                }
            } else {
                text
            };
            let mut catalog = shared.catalog.lock().unwrap();
            refreshed = rewrite_manifest(text, &base, resource.depth, &mut catalog)?.0;
            &refreshed
        } else {
            bytes
        };
        let mut response =
            full(StatusCode::OK, if request.method() == Method::HEAD { Bytes::new() } else { bytes.clone() });
        response.headers_mut().insert(header::CONTENT_LENGTH, bytes.len().into());
        response.headers_mut().insert(header::CONTENT_TYPE, resource.content_type.parse().unwrap());
        return Ok(response);
    }
    let mut headers = shared.headers.clone();
    if let Some(range) = request.headers().get(header::RANGE) {
        let range_text = range.to_str().map_err(|_| MediaError::GatewayFailed)?;
        let range_body = range_text.strip_prefix("bytes=").ok_or(MediaError::GatewayFailed)?;
        let (start, end) = range_body.split_once('-').ok_or(MediaError::GatewayFailed)?;
        if (start.is_empty() && end.is_empty())
            || (!start.is_empty() && start.parse::<u64>().is_err())
            || (!end.is_empty() && end.parse::<u64>().is_err())
        {
            return Err(MediaError::GatewayFailed);
        }
        headers.insert(header::RANGE, range.clone());
    }
    let cache =
        shared.transport.cache.as_ref().filter(|_| resource.kind == Kind::Media && request.method() == Method::GET);
    let cache_key = cache
        .map(|_| crate::cache::Cache::key_scoped(&resource.url, &headers, &shared.transport.cache_scope(&shared.auth)));
    // A file lock covers lookup, network validation and publication across
    // downloader instances and processes. Checkpoint jobs retain this lease
    // even when cancellation drops the surrounding request future.
    let lease = if let (Some(cache), Some(key)) = (cache, cache_key.as_ref()) {
        Some(cache.lease(key, &shared.cancel).await?)
    } else {
        None
    };
    let cached =
        if let (Some(cache), Some(key)) = (cache, cache_key.as_ref()) { cache.read(key.clone()).await } else { None };
    let original_headers = headers.clone();
    let mut partial = if cached.is_none() && shared.transport.resume_partial_segments {
        if let (Some(cache), Some(key), Some(lease)) = (cache, cache_key.as_ref(), lease.as_ref()) {
            cache.read_partial(key.clone(), lease.clone()).await
        } else {
            None
        }
    } else {
        None
    };
    if let Some(prefix) = &partial {
        headers.insert(header::RANGE, prefix.range().parse().map_err(|_| MediaError::GatewayFailed)?);
        headers.insert(header::IF_RANGE, prefix.etag.parse().map_err(|_| MediaError::GatewayFailed)?);
    }
    if let Some(entry) = &cached {
        headers.insert(header::IF_NONE_MATCH, entry.etag.parse().map_err(|_| MediaError::GatewayFailed)?);
    }
    let mut upstream = shared.transport.get(resource.url.clone(), &headers, &shared.auth, &shared.cancel).await?;
    if partial.as_ref().is_some_and(|prefix| !prefix.accepts(upstream.url(), upstream.status(), upstream.headers())) {
        // Includes ignored Range (200), unsatisfied Range (416), validator,
        // total-length, Content-Range and final redirect changes. Never join
        // unverified suffixes; repeat the original request exactly once.
        drop(upstream);
        if let (Some(cache), Some(key), Some(lease)) = (cache, cache_key.as_ref(), lease.as_ref()) {
            cache.discard_partial(key.clone(), lease.clone()).await;
        }
        partial = None;
        headers = original_headers;
        upstream = shared.transport.get(resource.url.clone(), &headers, &shared.auth, &shared.cancel).await?;
    }
    // A live segment can be announced shortly before its upload is complete.
    // Wait only inside the recording deadline; VOD missing resources still fail.
    while upstream.status() == StatusCode::NOT_FOUND
        && shared.live_deadline.is_some_and(|deadline| std::time::Instant::now() < deadline)
    {
        drop(upstream);
        tokio::select! { biased; _ = shared.cancel.cancelled() => return Err(MediaError::Cancelled), _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {} }
        upstream = shared.transport.get(resource.url.clone(), &headers, &shared.auth, &shared.cancel).await?;
    }
    if upstream.status() == StatusCode::NOT_MODIFIED
        && cached.as_ref().is_some_and(|entry| !entry.matches_url(upstream.url()))
    {
        headers.remove(header::IF_NONE_MATCH);
        upstream = shared.transport.get(resource.url.clone(), &headers, &shared.auth, &shared.cancel).await?;
    }
    if upstream.status() == StatusCode::NOT_MODIFIED {
        let entry = cached.ok_or(MediaError::GatewayFailed)?;
        if upstream.headers().get(header::ETAG).is_some_and(|tag| tag.as_bytes() != entry.etag.as_bytes()) {
            return Err(MediaError::GatewayFailed);
        }
        let mut response =
            full(StatusCode::from_u16(entry.status).map_err(|_| MediaError::GatewayFailed)?, Bytes::from(entry.bytes));
        for (name, value) in [(header::CONTENT_TYPE, entry.content_type), (header::CONTENT_RANGE, entry.content_range)]
        {
            if let Some(value) = value {
                response.headers_mut().insert(name, value.parse().map_err(|_| MediaError::GatewayFailed)?);
            }
        }
        return Ok(response);
    }
    if !upstream.status().is_success() {
        return Err(MediaError::HttpStatus(upstream.status().as_u16()));
    }
    if resource.kind == Kind::Key {
        let key = bounded_body(&mut upstream, 16, &shared.cancel).await?;
        if key.len() != 16 {
            return Err(MediaError::GatewayFailed);
        }
        let mut response =
            full(StatusCode::OK, if request.method() == Method::HEAD { Bytes::new() } else { Bytes::from(key) });
        response.headers_mut().insert(header::CONTENT_LENGTH, 16.into());
        return Ok(response);
    }
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    if request.method() == Method::HEAD {
        let mut response = full(status, Bytes::new());
        response.headers_mut().remove(header::CONTENT_LENGTH);
        copy_response_headers(&response_headers, &mut response);
        return Ok(response);
    }
    if let (Some(cache), Some(key)) = (cache, cache_key) {
        let cacheable =
            response_headers.get(header::ETAG).and_then(|v| v.to_str().ok()).is_some_and(crate::cache::valid_etag)
                && upstream.content_length().is_some_and(|length| length <= cache.resource_limit as u64)
                && !response_headers
                    .get(header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|value| value.split(',').any(|part| part.trim().eq_ignore_ascii_case("no-store")))
                && !response_headers.get(header::VARY).is_some_and(|value| value == "*");
        if cacheable {
            let final_url = upstream.url().clone();
            if partial.is_none() && shared.transport.resume_partial_segments {
                partial = crate::cache::Partial::new(&final_url, status, &response_headers, cache.resource_limit);
                if partial.as_ref().is_some_and(|prefix| !prefix.matches_request(&headers)) {
                    return Err(MediaError::GatewayFailed);
                }
            }
            let (bytes, status, response_headers) = if let (Some(mut prefix), Some(lease)) = (partial, lease.as_ref()) {
                let mut persisted = prefix.bytes.len();
                loop {
                    let chunk = tokio::select! { biased;
                        _ = shared.cancel.cancelled() => return Err(MediaError::Cancelled),
                        chunk = upstream.chunk() => chunk,
                    };
                    match chunk {
                        Ok(Some(bytes)) => {
                            if prefix.bytes.len().saturating_add(bytes.len()) > prefix.length() {
                                cache.discard_partial(key.clone(), lease.clone()).await;
                                return Err(MediaError::GatewayFailed);
                            }
                            prefix.bytes.extend_from_slice(&bytes);
                            if prefix.bytes.len() < prefix.length()
                                && (persisted == 0
                                    || prefix.bytes.len().saturating_sub(persisted) as u64
                                        >= shared.transport.partial_flush_bytes)
                            {
                                cache.checkpoint(key.clone(), prefix.clone(), lease.clone()).await?;
                                persisted = prefix.bytes.len();
                            }
                        }
                        Ok(None) if prefix.bytes.len() == prefix.length() => break,
                        _ => {
                            if prefix.bytes.len() > persisted && prefix.bytes.len() < prefix.length() {
                                cache.checkpoint(key.clone(), prefix.clone(), lease.clone()).await?;
                            }
                            return Err(MediaError::Network);
                        }
                    }
                }
                let response_headers = prefix.headers();
                let status = StatusCode::from_u16(prefix.status).map_err(|_| MediaError::GatewayFailed)?;
                cache.discard_partial(key.clone(), lease.clone()).await;
                (prefix.bytes, status, response_headers)
            } else {
                (bounded_body(&mut upstream, cache.resource_limit, &shared.cancel).await?, status, response_headers)
            };
            cache.put_leased(key, &final_url, status.as_u16(), &response_headers, bytes.clone(), lease.clone()).await;
            let mut response = full(status, Bytes::from(bytes));
            copy_response_headers(&response_headers, &mut response);
            return Ok(response);
        }
    }
    // No detached pump: upstream ownership stays in Hyper's response body.
    // Disconnect/cancel drops the body and connection permit together, and
    // shutdown's connection JoinSet therefore also awaits all upstream work.
    let stream = upstream.bytes_stream().map(move |chunk| match chunk {
        Ok(bytes) => Ok(Frame::data(bytes)),
        Err(_) => {
            shared.failed.store(true, Ordering::Release);
            Err(std::io::Error::other("media upstream failed"))
        }
    });
    let mut response = Response::new(StreamBody::new(stream).boxed_unsync());
    *response.status_mut() = status;
    copy_response_headers(&response_headers, &mut response);
    Ok(response)
}

fn copy_response_headers(response_headers: &HeaderMap, response: &mut Response<Body>) {
    for name in [header::CONTENT_LENGTH, header::CONTENT_RANGE, header::CONTENT_TYPE, header::ACCEPT_RANGES] {
        if let Some(value) = response_headers.get(&name) {
            response.headers_mut().insert(name, value.clone());
        }
    }
}

#[cfg(test)]
#[path = "partial_http_tests.rs"]
mod partial_http_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn origin(truncated: bool) -> (String, CancellationToken) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            loop {
                let socket = tokio::select! {
                    _ = token.cancelled() => break,
                    accepted = listener.accept() => accepted.unwrap().0,
                };
                tokio::spawn(async move {
                    let mut socket = socket;
                    let mut bytes = [0; 4096];
                    let count = socket.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..count]);
                    let body = if request.contains("vod.m3u8") {
                        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts?secret=not-for-ffmpeg\n#EXT-X-ENDLIST\n"
                    } else {
                        "media bytes"
                    };
                    let length = if truncated && !request.contains("vod.m3u8") { 100 } else { body.len() };
                    let response =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}");
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, cancel)
    }

    #[tokio::test]
    async fn disconnected_consumers_release_stalled_upstreams_and_shutdown_joins_all_streams() {
        use std::sync::atomic::AtomicUsize;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let origin_cancel = CancellationToken::new();
        let token = origin_cancel.clone();
        let active_tasks = active.clone();
        let peak_tasks = peak.clone();
        let origin_task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let active = active_tasks.clone();
                        let peak = peak_tasks.clone();
                        tasks.spawn(async move {
                            let mut request = Vec::new();
                            let mut byte = [0; 1];
                            while !request.ends_with(b"\r\n\r\n") {
                                if socket.read(&mut byte).await.unwrap() == 0 { return; }
                                request.push(byte[0]);
                            }
                            if String::from_utf8_lossy(&request).contains("vod.m3u8") {
                                let body = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n";
                                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                            } else {
                                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                                peak.fetch_max(count, Ordering::SeqCst);
                                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\nx").await;
                                // Never finish the body. The gateway must close
                                // this socket when its consumer disconnects.
                                let _ = socket.read(&mut byte).await;
                                active.fetch_sub(1, Ordering::SeqCst);
                            }
                        });
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });
        let config = Arc::new(MediaConfig::default());
        let network = crate::MediaNetworkConfig { max_concurrent_requests: 1, ..Default::default() };
        let transport = NetworkTransport::new(&network, config.network_timeout).unwrap();
        let cancel = CancellationToken::new();
        let mut inputs = vec![format!("{origin}/vod.m3u8")];
        let mut gateway =
            Gateway::start(transport, config, HeaderMap::new(), Default::default(), &mut inputs, None, cancel.clone())
                .await
                .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let manifest = client.get(&inputs[0]).send().await.unwrap().text().await.unwrap();
        let media = Url::parse(manifest.lines().find(|line| line.starts_with("http:")).unwrap()).unwrap();
        async fn connect(media: &Url) -> tokio::net::TcpStream {
            let host = format!("127.0.0.1:{}", media.port().unwrap());
            let mut socket = tokio::net::TcpStream::connect(&host).await.unwrap();
            socket
                .write_all(
                    format!("GET {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n", media.path()).as_bytes(),
                )
                .await
                .unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0; 1];
            while !bytes.ends_with(b"\r\n\r\n") {
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                bytes.push(byte[0]);
            }
            socket
        }
        for _ in 0..12 {
            let socket = tokio::time::timeout(std::time::Duration::from_secs(2), connect(&media)).await.unwrap();
            assert_eq!(active.load(Ordering::SeqCst), 1);
            drop(socket);
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while active.load(Ordering::SeqCst) != 0 || gateway.shared.requests.available_permits() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("disconnected downstream must release stalled upstream and permit");
        }
        let socket = connect(&media).await;
        assert_eq!(active.load(Ordering::SeqCst), 1);
        gateway.shutdown().await;
        assert_eq!(gateway.shared.requests.available_permits(), 1);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown must drop all upstream response bodies");
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert!(!gateway.failed());
        drop(socket);
        origin_cancel.cancel();
        origin_task.await.unwrap();
    }

    #[tokio::test]
    async fn gateway_serves_only_capability_ids_and_shuts_down_after_cancel() {
        let (origin, origin_cancel) = origin(false).await;
        let config = Arc::new(MediaConfig::default());
        let transport = NetworkTransport::new(&Default::default(), config.network_timeout).unwrap();
        let cancel = CancellationToken::new();
        let mut inputs = vec![format!("{origin}/vod.m3u8")];
        let mut gateway = Gateway::start(
            transport,
            config,
            HeaderMap::new(),
            MediaSessionAuth::default(),
            &mut inputs,
            None,
            cancel.clone(),
        )
        .await
        .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let playlist = client.get(&inputs[0]).send().await.unwrap().text().await.unwrap();
        let head = client.head(&inputs[0]).send().await.unwrap();
        assert_eq!(
            head.headers().get(header::CONTENT_LENGTH).unwrap().to_str().unwrap().parse::<usize>().unwrap(),
            playlist.len()
        );
        assert!(!playlist.contains(&origin));
        assert!(!playlist.contains("secret="));
        let media = playlist.lines().find(|line| line.starts_with("http:")).unwrap();
        assert_eq!(client.get(media).send().await.unwrap().text().await.unwrap(), "media bytes");
        let head = client.head(media).send().await.unwrap();
        assert_eq!(head.headers().get(header::CONTENT_LENGTH).unwrap(), "11");
        assert_eq!(
            client.get(&inputs[0]).header("Host", "evil.example").send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client.get(&inputs[0]).header("Origin", "https://evil.example").send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let unknown = inputs[0].replace("/asset.m3u8", "/missing.m3u8");
        assert_eq!(client.get(unknown).send().await.unwrap().status(), StatusCode::NOT_FOUND);
        cancel.cancel();
        gateway.shutdown().await;
        assert!(client.get(&inputs[0]).send().await.is_err());
        assert!(!gateway.failed());
        origin_cancel.cancel();
    }

    #[tokio::test]
    async fn truncated_upstream_marks_session_failed_even_if_ffmpeg_might_recover() {
        let (origin, origin_cancel) = origin(true).await;
        let config = Arc::new(MediaConfig::default());
        let transport = NetworkTransport::new(&Default::default(), config.network_timeout).unwrap();
        let mut inputs = vec![format!("{origin}/vod.m3u8")];
        let mut gateway = Gateway::start(
            transport,
            config,
            HeaderMap::new(),
            MediaSessionAuth::default(),
            &mut inputs,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let playlist = client.get(&inputs[0]).send().await.unwrap().text().await.unwrap();
        let media = playlist.lines().find(|line| line.starts_with("http:")).unwrap();
        let response = client.get(media).send().await;
        if let Ok(response) = response {
            assert!(response.bytes().await.is_err());
        }
        assert!(gateway.failed());
        gateway.shutdown().await;
        origin_cancel.cancel();
    }

    #[test]
    fn rewriting_rejects_unhandled_fetch_features_and_preserves_range_identity() {
        let base = Url::parse("https://origin.example/vod.m3u8").unwrap();
        let mut catalog = Catalog {
            base: "http://127.0.0.1:1234/token".into(),
            resources: Vec::new(),
            urls: HashMap::new(),
            max_resources: 16,
            max_playlists: 16,
            expanded: HashSet::new(),
            dash_timelines: HashMap::new(),
            max_timeline_segments: 32,
            timeline_segments: 0,
        };
        for tag in [
            "#EXT-X-DEFINE:NAME=\"host\",VALUE=\"https://example.org\"",
            "#EXT-X-CONTENT-STEERING:SERVER-URI=\"https://example.org\"",
            "#EXT-X-PART:DURATION=1,URI=\"part.ts\"",
        ] {
            let text = format!("#EXTM3U\n{tag}\n#EXTINF:1,\na.ts\n#EXT-X-ENDLIST\n");
            assert!(matches!(rewrite(&text, &base, 0, &mut catalog), Err(MediaError::UnsupportedGatewayMedia)));
        }
        let text = "#EXTM3U\n#EXT-X-BYTERANGE:10@0\n#EXTINF:1,\nall.ts?token=x\n#EXT-X-BYTERANGE:10\n#EXTINF:1,\nall.ts?token=x\n#EXT-X-ENDLIST\n";
        let result = rewrite(text, &base, 0, &mut catalog).unwrap();
        let result = std::str::from_utf8(&result).unwrap();
        let urls: Vec<_> = result.lines().filter(|line| line.starts_with("http:")).collect();
        assert_eq!(urls[0], urls[1]);
        assert!(result.contains("#EXT-X-BYTERANGE:10\n"));
        assert!(!result.contains("token=x"));
    }
    #[test]
    fn dash_templates_are_origin_bound_and_unhandled_external_fetches_fail() {
        let mut catalog = Catalog {
            base: "http://127.0.0.1:1234/cap".into(),
            resources: vec![],
            urls: HashMap::new(),
            max_resources: 32,
            max_playlists: 4,
            expanded: HashSet::new(),
            dash_timelines: HashMap::new(),
            max_timeline_segments: 32,
            timeline_segments: 0,
        };
        let base = Url::parse("https://origin.example/private/stream.mpd?token=secret").unwrap();
        let xml = r#"<MPD type="dynamic"><BaseURL>../assets/</BaseURL><Period><AdaptationSet><SegmentTemplate media="$RepresentationID$/$Time$.m4s?token=hidden" initialization="$RepresentationID$/init.mp4"/><Representation id="video-1" bandwidth="500"/></AdaptationSet></Period></MPD>"#;
        let (body, live) = crate::dash_gateway::rewrite(xml, &base, &mut catalog).unwrap();
        let body = String::from_utf8(body).unwrap();
        assert!(live);
        assert!(!body.contains("hidden"));
        assert!(!body.contains("origin.example"));
        let template =
            catalog.resources.iter().find(|resource| resource.template.is_some()).unwrap().template.as_ref().unwrap();
        assert_eq!(
            template.resolve("1234").unwrap().as_str(),
            "https://origin.example/assets/video-1/1234.m4s?token=hidden"
        );
        for suffix in ["../other", "https:%2f%2fevil", "1/2", "-1", "18446744073709551616"] {
            assert!(template.resolve(suffix).is_err());
        }
        for xml in [
            r#"<MPD><Period xlink:href="https://evil.example"/></MPD>"#,
            r#"<MPD><UTCTiming value="https://evil.example"/></MPD>"#,
            r#"<!DOCTYPE MPD [<!ENTITY x SYSTEM "file:///tmp/x">]><MPD/>"#,
        ] {
            assert!(crate::dash_gateway::rewrite(xml, &base, &mut catalog).is_err());
        }
        // Dollar signs in an HLS segment query remain literal, not DASH variables.
        let uri = catalog
            .register(Url::parse("https://origin.example/a.ts?token=$literal$").unwrap(), Kind::Media, 0)
            .unwrap();
        assert!(!uri.contains('$'));
    }

    #[test]
    fn live_dash_timeline_keeps_fractional_boundaries_and_window_identity_with_budgets() {
        let create = || Catalog {
            base: "http://127.0.0.1:1234/cap".into(),
            resources: vec![],
            urls: HashMap::new(),
            max_resources: 32,
            max_playlists: 4,
            expanded: HashSet::new(),
            dash_timelines: HashMap::new(),
            max_timeline_segments: 8,
            timeline_segments: 0,
        };
        let base = Url::parse("https://origin.example/live.mpd").unwrap();
        let xml = |start: u64, timeline: &str| {
            format!(
                r#"<MPD type="dynamic"><Period><AdaptationSet><Representation id="audio"><SegmentTemplate timescale="44100" startNumber="{start}" media="a-$Number%05d$-$Time%05d$.m4s" initialization="init.mp4"><SegmentTimeline>{timeline}</SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#
            )
        };
        let mut catalog = create();
        let (first, _) =
            crate::dash_gateway::rewrite(&xml(42, r#"<S t="1024" d="44032" r="1"/>"#), &base, &mut catalog).unwrap();
        let first = String::from_utf8(first).unwrap();
        assert!(first.contains("SegmentList"));
        assert!(!first.contains("SegmentTimeline"));
        assert!(first.contains("/00042/01024"));
        assert!(first.contains("/00043/45056"));
        assert!(first.contains("startNumber=\"0\""));
        let (next, _) =
            crate::dash_gateway::rewrite(&xml(43, r#"<S t="45056" d="44032" r="1"/>"#), &base, &mut catalog).unwrap();
        let next = String::from_utf8(next).unwrap();
        assert_eq!(next.matches("<SegmentURL ").count(), 3);
        assert!(next.contains("/00042/01024"), "sliding origin window must not renumber already seen media");
        assert!(next.contains("/00044/89088"));
        assert_eq!(catalog.timeline_segments, 3);
        for invalid in [xml(43, r#"<S t="45055" d="44032"/>"#), xml(46, r#"<S t="177152" d="44032"/>"#)] {
            assert!(matches!(
                crate::dash_gateway::rewrite(&invalid, &base, &mut catalog),
                Err(MediaError::InvalidManifest)
            ));
        }
        for invalid in [xml(1, r#"<S d="1024" r="18446744073709551615"/>"#), xml(1, r#"<S d="1024" r="8"/>"#)] {
            assert!(matches!(
                crate::dash_gateway::rewrite(&invalid, &base, &mut create()),
                Err(MediaError::ManifestLimit)
            ));
        }
        assert!(matches!(
            crate::dash_gateway::rewrite(&xml(1, r#"<S d="1024" r="-1"/>"#), &base, &mut create()),
            Err(MediaError::UnsupportedGatewayMedia)
        ));
        let bounded = xml(1, r#"<S t="0" d="1000" r="-1"/><S t="2000" d="1000"/>"#);
        let (body, _) = crate::dash_gateway::rewrite(&bounded, &base, &mut create()).unwrap();
        assert_eq!(String::from_utf8(body).unwrap().matches("<SegmentURL ").count(), 3);
    }
}
