//! Per-data-directory process lock and a separate, authenticated local control service.
//! Browser bridge endpoints never gain these privileges.
use std::convert::Infallible;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fs2::FileExt;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use swoop_core::{AddOptions, Engine, QueueId, TaskId, TaskStatus};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

const CONTROL_DIR: &str = ".swoop-control";
const MAX_BODY: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

pub fn data_dir() -> PathBuf {
    std::env::var_os("SWOOP_DATA_DIR")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::data_dir().map(|d| d.join("com.muffin.swoop")).unwrap_or_else(|| PathBuf::from(".")))
}

pub struct InstanceGuard {
    _lock: File,
    directory: PathBuf,
}

impl InstanceGuard {
    /// Acquire before opening SQLite or restoring any tasks. Never unlink the lock file:
    /// a second inode would permit two owners of the same database.
    pub fn acquire(data: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(data)?;
        let data = data.canonicalize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&data)?.permissions().mode() & 0o022 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    swoop_i18n::tr!("数据目录不得允许其他用户写入"),
                ));
            }
        }
        let directory = data.join(CONTROL_DIR);
        if let Ok(meta) = directory.symlink_metadata()
            && (!meta.is_dir() || meta.file_type().is_symlink())
        {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, swoop_i18n::tr!("控制目录必须是实际目录")));
        }
        std::fs::create_dir_all(&directory)?;
        restrict_permissions(&directory, true)?;
        let path = directory.join("instance.lock");
        reject_link(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&path)?;
        restrict_permissions(&path, false)?;
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => {
                // Clear a crashed owner's endpoint before opening SQLite.
                match std::fs::remove_file(directory.join("locator.json")) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                Ok(Some(Self { _lock: lock, directory }))
            }
            Err(error) if lock_busy(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn publish(&self, locator: &Locator) -> io::Result<()> {
        let path = self.directory.join("locator.json");
        reject_link(&path)?;
        // Only the lock owner writes this file. Clients retry an incomplete read
        // during initialization; they never use a stale unlocked locator.
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        restrict_permissions(&path, false)?;
        file.write_all(&serde_json::to_vec(locator)?)?;
        file.sync_all()
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Retain instance.lock so concurrent clients always contend on one inode.
        let _ = std::fs::remove_file(self.directory.join("locator.json"));
        // Release explicitly: a concurrent fork may briefly inherit this open
        // file description before CLOEXEC runs. Closing only the parent's fd
        // can otherwise leave a stale locator looking owned by that child.
        let _ = FileExt::unlock(&self._lock);
    }
}

fn lock_busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn reject_link(path: &Path) -> io::Result<()> {
    if let Ok(meta) = path.symlink_metadata()
        && (!meta.is_file() || meta.file_type().is_symlink())
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, swoop_i18n::tr!("控制文件不是普通文件")));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }))
}

#[cfg(windows)]
fn restrict_permissions(path: &Path, _directory: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };
    // Protected DACL: only the file owner, inherited by newly created children.
    // No token exists until the private directory's DACL has been installed.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = (result == 0).then(io::Error::last_os_error);
        LocalFree(descriptor);
        error.map_or(Ok(()), Err)
    }
}

