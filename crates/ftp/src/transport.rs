use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use curl::{
    easy::{Easy2, Handler, InfoType, WriteError},
    multi::Multi,
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{Error, FtpConfig, FtpMetadata, FtpProxyKind, FtpRequest, FtpTlsMode, Result};

mod ffi;

const CHUNK_LIMIT: usize = 16 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug)]
pub struct FtpClient {
    config: FtpConfig,
}

impl FtpClient {
    pub fn new(config: FtpConfig) -> Result<Self> {
        config.validate()?;
        curl::init();
        let version = curl::Version::get();
        let protocols: Vec<_> = version.protocols().collect();
        if !protocols.contains(&"ftp")
            || !protocols.contains(&"ftps")
            || !version.feature_async_dns()
        {
            return Err(Error::UnsupportedBuild);
        }
        Ok(Self { config })
    }

    pub fn probe(&self, request: FtpRequest) -> Result<Probe> {
        Ok(Probe {
            job: self.start(request, 0, None)?,
        })
    }

    /// The caller must establish that `offset` belongs to this remote representation.
    /// This API does not treat SIZE/MDTM as proof and does not decide when resume is safe.
    pub fn download(&self, request: FtpRequest, offset: u64) -> Result<Download> {
        if offset > i64::MAX as u64 {
            return Err(Error::InvalidConfig(
                "resume offset exceeds libcurl's signed 64-bit range",
            ));
        }
        let (tx, rx) = mpsc::channel(self.config.buffer_chunks);
        let job = self.start(request, offset, Some(tx))?;
        Ok(Download {
            job,
            rx,
            drained: false,
        })
    }

    fn start(
        &self,
        request: FtpRequest,
        offset: u64,
        tx: Option<mpsc::Sender<Bytes>>,
    ) -> Result<Job> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::MissingRuntime)?;
        let signal = Arc::new(Signal::default());
        let worker_signal = signal.clone();
        let config = self.config.clone();
        let handle =
            runtime.spawn_blocking(move || run(config, request, offset, tx, worker_signal));
        Ok(Job {
            signal,
            handle: Some(handle),
            result: None,
        })
    }
}

pub struct Probe {
    job: Job,
}

impl Probe {
    /// Waits for handle destruction and the worker to exit. Cancellation-safe to poll repeatedly.
    pub async fn finish(&mut self) -> Result<FtpMetadata> {
        self.job.finish().await
    }

    /// Returns only after the libcurl handle has been destroyed and its worker has exited.
    pub async fn cancel(&mut self) -> Result<()> {
        self.job.cancel().await
    }
}

pub struct Download {
    job: Job,
    rx: mpsc::Receiver<Bytes>,
    drained: bool,
}

impl Download {
    /// EOF only means no more data. Call `finish` to inspect the final FTP response.
    /// Polling and then dropping this future does not lose a chunk.
    pub async fn next_chunk(&mut self) -> Option<Bytes> {
        let chunk = self.rx.recv().await;
        self.drained = chunk.is_none();
        self.job.signal.wake.notify_one();
        chunk
    }

    /// Refuses success until the caller has consumed all delivered chunks and observed EOF.
    pub async fn finish(&mut self) -> Result<FtpMetadata> {
        if !self.drained {
            return Err(Error::NotDrained);
        }
        self.job.finish().await
    }

    /// Discards pending chunks and waits for the worker/handle to exit. No file is modified here.
    pub async fn cancel(&mut self) -> Result<()> {
        self.job.signal.cancel();
        self.rx.close();
        while self.rx.try_recv().is_ok() {}
        self.drained = true;
        self.job.cancel().await
    }
}

#[derive(Default)]
struct Signal {
    cancelled: AtomicBool,
    wake: Condvar,
    lock: Mutex<()>,
}

impl Signal {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn brief_wait(&self) {
        if let Ok(guard) = self.lock.lock() {
            // The predicate and short bound make missed wakeups harmless.
            let _ = self
                .wake
                .wait_timeout_while(guard, POLL_INTERVAL, |_| !self.is_cancelled());
        }
    }
}

