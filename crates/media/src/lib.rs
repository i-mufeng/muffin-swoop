//! Bounded HLS/DASH inspection and cancellable FFmpeg remuxing.
//!
//! This library never starts a shell, logs media URLs, forwards credentials to
//! FFmpeg, or allows FFmpeg to open local files referenced by a remote manifest.
//! Explicit session credentials only reach scoped upstream requests in the media gateway.
//! FFmpeg and (for external integration tests) FFprobe are installed separately.

mod cache;
mod dash_gateway;
mod gateway;
mod manifest;
mod network;
mod publication;
mod selection;
mod session_auth;
// TTML/IMSC/wvtt 转换尚未接入下载流程；目前只用到 DASH 清单过滤与配置。
#[allow(dead_code)]
mod subtitle;
mod limiter;

use std::{fmt, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::Semaphore,
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub use manifest::{MediaInfo, MediaKind};
pub use network::{MediaNetworkConfig, MediaProxyConfig, MediaProxyCredentials};
pub use subtitle::SubtitleConfig;
pub use selection::{
    AudioSelection, AudioTrack, MediaSelection, MediaVariant, SubtitleFormat, SubtitleSelection, SubtitleTrack,
    VideoSelection,
};
pub use session_auth::{MediaAuthorization, MediaSessionAuth, OriginCredentials, ScopedCookie};

#[derive(Clone, Debug)]
pub struct MediaConfig {
    pub ffmpeg_path: PathBuf,
    /// Shared by clones of the same MediaDownloader. Valid range: 1..=32.
    pub max_concurrent: usize,
    pub network_timeout: Duration,
    pub max_manifest_bytes: usize,
    pub max_playlists: usize,
    /// Explicit transport for legacy mms:// links; never retries another protocol.
    pub mms_transport: MmsTransport,
    /// Extra wall time allowed for a live playlist to finalize after recording duration.
    pub live_finalize_grace_secs: u64,
    pub subtitles: SubtitleConfig,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ffmpeg_path: "ffmpeg".into(),
            max_concurrent: 2,
            network_timeout: Duration::from_secs(30),
            max_manifest_bytes: 2 * 1024 * 1024,
            max_playlists: 32,
            mms_transport: MmsTransport::Http,
            live_finalize_grace_secs: 3,
            subtitles: SubtitleConfig::default(),
        }
    }
}

/// MMSH uses HTTP streaming; MMST uses the legacy TCP wire protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MmsTransport {
    #[default]
    Http,
    Tcp,
}

/// Headers are restricted at runtime to User-Agent, Accept, Accept-Language and
/// Referer. Referer is reduced to its origin; credentials/custom headers fail.
#[derive(Clone)]
pub struct MediaSource {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl MediaSource {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), headers: Vec::new() }
    }

    /// Validate URL syntax and supported headers without making a request.
    pub fn validate(&self) -> Result<(), MediaError> {
        source_url(&self.url)?;
        safe_headers(&self.headers)?;
        Ok(())
    }
}

impl fmt::Debug for MediaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaSource").field("url", &"<redacted>").field("headers", &"<redacted>").finish()
    }
}