#[derive(Serialize, Deserialize)]
struct Locator {
    protocol: u32,
    port: u16,
    token: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConflictAction {
    Ask,
    Rename,
    Overwrite,
    Skip,
}
impl ConflictAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ask" => Ok(Self::Ask),
            "rename" => Ok(Self::Rename),
            "overwrite" => Ok(Self::Overwrite),
            "skip" => Ok(Self::Skip),
            _ => Err(swoop_i18n::tr!("策略必须为 ask、rename、overwrite 或 skip").into()),
        }
    }
    pub fn core(self) -> swoop_core::FileConflictPolicy {
        match self {
            Self::Ask => swoop_core::FileConflictPolicy::Ask,
            Self::Rename => swoop_core::FileConflictPolicy::Rename,
            Self::Overwrite => swoop_core::FileConflictPolicy::Overwrite,
            Self::Skip => swoop_core::FileConflictPolicy::Skip,
        }
    }
}
fn conflict_policy_name(policy: swoop_core::FileConflictPolicy) -> &'static str {
    match policy {
        swoop_core::FileConflictPolicy::Ask => "ask",
        swoop_core::FileConflictPolicy::Rename => "rename",
        swoop_core::FileConflictPolicy::Overwrite => "overwrite",
        swoop_core::FileConflictPolicy::Skip => "skip",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    Status,
    Show,
    Resolve {
        id: u64,
        action: ConflictAction,
    },
    ConflictPolicy {
        policy: Option<ConflictAction>,
    },
    Add {
        url: String,
        directory: Option<PathBuf>,
        name: Option<String>,
        queue: Option<u64>,
    },
    Pause {
        id: u64,
    },
    Resume {
        id: u64,
    },
    QueueStart {
        id: u64,
    },
    QueueStop {
        id: u64,
    },
    MediaAdd {
        url: String,
        output: PathBuf,
        duration: Option<u64>,
        max_height: Option<u32>,
        audio_language: Option<String>,
        #[serde(default)]
        speed_limit_bps: Option<u64>,
    },
    MediaStatus {
        id: Option<u64>,
    },
    MediaLimit {
        id: u64,
        speed_limit_bps: u64,
    },
    MediaStop {
        id: u64,
    },
    MediaRetry {
        id: u64,
    },
    MediaResolve {
        id: u64,
        action: ConflictAction,
    },
    GrabberCreate {
        url: String,
        directory: PathBuf,
        max_depth: Option<u32>,
        max_pages: Option<usize>,
        max_bytes: Option<u64>,
        concurrency: Option<usize>,
    },
    GrabberOpen {
        directory: PathBuf,
    },
    GrabberRun,
    GrabberStop,
    GrabberStatus,
}

impl Command {
    pub fn desktop(&self) -> bool {
        matches!(
            self,
            Self::MediaAdd { .. }
                | Self::MediaStatus { .. }
                | Self::MediaStop { .. }
                | Self::MediaLimit { .. }
                | Self::MediaRetry { .. }
                | Self::MediaResolve { .. }
                | Self::GrabberCreate { .. }
                | Self::GrabberOpen { .. }
                | Self::GrabberRun
                | Self::GrabberStop
                | Self::GrabberStatus
        )
    }
    /// Validate both CLI input and raw authenticated HTTP requests before UI dispatch.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Add { url, .. } => download_url(url)?,
            Self::Resolve { id: 0, .. } | Self::MediaResolve { id: 0, .. } => {
                return Err(swoop_i18n::tr!("ID 必须是正整数").into());
            }
            Self::Resolve { action: ConflictAction::Ask, .. }
            | Self::MediaResolve { action: ConflictAction::Ask, .. } => {
                return Err(swoop_i18n::tr!("单次处理只能选择 rename、overwrite 或 skip").into());
            }
            Self::MediaAdd { url, output, duration, max_height, audio_language, .. } => {
                media_url(url)?;
                if !output.is_absolute()
                    || !output
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|v| v.eq_ignore_ascii_case("mp4") || v.eq_ignore_ascii_case("mkv"))
                {
                    return Err(swoop_i18n::tr!("媒体输出需要为绝对路径的 .mp4 或 .mkv 文件").into());
                }
                if *duration == Some(0) || max_height.is_some_and(|v| !(1..=8640).contains(&v)) {
                    return Err(swoop_i18n::tr!("录制秒数必须大于零，最大高度需为 1–8640").into());
                }
                if url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "mms" | "mmsh" | "mmst"))
                    && (duration.is_none()
                        || !output.extension().and_then(|v| v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case("mkv")))
                {
                    return Err(swoop_i18n::tr!("MMS 需要 MKV 输出和明确录制秒数").into());
                }
                if audio_language.as_ref().is_some_and(|v| {
                    v.is_empty()
                        || v.len() > 64
                        || !v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                }) {
                    return Err(swoop_i18n::tr!("音轨语言必须为 1–64 位语言标签").into());
                }
            }
            Self::GrabberCreate { url, directory, max_depth, max_pages, max_bytes, concurrency } => {
                network_url(url)?;
                if !directory.is_absolute() {
                    return Err(swoop_i18n::tr!("项目目录必须为绝对路径").into());
                }
                if max_depth.is_some_and(|v| v > 100)
                    || max_pages.is_some_and(|v| !(1..=100_000).contains(&v))
                    || *max_bytes == Some(0)
                    || concurrency.is_some_and(|v| !(1..=16).contains(&v))
                {
                    return Err(swoop_i18n::tr!("深度需 0–100、页数 1–100000、字节上限大于零、并发 1–16").into());
                }
            }
            Self::GrabberOpen { directory } if !directory.is_absolute() => {
                return Err(swoop_i18n::tr!("项目目录必须为绝对路径").into());
            }
            Self::MediaStatus { id: Some(0) }
            | Self::MediaStop { id: 0 }
            | Self::MediaLimit { id: 0, .. }
            | Self::MediaRetry { id: 0 } => {
                return Err(swoop_i18n::tr!("ID 必须是正整数").into());
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn network_url(value: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| swoop_i18n::tr!("需要有效的 HTTP(S) 链接"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(swoop_i18n::tr!("仅支持不含用户名密码的 HTTP(S) 链接").into());
    }
    Ok(())
}

pub fn download_url(value: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| swoop_i18n::tr!("需要有效的 HTTP(S)/FTP(S) 链接"))?;
    if matches!(parsed.scheme(), "ftp" | "ftps") {
        swoop_core::FtpTaskOptions::for_url(value)
            .map_err(|_| swoop_i18n::tr!("FTP 链接无效，请在桌面登录栏填写用户名密码").to_string())?;
        Ok(())
    } else {
        network_url(value)
    }
}