struct Job {
    signal: Arc<Signal>,
    handle: Option<JoinHandle<Result<FtpMetadata>>>,
    result: Option<Result<FtpMetadata>>,
}

impl Job {
    async fn finish(&mut self) -> Result<FtpMetadata> {
        if let Some(result) = &self.result {
            return result.clone();
        }
        // Await by reference: cancellation of this future must not detach the JoinHandle.
        let result = match self.handle.as_mut() {
            Some(handle) => handle
                .await
                .map_err(|_| Error::Worker)
                .and_then(|result| result),
            None => Err(Error::Worker),
        };
        self.handle = None;
        self.result = Some(result.clone());
        result
    }

    async fn cancel(&mut self) -> Result<()> {
        self.signal.cancel();
        match self.finish().await {
            Err(Error::Worker) => Err(Error::Worker),
            _ => Ok(()), // Any transport result confirms the worker and its handles have exited.
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // Drop requests shutdown but cannot acknowledge it; callers requiring that use cancel().await.
        self.signal.cancel();
    }
}

struct Receiver {
    tx: Option<mpsc::Sender<Bytes>>,
    signal: Arc<Signal>,
    paused: bool,
}

impl Handler for Receiver {
    fn write(&mut self, data: &[u8]) -> std::result::Result<usize, WriteError> {
        if self.signal.is_cancelled() || data.len() > CHUNK_LIMIT {
            return Ok(0);
        }
        if data.is_empty() {
            return Ok(0);
        }
        // FTP NOBODY emits synthetic metadata headers through the write callback.
        // No RETR is sent in this mode, so these bytes can be discarded safely.
        let Some(tx) = &self.tx else {
            return Ok(data.len());
        };
        // Reserve first so a full queue never allocates or accepts bytes that curl will redeliver.
        match tx.try_reserve() {
            Ok(permit) => {
                permit.send(Bytes::copy_from_slice(data));
                Ok(data.len())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.paused = true;
                Err(WriteError::Pause)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Ok(0),
        }
    }

    fn progress(&mut self, _: f64, _: f64, _: f64, _: f64) -> bool {
        !self.signal.is_cancelled()
    }

    fn debug(&mut self, _: InfoType, _: &[u8]) {
        // Never print control-channel commands, passwords or arbitrary server responses.
    }
}

fn transfer_error(error: curl::Error) -> Error {
    Error::Transfer {
        code: error.code(),
        status: 0,
    }
}

fn multi_error(error: curl::MultiError) -> Error {
    Error::Multi(error.code() as u32)
}

fn configured_handle(
    config: &FtpConfig,
    request: FtpRequest,
    offset: u64,
    tx: Option<mpsc::Sender<Bytes>>,
    signal: Arc<Signal>,
) -> Result<Easy2<Receiver>> {
    let probe = tx.is_none();
    let mut easy = Easy2::new(Receiver {
        tx,
        signal,
        paused: false,
    });
    let url = if request.tls == FtpTlsMode::Implicit && request.url.scheme() == "ftp" {
        request.url.as_str().replacen("ftp://", "ftps://", 1)
    } else {
        request.url.to_string()
    };
    easy.url(&url).map_err(transfer_error)?;
    easy.progress(true).map_err(transfer_error)?;
    easy.signal(false).map_err(transfer_error)?;
    easy.verbose(false).map_err(transfer_error)?;
    easy.follow_location(false).map_err(transfer_error)?;
    easy.ssl_verify_peer(true).map_err(transfer_error)?;
    easy.ssl_verify_host(true).map_err(transfer_error)?;
    easy.proxy_ssl_verify_peer(true).map_err(transfer_error)?;
    easy.proxy_ssl_verify_host(true).map_err(transfer_error)?;
    easy.connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .map_err(transfer_error)?;
    easy.low_speed_limit(1).map_err(transfer_error)?;
    easy.low_speed_time(Duration::from_secs(config.read_timeout_secs))
        .map_err(transfer_error)?;
    if let Some(seconds) = config.transfer_timeout_secs {
        easy.timeout(Duration::from_secs(seconds))
            .map_err(transfer_error)?;
    }
    if let Some(ca) = &config.ca_certificate {
        easy.cainfo(ca).map_err(transfer_error)?;
        if config
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.kind == FtpProxyKind::HttpsTunnel)
        {
            easy.proxy_cainfo(
                ca.to_str()
                    .ok_or(Error::InvalidConfig("HTTPS proxy CA path must be UTF-8"))?,
            )
            .map_err(transfer_error)?;
        }
    }
    easy.fetch_filetime(true).map_err(transfer_error)?;
    easy.nobody(probe).map_err(transfer_error)?;
    easy.buffer_size(CHUNK_LIMIT).map_err(transfer_error)?;
    if !probe {
        easy.resume_from(offset).map_err(transfer_error)?;
    }
    match request.credentials {
        Some(credentials) => {
            easy.username(&credentials.username)
                .map_err(transfer_error)?;
            easy.password(&credentials.password)
                .map_err(transfer_error)?;
        }
        None => {
            easy.username("anonymous").map_err(transfer_error)?;
            easy.password("swoop@localhost").map_err(transfer_error)?;
        }
    }
    // Disable environment-driven proxy/netrc choices. Explicit private credentials never fall back.
    easy.netrc(curl::easy::NetRc::Ignored)
        .map_err(transfer_error)?;
    easy.proxy("").map_err(transfer_error)?;
    easy.noproxy("").map_err(transfer_error)?;
    if let Some(proxy) = &config.proxy {
        let scheme = match proxy.kind {
            FtpProxyKind::HttpTunnel => "http",
            FtpProxyKind::HttpsTunnel => "https",
            FtpProxyKind::Socks5 => "socks5",
            FtpProxyKind::Socks5Hostname => "socks5h",
        };
        let host = if proxy.host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("[{}]", proxy.host)
        } else {
            proxy.host.clone()
        };
        easy.proxy(&format!("{scheme}://{host}:{}", proxy.port))
            .map_err(transfer_error)?;
        easy.noproxy(&proxy.bypass_hosts.join(","))
            .map_err(transfer_error)?;
        easy.http_proxy_tunnel(true).map_err(transfer_error)?;
    }
    ffi::configure(&easy, config, request.tls)?;
    Ok(easy)
}