#[derive(Clone, Debug)]
pub struct MediaRequest {
    pub source: MediaSource,
    /// An .mp4 or .mkv path. Its parent directory must already exist.
    pub destination: PathBuf,
    /// Required for live media. Also permits bounded recordings or VOD clips.
    pub duration_limit: Option<Duration>,
    pub selection: MediaSelection,
    /// One-shot authorization; an existing regular file must remain unchanged.
    pub overwrite: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MediaProgress {
    pub output_bytes: u64,
    pub media_time: Duration,
    pub speed: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum MediaEvent {
    Waiting,
    Started { info: MediaInfo },
    Progress(MediaProgress),
    Completed { bytes: u64 },
}

#[derive(Clone, Copy, Debug)]
pub struct MediaCapabilities {
    pub hls: bool,
    pub mmsh: bool,
    pub mmst: bool,
    pub asf: bool,
    pub dash: bool,
    pub verifies_tls_by_default: bool,
    pub webvtt_decode: bool,
    pub mov_text_decode: bool,
    pub subrip_decode: bool,
    pub ass_decode: bool,
    pub mov_text_encode: bool,
    pub subrip_encode: bool,
}

#[derive(Clone, Debug)]
pub struct MediaResult {
    pub destination: PathBuf,
    pub bytes: u64,
    pub info: MediaInfo,
}

/// Errors intentionally exclude request URLs, header values and FFmpeg stderr.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("媒体配置无效")]
    InvalidConfig,
    #[error("媒体网络配置无效")]
    InvalidNetwork,
    #[error("媒体会话凭据或范围无效")]
    InvalidAuth,
    #[error("媒体代理认证失败：{0}")]
    ProxyAuthentication(String),
    #[error("该媒体清单包含受控网络模式尚不支持的功能")]
    UnsupportedGatewayMedia,
    #[error("媒体网关请求失败，未生成完成文件")]
    GatewayFailed,
    #[error("MMS 仅支持直连且需关闭会话登录；不静默绕过代理配置")]
    MmsNetworkUnsupported,
    #[error("MMS 录制请使用 MKV 文件并设置时长")]
    MmsRequiresMkvDuration,
    #[error("仅支持不含用户名密码的 HTTP(S) 或 MMS 媒体地址")]
    InvalidUrl,
    #[error("请求头格式无效，禁止换行")]
    InvalidHeader,
    #[error("认证或自定义请求头暂不支持；仅支持 User-Agent、Accept、Accept-Language 和裁剪后的 Referer")]
    UnsupportedHeader,
    #[error("媒体清单请求失败")]
    Network,
    #[error("媒体服务器返回 HTTP {0}")]
    HttpStatus(u16),
    #[error("媒体清单超过大小、数量或层级限制")]
    ManifestLimit,
    #[error("不是有效的 HLS 或 DASH 清单")]
    InvalidManifest,
    #[error("所选清晰度、音轨或字幕已失效、不兼容或无法精确匹配，请重新检查媒体清单")]
    InvalidSelection,
    #[error("该字幕格式暂不支持；仅支持当前 FFmpeg 具备解码能力的文本字幕")]
    UnsupportedSubtitle,
    #[error("字幕结构或时间无效，未生成完成文件")]
    InvalidSubtitle,
    #[error("字幕超过资源、数量或解析限制")]
    SubtitleLimit,
    #[error("暂不支持直播字幕，请关闭字幕后录制")]
    LiveSubtitlesUnsupported,
    #[error("暂不支持需要许可证的 DRM 或 SAMPLE-AES 媒体")]
    ProtectedMedia,
    #[error("直播下载需要设置录制时长")]
    LiveRequiresDuration,
    #[error("仅支持 .mp4 或 .mkv 输出")]
    UnsupportedOutput,
    #[error("目标文件已存在，未覆盖")]
    DestinationExists,
    #[error("目标文件在下载期间发生变化，请重新确认")]
    DestinationChanged,
    #[error("原文件已保留在恢复路径：{0}")]
    RecoveryRequired(PathBuf),
    #[error("目标不是普通文件，不能覆盖目录或符号链接")]
    UnsafeDestination,
    #[error("无法启动 FFmpeg，请检查可执行文件路径")]
    FfmpegUnavailable,
    #[error("当前 FFmpeg 构建不支持 {0}，请配置包含该能力的版本")]
    FfmpegUnsupported(&'static str),
    #[error("FFmpeg 下载失败（退出码 {0:?}）")]
    FfmpegFailed(Option<i32>),
    #[error("FFmpeg 报告媒体警告或分片缺失，未生成完成文件")]
    FfmpegDiagnostic,
    #[error("直播在录制时长前中断，未生成完成文件")]
    IncompleteRecording,
    #[error("FFmpeg 没有生成有效输出")]
    EmptyOutput,
    #[error("媒体下载已取消")]
    Cancelled,
    #[error("媒体文件或子进程操作失败：{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct MediaDownloader {
    config: Arc<MediaConfig>,
    transport: network::NetworkTransport,
    gateway_enabled: bool,
    slots: Arc<Semaphore>,
    capabilities: Arc<tokio::sync::OnceCell<MediaCapabilities>>,
}

impl MediaDownloader {
    pub fn new(config: MediaConfig) -> Result<Self, MediaError> {
        Self::build(config, MediaNetworkConfig::default(), true, None)
    }

    /// Use one controlled transport for HLS and DASH, including bounded live recordings.
    /// Unsupported addressing fails explicitly; there is no direct-network fallback.
    pub fn new_with_network(config: MediaConfig, network: MediaNetworkConfig) -> Result<Self, MediaError> {
        Self::build(config, network, true, None)
    }

    pub fn new_with_network_and_proxy(config: MediaConfig, network: MediaNetworkConfig, credentials: Option<MediaProxyCredentials>) -> Result<Self, MediaError> {
        Self::build(config, network, true, credentials)
    }

    pub fn for_task(&self, limit_bps: u64) -> Result<Self, MediaError> {
        let mut downloader = self.clone();
        downloader.transport = self.transport.for_task(limit_bps);
        downloader.gateway_enabled = true;
        Ok(downloader)
    }

    pub fn set_task_speed_limit(&self, limit_bps: u64) {
        self.transport.set_task_speed_limit(limit_bps);
    }

    fn build(config: MediaConfig, network: MediaNetworkConfig, gateway_enabled: bool, credentials: Option<MediaProxyCredentials>) -> Result<Self, MediaError> {
        if !(1..=32).contains(&config.max_concurrent)
            || config.network_timeout.is_zero()
            || config.network_timeout > Duration::from_secs(3600)
            || !(1024..=16 * 1024 * 1024).contains(&config.max_manifest_bytes)
            || !(1..=128).contains(&config.max_playlists)
            || config.live_finalize_grace_secs > 300
            || config.ffmpeg_path.as_os_str().is_empty()
        {
            return Err(MediaError::InvalidConfig);
        }
        config.subtitles.validate()?;
        let transport = network::NetworkTransport::new_with_proxy(&network, config.network_timeout, credentials)?;
        Ok(Self {
            slots: Arc::new(Semaphore::new(config.max_concurrent)),
            capabilities: Arc::new(tokio::sync::OnceCell::new()),
            transport,
            gateway_enabled,
            config: Arc::new(config),
        })
    }

    /// Probe the configured executable without making a network request. Results
    /// are cached for this downloader and its clones. No shell is invoked.
    pub async fn capabilities(&self, cancel: &CancellationToken) -> Result<MediaCapabilities, MediaError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MediaError::Cancelled),
            result = self.capabilities.get_or_try_init(|| async {
                let protocols = self.probe_output(&["-hide_banner", "-protocols"]).await?;
                let protocol = |name: &str| protocols.lines().any(|line| line.trim() == name);
                let demuxers = self.probe_output(&["-hide_banner", "-demuxers"]).await?;
                let tls = self.probe_output(&["-hide_banner", "-h", "protocol=tls"]).await?;
                let decoders = self.probe_output(&["-hide_banner", "-decoders"]).await?;
                let encoders = self.probe_output(&["-hide_banner", "-encoders"]).await?;
                let supports = |name: &str| demuxers.lines().any(|line| line.split_whitespace().nth(1) == Some(name));
                let has = |output: &str, name: &str| output.lines().any(|line| line.split_whitespace().nth(1) == Some(name));
                let verifies_tls_by_default = tls.lines().any(|line| line.contains("-tls_verify") && (line.contains("(default true)") || line.contains("(default 1)")));
                Ok(MediaCapabilities { mmsh: protocol("mmsh"), mmst: protocol("mmst"), asf: supports("asf"), hls: supports("hls"), dash: supports("dash"), verifies_tls_by_default,
                    webvtt_decode: has(&decoders, "webvtt") && supports("webvtt"), mov_text_decode: has(&decoders, "mov_text"),
                    subrip_decode: has(&decoders, "subrip") && supports("srt"), ass_decode: has(&decoders, "ssa") && supports("ass"),
                    mov_text_encode: has(&encoders, "mov_text"), subrip_encode: has(&encoders, "subrip") })
            }) => result.copied(),
        }
    }

    async fn probe_output(&self, arguments: &[&str]) -> Result<String, MediaError> {
        let mut command = Command::new(&self.config.ffmpeg_path);
        command
            .args(arguments)
            .env_remove("FFREPORT")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .map_err(|_| MediaError::FfmpegUnavailable)?
            .map_err(|_| MediaError::FfmpegUnavailable)?;
        if !output.status.success() {
            return Err(MediaError::FfmpegUnavailable);
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Checks nested HLS playlists and clear/AES-128 media without downloading
    /// segments. Cancellation is supported throughout manifest requests.
    pub async fn inspect(&self, source: &MediaSource, cancel: &CancellationToken) -> Result<MediaInfo, MediaError> {
        self.inspect_with_auth(source, &MediaSessionAuth::default(), cancel).await
    }

    pub async fn inspect_with_auth(
        &self,
        source: &MediaSource,
        auth: &MediaSessionAuth,
        cancel: &CancellationToken,
    ) -> Result<MediaInfo, MediaError> {
        let url = source_url(&source.url)?;
        let headers = safe_headers(&source.headers)?;
        if is_mms(&url) {
            if self.transport.has_proxy || !auth.is_empty() {
                return Err(MediaError::MmsNetworkUnsupported);
            }
            return Ok(MediaInfo {
                kind: MediaKind::Mms,
                is_live: true,
                is_master: false,
                duration: None,
                variant_count: 0,
                uses_aes128: false,
                variants: vec![],
                audio_tracks: vec![],
                subtitle_tracks: vec![],
            });
        }
        let info = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MediaError::Cancelled),
            result = manifest::inspect(&self.transport, &self.config, url, headers, auth, cancel) => result,
        }?;
        Ok(info)
    }

    /// The callback must return quickly. Cancel and await this future to ensure
    /// the FFmpeg process has exited and the temporary output has been removed.
    /// Existing destinations require one-shot overwrite authorization and must
    /// remain unchanged when moved aside for no-clobber publication.
    pub async fn download(
        &self,
        request: MediaRequest,
        cancel: CancellationToken,
        on_event: impl FnMut(MediaEvent) + Send,
    ) -> Result<MediaResult, MediaError> {
        self.download_with_auth(request, MediaSessionAuth::default(), cancel, on_event).await
    }

    pub async fn download_with_auth(
        &self,
        request: MediaRequest,
        auth: MediaSessionAuth,
        cancel: CancellationToken,
        mut on_event: impl FnMut(MediaEvent) + Send,
    ) -> Result<MediaResult, MediaError> {
        source_url(&request.source.url)?;
        let headers = safe_headers(&request.source.headers)?;
        let format = match request.destination.extension().and_then(|s| s.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("mp4") => "mp4",
            Some(ext) if ext.eq_ignore_ascii_case("mkv") => "matroska",
            _ => return Err(MediaError::UnsupportedOutput),
        };
        if request.duration_limit.is_some_and(|duration| duration.is_zero()) {
            return Err(MediaError::InvalidConfig);
        }
        on_event(MediaEvent::Waiting);
        let _permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(MediaError::Cancelled),
            permit = self.slots.acquire() => permit.map_err(|_| MediaError::Cancelled)?,
        };
        let publication =
            publication::Publication::prepare(request.destination.clone(), request.overwrite, cancel.clone()).await?;
        let info = self.inspect_with_auth(&request.source, &auth, &cancel).await?;
        let mut plan = selection::plan(&request.source, &info, &request.selection)?;
        let capabilities = self.capabilities(&cancel).await?;
        if info.kind == MediaKind::Mms {
            let mut url = source_url(&request.source.url)?;
            if url.scheme() == "mms" {
                url.set_scheme(match self.config.mms_transport {
                    MmsTransport::Http => "mmsh",
                    MmsTransport::Tcp => "mmst",
                })
                .map_err(|_| MediaError::InvalidUrl)?;
            }
            if !capabilities.asf
                || (url.scheme() == "mmsh" && !capabilities.mmsh)
                || (url.scheme() == "mmst" && !capabilities.mmst)
            {
                return Err(MediaError::FfmpegUnsupported("MMS/ASF 协议与解复用器"));
            }
            if format != "matroska" || request.duration_limit.is_none() {
                return Err(MediaError::MmsRequiresMkvDuration);
            }
            plan.inputs = vec![url.to_string()];
        }
        match info.kind {
            MediaKind::Hls if !capabilities.hls => {
                return Err(MediaError::FfmpegUnsupported("HLS 解复用器"));
            }
            MediaKind::Dash if !capabilities.dash => {
                return Err(MediaError::FfmpegUnsupported("DASH 解复用器"));
            }
            _ => {}
        }
        // Nested HTTPS resources may appear even under a plain HTTP manifest.
        // Require secure protocol defaults rather than silently accepting an old
        // FFmpeg build that skips certificate verification on those requests.
        if !capabilities.verifies_tls_by_default {
            return Err(MediaError::FfmpegUnsupported("默认 TLS 证书验证"));
        }
        if let Some(subtitle) = plan.subtitle {
            match subtitle {
                SubtitleFormat::WebVtt if !capabilities.webvtt_decode => {
                    return Err(MediaError::FfmpegUnsupported("WebVTT 字幕解码/解复用器"));
                }
                SubtitleFormat::MovText if !capabilities.mov_text_decode => {
                    return Err(MediaError::FfmpegUnsupported("mov_text 字幕解码器"));
                }
                SubtitleFormat::SubRip if !capabilities.subrip_decode => {
                    return Err(MediaError::FfmpegUnsupported("SubRip/SRT 字幕解码器"));
                }
                SubtitleFormat::Ass if !capabilities.ass_decode => {
                    return Err(MediaError::FfmpegUnsupported("ASS/SSA 字幕解码器"));
                }
                SubtitleFormat::Unsupported => return Err(MediaError::UnsupportedSubtitle),
                _ => {}
            }
            if format == "mp4" && !capabilities.mov_text_encode {
                return Err(MediaError::FfmpegUnsupported("MP4 mov_text 字幕编码器"));
            }
            if format == "matroska" && !capabilities.subrip_encode {
                return Err(MediaError::FfmpegUnsupported("MKV SubRip 字幕编码器"));
            }
        }
        if info.is_live && request.duration_limit.is_none() {
            return Err(MediaError::LiveRequiresDuration);
        }
        let parent = publication.parent();
        let temporary =
            tempfile::Builder::new().prefix(".swoop-media-").suffix(".part").tempfile_in(parent)?.into_temp_path();
        let mut gateway = if info.kind != MediaKind::Mms && (self.gateway_enabled || !auth.is_empty() || info.is_live) {
            Some(
                gateway::Gateway::start(
                    self.transport.clone(),
                    self.config.clone(),
                    headers.clone(),
                    auth,
                    &mut plan.inputs,
                    request.duration_limit.filter(|_| info.is_live),
                    cancel.child_token(),
                )
                .await?,
            )
        } else {
            None
        };
        let command_headers = if gateway.is_some() { HeaderMap::new() } else { headers };
        let mut command = self.command(&request, &command_headers, &info, &plan, format, &temporary);
        let mut progress = MediaProgress::default();
        let outcome = async {
        let mut child = command.spawn().map_err(|_| MediaError::FfmpegUnavailable)?;
        let stdout = child.stdout.take().expect("FFmpeg stdout was configured as piped");
        let mut stderr = child.stderr.take().expect("FFmpeg stderr was configured as piped");
        let mut diagnostic_buffer = [0u8; 4096];
        let mut diagnostics_open = true;
        let mut saw_diagnostics = false;
        let mut lines = BufReader::new(stdout).lines();
        let mut progress_open = true;
        on_event(MediaEvent::Started { info: info.clone() });
        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    child.kill().await?;
                    return Err(MediaError::Cancelled);
                },
                status = child.wait() => break status?,
                diagnostic = stderr.read(&mut diagnostic_buffer), if diagnostics_open => {
                    match diagnostic {
                        Ok(0) => diagnostics_open = false,
                        Ok(count) => saw_diagnostics |= diagnostic_buffer[..count].iter().any(|byte| !byte.is_ascii_whitespace()),
                        Err(error) => {
                            let _ = child.kill().await;
                            return Err(error.into());
                        },
                    }
                },
                line = lines.next_line(), if progress_open => {
                    match line {
                        Ok(Some(line)) => {
                            if parse_progress(&line, &mut progress) {
                                on_event(MediaEvent::Progress(progress.clone()));
                            }
                        },
                        Ok(None) => progress_open = false,
                        Err(error) => {
                            let _ = child.kill().await;
                            return Err(error.into());
                        },
                    }
                },
            }
        };
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        if !status.success() {
            return Err(MediaError::FfmpegFailed(status.code()));
        }
        // Drain the finite progress tail after the child exits.
        while let Some(line) = lines.next_line().await? {
            if parse_progress(&line, &mut progress) {
                on_event(MediaEvent::Progress(progress.clone()));
            }
        }
        // HLS demuxers can skip an unavailable segment, warn, then exit 0.
        // Treat diagnostics as incomplete rather than publishing a partial
        // movie. Consume bounded chunks and never retain or log source URLs.
        loop {
            let count = stderr.read(&mut diagnostic_buffer).await?;
            if count == 0 {
                break;
            }
            saw_diagnostics |= diagnostic_buffer[..count].iter().any(|byte| !byte.is_ascii_whitespace());
        }
        if saw_diagnostics {
            return Err(MediaError::FfmpegDiagnostic);
        }
        Ok(())
        }.await;
        if let Some(gateway) = &mut gateway {
            gateway.shutdown().await;
            if gateway.failed() && !matches!(outcome, Err(MediaError::Cancelled)) {
                return Err(MediaError::GatewayFailed);
            }
        }
        outcome?;
        if info.is_live
            && request
                .duration_limit
                .is_some_and(|limit| progress.media_time.saturating_add(Duration::from_millis(250)) < limit)
        {
            return Err(MediaError::IncompleteRecording);
        }
        let output = tokio::fs::OpenOptions::new().read(true).write(true).open(&temporary).await?;
        let bytes = output.metadata().await?.len();
        if bytes == 0 {
            return Err(MediaError::EmptyOutput);
        }
        output.sync_all().await?;
        drop(output);
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        publication.publish(temporary, cancel.clone()).await?;
        on_event(MediaEvent::Completed { bytes });
        Ok(MediaResult { destination: request.destination, bytes, info })
    }

    fn command(
        &self,
        request: &MediaRequest,
        headers: &HeaderMap,
        info: &MediaInfo,
        plan: &selection::Plan,
        format: &str,
        output: &std::path::Path,
    ) -> Command {
        let mut command = Command::new(&self.config.ffmpeg_path);
        command.kill_on_drop(true).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // FFmpeg reports can contain URLs and headers. Never inherit FFREPORT.
        command.env_remove("FFREPORT");
        for variable in ["http_proxy", "https_proxy", "all_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            command.env_remove(variable);
        }
        command.args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-xerror",
            "-nostdin",
            "-nostats",
            "-y",
            "-progress",
            "pipe:1",
            "-stats_period",
            "0.2",
        ]);
        if plan.subtitle.is_some() {
            command.args(["-abort_on", "empty_output_stream"]);
        }
        let mut header_text = String::new();
        for (name, value) in headers {
            header_text.push_str(name.as_str());
            header_text.push_str(": ");
            // safe_headers only constructs visible ASCII header values.
            header_text.push_str(value.to_str().expect("validated header"));
            header_text.push_str("\r\n");
        }
        for (input_index, input) in plan.inputs.iter().enumerate() {
            let input = source_url(input).expect("inspected media URL");
            command.args([
                "-protocol_whitelist",
                if info.kind == MediaKind::Mms { "mmsh,mmst,http,https,tcp,tls" } else { "http,https,tcp,tls,crypto" },
                "-rw_timeout",
            ]);
            command.arg(self.config.network_timeout.as_micros().to_string());
            if info.kind == MediaKind::Mms {
                command.args(["-guess_layout_max", "0"]);
            }
            if input.scheme() == "https" {
                command.args(["-tls_verify", "1"]);
            }
            if !header_text.is_empty() {
                command.arg("-headers").arg(&header_text);
            }
            if plan.subtitle_input == Some(input_index) {
                // HLS initially discards subtitles; DASH reopens their streams
                // after probing. Generic probing can lose/repeat opening cues.
                // This dedicated manifest input only supplies the declared text
                // stream; video/audio inputs retain their normal probing.
                // Audio/video from this input are unused. Do not guess an
                // unprobed audio channel layout for those discarded streams.
                command.args(["-nofind_stream_info", "-an", "-vn", "-guess_layout_max", "0"]);
            }
            command.args([
                "-f",
                match info.kind {
                    MediaKind::Hls => "hls",
                    MediaKind::Dash => "dash",
                    MediaKind::Mms => "asf",
                },
            ]);
            command.arg("-i").arg(input.as_str());
        }
        for map in &plan.maps {
            command.arg("-map").arg(map);
        }
        // Never copy manifest metadata containing source URLs into the output.
        command.args([
            "-map_metadata",
            "-1",
            "-map_metadata:s",
            "-1",
            "-map_chapters",
            "-1",
            "-c:v",
            "copy",
            "-c:a",
            "copy",
        ]);
        if plan.subtitle.is_some() {
            command.args(["-c:s", if format == "mp4" { "mov_text" } else { "subrip" }]);
            if let Some(language) = &plan.subtitle_language {
                command.arg("-metadata:s:s:0").arg(format!("language={language}"));
            }
        } else {
            command.arg("-sn");
        }
        if let Some(duration) = request.duration_limit {
            command.arg("-t").arg(format!("{:.6}", duration.as_secs_f64()));
        }
        if format == "mp4" {
            command.args(["-movflags", "+faststart"]);
        }
        command.args(["-f", format]).arg(output);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        command
    }
}