pub fn media_url(value: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| swoop_i18n::tr!("需要有效的媒体链接"))?;
    if !matches!(parsed.scheme(), "http" | "https" | "mms" | "mmsh" | "mmst")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(swoop_i18n::tr!("媒体链接仅支持 HTTP(S)/MMS，且不能内嵌用户名密码").into());
    }
    Ok(())
}

pub type ControlReply = tokio::sync::oneshot::Sender<Result<Value, String>>;
pub struct ControlRequest {
    command: Command,
    reply: ControlReply,
    deadline: Instant,
}
impl ControlRequest {
    pub fn respond(self, execute: impl FnOnce(Command) -> Result<Value, String>) {
        if self.reply.is_closed() {
            return;
        }
        let result = if Instant::now() >= self.deadline {
            Err(swoop_i18n::tr!("命令排队已过期，未执行").into())
        } else {
            execute(self.command)
        };
        let _ = self.reply.send(result);
    }
}
pub type DesktopControl = Arc<dyn Fn(ControlRequest) -> bool + Send + Sync>;
pub type Activate = Arc<dyn Fn() -> bool + Send + Sync>;
pub type DefaultDirectory = Arc<dyn Fn(&str, Option<&str>) -> PathBuf + Send + Sync>;

pub struct ControlServer {
    cancel: CancellationToken,
}

impl ControlServer {
    pub fn start(
        guard: &InstanceGuard,
        engine: Arc<Engine>,
        handle: &Handle,
        activate: Activate,
        directory: DefaultDirectory,
        desktop: DesktopControl,
    ) -> io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let listener = {
            let _entered = handle.enter();
            tokio::net::TcpListener::from_std(listener)?
        };
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        guard.publish(&Locator { protocol: 1, port, token: token.clone() })?;
        let shared = Arc::new(ControlState { engine, token, port, activate, directory, desktop });
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        handle.spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(16));
            loop {
                let accepted =
                    tokio::select! { biased; _ = stop.cancelled() => return, result = listener.accept() => result };
                let Ok((stream, peer)) = accepted else { return };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                let shared = shared.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let service = hyper::service::service_fn(move |request| serve_control(shared.clone(), request));
                    let connection =
                        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service);
                    tokio::select! { _ = stop.cancelled() => {}, _ = tokio::time::timeout(TIMEOUT, connection) => {} }
                });
            }
        });
        Ok(Self { cancel })
    }
}
impl Drop for ControlServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct ControlState {
    engine: Arc<Engine>,
    token: String,
    port: u16,
    activate: Activate,
    directory: DefaultDirectory,
    desktop: DesktopControl,
}