fn run(
    config: FtpConfig,
    request: FtpRequest,
    offset: u64,
    tx: Option<mpsc::Sender<Bytes>>,
    signal: Arc<Signal>,
) -> Result<FtpMetadata> {
    if signal.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let easy = configured_handle(&config, request, offset, tx, signal.clone())?;
    let multi = Multi::new();
    let mut handle = multi.add2(easy).map_err(multi_error)?;
    loop {
        if signal.is_cancelled() {
            drop(multi.remove2(handle).map_err(multi_error)?);
            return Err(Error::Cancelled);
        }
        if handle.get_ref().paused
            && handle
                .get_ref()
                .tx
                .as_ref()
                .is_some_and(|tx| tx.capacity() > 0)
        {
            handle.get_mut().paused = false;
            handle.unpause_write().map_err(transfer_error)?;
        }
        multi.perform().map_err(multi_error)?;
        let mut completion = None;
        multi.messages(|message| {
            if let Some(result) = message.result_for2(&handle) {
                completion = Some(result);
            }
        });
        if let Some(completion) = completion {
            let status = handle.response_code().unwrap_or(0);
            let easy = multi.remove2(handle).map_err(multi_error)?;
            if signal.is_cancelled() {
                return Err(Error::Cancelled);
            }
            completion.map_err(|error| Error::Transfer {
                code: error.code(),
                status,
            })?;
            if status >= 400 {
                return Err(Error::Transfer { code: 0, status });
            }
            let mut metadata = ffi::metadata(&easy)?;
            // libcurl reports the remaining body length after REST. When SIZE says the
            // requested offset is already EOF it skips RETR and retains the original SIZE.
            if offset > 0 && matches!(status, 226 | 250) {
                metadata.size = metadata.size.and_then(|size| size.checked_add(offset));
            }
            return Ok(metadata);
        }
        let before = Instant::now();
        multi.wait(&mut [], POLL_INTERVAL).map_err(multi_error)?;
        if before.elapsed() < Duration::from_millis(1) {
            signal.brief_wait();
        }
    }
}