fn is_mms(url: &Url) -> bool {
    matches!(url.scheme(), "mms" | "mmsh" | "mmst")
}
fn source_url(raw: &str) -> Result<Url, MediaError> {
    let url = Url::parse(raw).map_err(|_| MediaError::InvalidUrl)?;
    if !is_mms(&url) {
        return network_url(raw);
    }
    if raw.len() > 16384
        || raw.contains(['\r', '\n', '\0'])
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(MediaError::InvalidUrl);
    }
    Ok(url)
}

fn network_url(raw: &str) -> Result<Url, MediaError> {
    let url = Url::parse(raw).map_err(|_| MediaError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || raw.contains(['\r', '\n', '\0'])
    {
        return Err(MediaError::InvalidUrl);
    }
    Ok(url)
}

fn safe_headers(headers: &[(String, String)]) -> Result<HeaderMap, MediaError> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        if name.contains(['\r', '\n', '\0']) || value.contains(['\r', '\n', '\0']) {
            return Err(MediaError::InvalidHeader);
        }
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| MediaError::InvalidHeader)?;
        let value = match name.as_str() {
            "user-agent" | "accept" | "accept-language" => value.clone(),
            "referer" => {
                let url = network_url(value)?;
                format!("{}/", url.origin().ascii_serialization())
            }
            _ => return Err(MediaError::UnsupportedHeader),
        };
        let value = HeaderValue::from_str(&value).map_err(|_| MediaError::InvalidHeader)?;
        if value.to_str().is_err() {
            return Err(MediaError::InvalidHeader);
        }
        result.insert(name, value);
    }
    Ok(result)
}