fn authenticated(headers: &hyper::HeaderMap, port: u16, token: &str) -> bool {
    if headers.contains_key(hyper::header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return false;
    }
    let expected_host = format!("127.0.0.1:{port}");
    if headers.get(hyper::header::HOST).and_then(|v| v.to_str().ok()) != Some(expected_host.as_str()) {
        return false;
    }
    let Some(value) =
        headers.get(hyper::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    // Equal-length constant-time comparison; never log supplied or expected tokens.
    value.len() == token.len() && value.bytes().zip(token.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

async fn serve_control(
    shared: Arc<ControlState>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if !authenticated(request.headers(), shared.port, &shared.token) {
        return Ok(response(StatusCode::FORBIDDEN, failure("forbidden", swoop_i18n::tr!("控制请求未授权"))));
    }
    if request.method() != Method::POST || request.uri().path() != "/v1/control" {
        return Ok(response(StatusCode::NOT_FOUND, failure("not_found", swoop_i18n::tr!("未知控制接口"))));
    }
    if request.headers().get(hyper::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) != Some("application/json") {
        return Ok(response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            failure("invalid_content_type", swoop_i18n::tr!("需要 application/json")),
        ));
    }
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => {
            return Ok(response(
                StatusCode::PAYLOAD_TOO_LARGE,
                failure("request_too_large", swoop_i18n::tr!("请求体过大或读取失败")),
            ));
        }
    };
    let command: Command = match serde_json::from_slice(&body) {
        Ok(command) => command,
        Err(_) => {
            return Ok(response(
                StatusCode::BAD_REQUEST,
                failure("invalid_command", swoop_i18n::tr!("控制命令格式无效")),
            ));
        }
    };
    let result = execute(&shared, command).await;
    Ok(match result {
        Ok(data) => response(StatusCode::OK, json!({"ok":true,"data":data})),
        Err(ControlFailure::Rejected(error)) => {
            response(StatusCode::UNPROCESSABLE_ENTITY, failure("command_failed", &error))
        }
        Err(ControlFailure::Unconfirmed) => response(
            StatusCode::GATEWAY_TIMEOUT,
            failure("unconfirmed", swoop_i18n::tr!("桌面操作结果未确认；请查询对应 status，不要直接重复创建命令")),
        ),
    })
}

enum ControlFailure {
    Rejected(String),
    Unconfirmed,
}
impl From<String> for ControlFailure {
    fn from(error: String) -> Self {
        Self::Rejected(error)
    }
}
impl From<&str> for ControlFailure {
    fn from(error: &str) -> Self {
        Self::Rejected(error.into())
    }
}

async fn execute(shared: &ControlState, command: Command) -> Result<Value, ControlFailure> {
    command.validate()?;
    if command.desktop() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let deadline = Instant::now() + Duration::from_secs(4);
        if !(shared.desktop)(ControlRequest { command, reply: tx, deadline }) {
            return Err(swoop_i18n::tr!("界面正在退出").into());
        }
        return tokio::time::timeout(Duration::from_secs(4), rx)
            .await
            .map_err(|_| ControlFailure::Unconfirmed)?
            .map_err(|_| ControlFailure::Unconfirmed)?
            .map_err(ControlFailure::Rejected);
    }
    let engine = &shared.engine;
    match command {
        Command::Status => {
            let tasks: Vec<_> = engine.snapshot().into_iter().map(|t| {
                let state = match t.status { TaskStatus::Queued => "queued", TaskStatus::Probing => "probing", TaskStatus::Downloading => "downloading", TaskStatus::Paused => "paused", TaskStatus::FileConflict => "waiting_for_confirmation", TaskStatus::Completed => "completed", TaskStatus::Failed(_) => "failed" };
                json!({"id":t.id.0,"name":t.file_name,"path":t.path,"status":state,"downloaded":t.downloaded,"total":t.total,"speed_bps":t.speed_bps})
            }).collect();
            let queues: Vec<_> = engine.queues().into_iter().map(|q| json!({"id":q.id.0,"name":q.name,"running":q.running,"max_concurrent":q.max_concurrent,"task_ids":q.task_ids,"active_count":q.active_count,"waiting_count":q.waiting_count})).collect();
            Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"tasks":tasks,"queues":queues,"file_conflict_policy":conflict_policy_name(engine.file_conflict_policy())}),
            )
        }
        Command::Resolve { id, action } => {
            engine.resolve_file_conflict(TaskId(id), action.core()).map_err(|e| e.to_string())?;
            Ok(json!({"id":id,"accepted":true}))
        }
        Command::ConflictPolicy { policy } => {
            if let Some(policy) = policy {
                engine.set_file_conflict_policy(policy.core()).map_err(|e| e.to_string())?;
            }
            Ok(json!({"policy":conflict_policy_name(engine.file_conflict_policy())}))
        }
        Command::Show => {
            if (shared.activate)() {
                Ok(json!({"shown":true}))
            } else {
                Err(swoop_i18n::tr!("界面正在退出").into())
            }
        }
        Command::Add { url, directory, name, queue } => {
            let directory = directory.unwrap_or_else(|| (shared.directory)(&url, name.as_deref()));
            if !directory.is_absolute() {
                return Err(swoop_i18n::tr!("下载目录必须为绝对路径").into());
            }
            if name.as_ref().is_some_and(|n| n.is_empty() || n.contains(['/', '\\']) || n == "." || n == "..") {
                return Err(swoop_i18n::tr!("文件名不得包含路径").into());
            }
            let id = engine
                .enqueue(
                    AddOptions { url, dir: directory, file_name: name, headers: vec![] },
                    QueueId(queue.unwrap_or(1)),
                )
                .map_err(|e| e.to_string())?;
            Ok(json!({"id":id.0}))
        }
        Command::Pause { id } => {
            engine.pause(TaskId(id)).map_err(|e| e.to_string())?;
            Ok(json!({"id":id}))
        }
        Command::Resume { id } => {
            engine.resume(TaskId(id)).map_err(|e| e.to_string())?;
            Ok(json!({"id":id}))
        }
        Command::QueueStart { id } => {
            engine.start_queue(QueueId(id)).map_err(|e| e.to_string())?;
            Ok(json!({"id":id}))
        }
        Command::QueueStop { id } => {
            engine.stop_queue(QueueId(id)).map_err(|e| e.to_string())?;
            Ok(json!({"id":id}))
        }
        _ => Err(swoop_i18n::tr!("未知控制命令").into()),
    }
}

fn response(status: StatusCode, body: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("static control response")
}

pub fn failure(code: &str, message: &str) -> Value {
    json!({"ok":false,"error":{"code":code,"message":message}})
}

fn read_locator(data: &Path) -> Result<Locator, &'static str> {
    let directory = data.join(CONTROL_DIR);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("instance.lock"))
        .map_err(|_| swoop_i18n::tr!("未找到运行中的 Swoop；请先启动桌面应用"))?;
    match FileExt::try_lock_exclusive(&lock) {
        Ok(()) => return Err(swoop_i18n::tr!("Swoop 未运行；请先启动桌面应用")),
        Err(e) if lock_busy(&e) => {}
        Err(_) => return Err(swoop_i18n::tr!("无法验证 Swoop 实例锁")),
    }
    let mut bytes = Vec::new();
    File::open(directory.join("locator.json"))
        .and_then(|f| f.take(4097).read_to_end(&mut bytes))
        .map_err(|_| swoop_i18n::tr!("Swoop 控制服务尚未就绪"))?;
    if bytes.len() > 4096 {
        return Err(swoop_i18n::tr!("Swoop 控制信息无效"));
    }
    let locator: Locator = serde_json::from_slice(&bytes).map_err(|_| swoop_i18n::tr!("Swoop 控制信息尚未就绪"))?;
    if locator.protocol != 1
        || locator.port == 0
        || locator.token.len() != 64
        || !locator.token.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(swoop_i18n::tr!("Swoop 控制信息无效"));
    }
    Ok(locator)
}