fn parse_progress(line: &str, progress: &mut MediaProgress) -> bool {
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };
    match key {
        "total_size" => {
            if let Ok(bytes) = value.parse() {
                progress.output_bytes = bytes;
            }
        }
        "out_time_us" => {
            if let Ok(micros) = value.parse::<u64>() {
                progress.media_time = Duration::from_micros(micros);
            }
        }
        "speed" => {
            progress.speed = value.trim_end_matches('x').parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0);
        }
        "progress" => return value == "continue" || value == "end",
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_header_injection_are_rejected() {
        for name in ["Cookie", "Authorization", "Proxy-Authorization", "X-Token", "Host"] {
            assert!(matches!(safe_headers(&[(name.into(), "secret".into())]), Err(MediaError::UnsupportedHeader)));
        }
        assert!(matches!(
            safe_headers(&[("User-Agent".into(), "ok\r\nCookie: secret".into())]),
            Err(MediaError::InvalidHeader)
        ));
        let headers =
            safe_headers(&[("Referer".into(), "https://example.org/private/path?token=secret#fragment".into())])
                .unwrap();
        assert_eq!(headers["referer"], "https://example.org/");
        assert!(network_url("file:///etc/passwd").is_err());
        assert!(network_url("https://user:secret@example.org/media.m3u8").is_err());
        assert!(!format!("{:?}", MediaSource::new("https://example.org/?token=secret")).contains("secret"));
    }

    #[test]
    fn progress_rejects_invalid_numeric_values() {
        let mut progress = MediaProgress::default();
        parse_progress("out_time_us=-1", &mut progress);
        parse_progress("speed=NaNx", &mut progress);
        assert_eq!(progress.media_time, Duration::ZERO);
        assert!(progress.speed.is_none());
        parse_progress("out_time_us=1200000", &mut progress);
        parse_progress("total_size=4096", &mut progress);
        parse_progress("speed=2.5x", &mut progress);
        assert!(parse_progress("progress=end", &mut progress));
        assert_eq!(progress.media_time, Duration::from_millis(1200));
        assert_eq!(progress.output_bytes, 4096);
        assert_eq!(progress.speed, Some(2.5));
    }
}