/// No proxy or redirects: a bearer token is sent only to the exact loopback endpoint.
/// A mutation is never retried after dispatch; an ambiguous transport failure is
/// returned to the caller so an add command cannot silently create duplicates.
pub async fn send(data: &Path, command: &Command, wait_for_startup: bool) -> Result<Value, String> {
    let mut locator = read_locator(data);
    if wait_for_startup {
        for _ in 0..30 {
            if locator.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            locator = read_locator(data);
        }
    }
    let locator = locator.map_err(str::to_string)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|_| swoop_i18n::tr!("无法初始化本机控制客户端"))?;
    let response = client
        .post(format!("http://127.0.0.1:{}/v1/control", locator.port))
        .bearer_auth(locator.token)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(command).map_err(|_| swoop_i18n::tr!("无法编码命令"))?)
        .send()
        .await
        .map_err(|_| swoop_i18n::tr!("控制请求未得到确认，请用 status 检查后再重试；不要直接重复 add"))?;
    let bytes = response.bytes().await.map_err(|_| swoop_i18n::tr!("读取控制响应失败"))?;
    serde_json::from_slice(&bytes).map_err(|_| swoop_i18n::tr!("控制响应不是有效 JSON").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn lock_child() {
        let Some(directory) = std::env::var_os("SWOOP_LOCK_CHILD") else {
            return;
        };
        let expect_owner = std::env::var_os("SWOOP_LOCK_EXPECT_OWNER").is_some();
        assert_eq!(InstanceGuard::acquire(Path::new(&directory)).unwrap().is_some(), expect_owner);
    }

    #[test]
    fn process_lock_is_exclusive_and_data_directories_are_independent() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let guard = InstanceGuard::acquire(first.path()).unwrap().unwrap();
        let independent = InstanceGuard::acquire(second.path()).unwrap().unwrap();
        let child = |owner| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "instance::tests::lock_child", "--nocapture"])
                .env("SWOOP_LOCK_CHILD", first.path())
                .env_remove("SWOOP_LOCK_EXPECT_OWNER");
            if owner {
                command.env("SWOOP_LOCK_EXPECT_OWNER", "1");
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        };
        child(false);
        drop(guard);
        child(true);
        drop(independent);
        assert!(!first.path().join("swoop.db").exists());
    }

    #[test]
    fn authenticated_commands_reject_browser_origins_and_preserve_bridge_separation() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let guard = InstanceGuard::acquire(directory.path()).unwrap().unwrap();
        let engine = Arc::new(Engine::new(runtime.handle().clone(), swoop_core::Config::default()));
        let shown = Arc::new(AtomicUsize::new(0));
        let counter = shown.clone();
        let downloads = directory.path().to_path_buf();
        let server = ControlServer::start(
            &guard,
            engine.clone(),
            runtime.handle(),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }),
            Arc::new(move |_, _| downloads.clone()),
            Arc::new(|request| {
                request.respond(|_| Err("desktop unavailable".into()));
                true
            }),
        )
        .unwrap();
        runtime.block_on(async {
            let locator = read_locator(directory.path()).unwrap();
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let endpoint = format!("http://127.0.0.1:{}/v1/control", locator.port);
            for origin in ["https://example.com", "chrome-extension://abc", "moz-extension://abc", "null"] {
                let response = client
                    .post(&endpoint)
                    .bearer_auth(&locator.token)
                    .header("Origin", origin)
                    .header("Content-Type", "application/json")
                    .body(r#"{"command":"show"}"#)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 403);
            }
            let response = client
                .post(&endpoint)
                .bearer_auth("wrong")
                .header("Content-Type", "application/json")
                .body(r#"{"command":"show"}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403);
            assert_eq!(shown.load(Ordering::SeqCst), 0);
            assert_eq!(send(directory.path(), &Command::Show, false).await.unwrap()["ok"], true);
            assert_eq!(shown.load(Ordering::SeqCst), 1);
            assert_eq!(send(directory.path(), &Command::QueueStop { id: 1 }, false).await.unwrap()["ok"], true);
            let result = send(
                directory.path(),
                &Command::Add {
                    url: "https://example.invalid/file?token=never-output".into(),
                    directory: None,
                    name: Some("文件.bin".into()),
                    queue: None,
                },
                false,
            )
            .await
            .unwrap();
            let id = result["data"]["id"].as_u64().unwrap();
            assert_eq!(send(directory.path(), &Command::Pause { id }, false).await.unwrap()["ok"], true);
            assert_eq!(send(directory.path(), &Command::Resume { id }, false).await.unwrap()["ok"], true);
            let status = send(directory.path(), &Command::Status, false).await.unwrap();
            assert_eq!(status["data"]["tasks"][0]["id"], id);
            assert!(!status.to_string().contains("never-output"));
            assert_eq!(send(directory.path(), &Command::Pause { id: u64::MAX }, false).await.unwrap()["ok"], false);
            engine.pause(TaskId(id)).unwrap();
            assert_eq!(send(directory.path(), &Command::QueueStart { id: 1 }, false).await.unwrap()["ok"], true);
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(directory.path().join(CONTROL_DIR)).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(directory.path().join(CONTROL_DIR).join("locator.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(server);
        drop(runtime);
        drop(guard);
        assert!(read_locator(directory.path()).is_err());
        assert!(!directory.path().join(CONTROL_DIR).join("locator.json").exists());
        assert!(directory.path().join(CONTROL_DIR).join("instance.lock").exists());
    }

    #[test]
    fn stale_locator_never_establishes_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let guard = InstanceGuard::acquire(directory.path()).unwrap().unwrap();
        guard.publish(&Locator { protocol: 1, port: 1234, token: "a".repeat(64) }).unwrap();
        assert!(read_locator(directory.path()).is_ok());
        // Emulate the duplicate fd held between fork and exec in another thread.
        let inherited = guard._lock.try_clone().unwrap();
        drop(guard);
        std::fs::write(
            directory.path().join(CONTROL_DIR).join("locator.json"),
            r#"{"protocol":1,"port":1234,"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert!(read_locator(directory.path()).is_err());
        drop(inherited);
    }
    #[test]
    fn expired_or_abandoned_desktop_requests_never_execute() {
        let (reply, receiver) = tokio::sync::oneshot::channel();
        ControlRequest { command: Command::GrabberRun, reply, deadline: Instant::now() - Duration::from_millis(1) }
            .respond(|_| panic!("expired request must not run"));
        assert!(receiver.blocking_recv().unwrap().is_err());
        let (reply, receiver) = tokio::sync::oneshot::channel();
        drop(receiver);
        ControlRequest { command: Command::GrabberRun, reply, deadline: Instant::now() + Duration::from_secs(1) }
            .respond(|_| panic!("abandoned request must not run"));
    }

    #[test]
    fn desktop_http_timeout_is_unconfirmed_and_invalid_parameters_never_queue() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let guard = InstanceGuard::acquire(directory.path()).unwrap().unwrap();
        let engine = Arc::new(Engine::new(runtime.handle().clone(), swoop_core::Config::default()));
        let pending = Arc::new(std::sync::Mutex::new(Vec::<ControlRequest>::new()));
        let queued = pending.clone();
        let output = directory.path().to_owned();
        let _server = ControlServer::start(
            &guard,
            engine,
            runtime.handle(),
            Arc::new(|| true),
            Arc::new(move |_, _| output.clone()),
            Arc::new(move |request| {
                queued.lock().unwrap().push(request);
                true
            }),
        )
        .unwrap();
        runtime.block_on(async {
            for command in [
                Command::MediaAdd {
                    url: "https://media.test/a.m3u8".into(),
                    output: "relative.mp4".into(),
                    duration: None,
                    max_height: None,
                    audio_language: None,
                    speed_limit_bps: None,
                },
                Command::GrabberCreate {
                    url: "file:///tmp/secret".into(),
                    directory: directory.path().join("project"),
                    max_depth: None,
                    max_pages: None,
                    max_bytes: None,
                    concurrency: None,
                },
                Command::MediaStop { id: 0 },
            ] {
                let value = send(directory.path(), &command, false).await.unwrap();
                assert_eq!(value["error"]["code"], "command_failed");
            }
            assert!(pending.lock().unwrap().is_empty());
            let response = send(directory.path(), &Command::GrabberStatus, false).await.unwrap();
            assert_eq!(response["error"]["code"], "unconfirmed");
        });
        pending.lock().unwrap().pop().unwrap().respond(|_| panic!("timed-out mutation must not execute later"));
    }
    #[test]
    fn real_http_resolves_conflicts_without_early_writes_and_persists_policy() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(swoop_core::Store::in_memory().unwrap());
        let engine =
            Arc::new(Engine::with_store(runtime.handle().clone(), swoop_core::Config::default(), store.clone()));
        let guard = InstanceGuard::acquire(directory.path()).unwrap().unwrap();
        let output = directory.path().to_owned();
        let _server = ControlServer::start(
            &guard,
            engine.clone(),
            runtime.handle(),
            Arc::new(|| true),
            Arc::new(move |_, _| output.clone()),
            Arc::new(|request| {
                request.respond(|_| Err("unused".into()));
                true
            }),
        )
        .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(|_| async {
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .header("Content-Length", "11")
                                    .body(Full::new(Bytes::from_static(b"replacement")))
                                    .unwrap(),
                            )
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
            async fn wait(engine: &Engine, id: TaskId, state: TaskStatus) {
                let deadline = Instant::now() + Duration::from_secs(10);
                while engine.snapshot_one(id).unwrap().status != state {
                    assert!(
                        Instant::now() < deadline,
                        "{:?}",
                        engine.snapshot_one(id).unwrap().status
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
            assert_eq!(
                send(
                    directory.path(),
                    &Command::ConflictPolicy { policy: None },
                    false
                )
                .await
                .unwrap()["data"]["policy"],
                "ask"
            );
            for (name, action) in [
                ("rename.bin", ConflictAction::Rename),
                ("overwrite.bin", ConflictAction::Overwrite),
                ("skip.bin", ConflictAction::Skip),
            ] {
                let path = directory.path().join(name);
                std::fs::write(&path, b"old bytes").unwrap();
                let result = send(
                    directory.path(),
                    &Command::Add {
                        url: url.clone(),
                        directory: None,
                        name: Some(name.into()),
                        queue: None,
                    },
                    false,
                )
                .await
                .unwrap();
                let id = TaskId(result["data"]["id"].as_u64().unwrap());
                wait(&engine, id, TaskStatus::FileConflict).await;
                let status = send(directory.path(), &Command::Status, false)
                    .await
                    .unwrap();
                let task = status["data"]["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["id"] == id.0)
                    .unwrap();
                assert_eq!(task["status"], "waiting_for_confirmation");
                assert_eq!(std::fs::read(&path).unwrap(), b"old bytes");
                assert_eq!(
                    send(
                        directory.path(),
                        &Command::Resolve {
                            id: id.0,
                            action: ConflictAction::Ask
                        },
                        false
                    )
                    .await
                    .unwrap()["ok"],
                    false
                );
                assert_eq!(
                    send(
                        directory.path(),
                        &Command::Resolve { id: id.0, action },
                        false
                    )
                    .await
                    .unwrap()["data"]["accepted"],
                    true
                );
                wait(
                    &engine,
                    id,
                    if action == ConflictAction::Skip {
                        TaskStatus::Paused
                    } else {
                        TaskStatus::Completed
                    },
                )
                .await;
                let task = engine.snapshot_one(id).unwrap();
                match action {
                    ConflictAction::Rename => {
                        assert_ne!(task.path, path);
                        assert_eq!(std::fs::read(&path).unwrap(), b"old bytes");
                        assert_eq!(std::fs::read(&task.path).unwrap(), b"replacement");
                    }
                    ConflictAction::Overwrite => {
                        assert_eq!(task.path, path);
                        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
                    }
                    ConflictAction::Skip => assert_eq!(std::fs::read(&path).unwrap(), b"old bytes"),
                    ConflictAction::Ask => unreachable!(),
                }
                assert_eq!(
                    engine.file_conflict_policy(),
                    swoop_core::FileConflictPolicy::Ask
                );
            }
            assert_eq!(
                send(
                    directory.path(),
                    &Command::Resolve {
                        id: 9999,
                        action: ConflictAction::Skip
                    },
                    false
                )
                .await
                .unwrap()["ok"],
                false
            );
            assert_eq!(
                send(
                    directory.path(),
                    &Command::ConflictPolicy {
                        policy: Some(ConflictAction::Rename)
                    },
                    false
                )
                .await
                .unwrap()["data"]["policy"],
                "rename"
            );
            assert_eq!(
                store
                    .get_setting("file_conflict_policy")
                    .unwrap()
                    .as_deref(),
                Some("rename")
            );
            assert_eq!(
                send(
                    directory.path(),
                    &Command::ConflictPolicy {
                        policy: Some(ConflictAction::Ask)
                    },
                    false
                )
                .await
                .unwrap()["data"]["policy"],
                "ask"
            );
        });
    }
}
