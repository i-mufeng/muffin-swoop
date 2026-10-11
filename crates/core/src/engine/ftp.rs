//! FTP 的单连接传输适配；文件归属、队列和发布仍由引擎管理。
use std::fs::File;
use std::sync::atomic::Ordering;

use swoop_ftp::{FtpClient, FtpRequest};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{Engine, Inner, TaskShared, record_of};
use crate::network::{NetworkPolicy, retryable, wait_retry};
use crate::probe::{ProbeInfo, file_name_from_url};
use crate::segment::SegmentState;
use crate::util::sanitize_file_name;
use crate::{Error, FtpCredentials, FtpProxy, FtpProxyKind, FtpProxyPolicy, FtpTaskOptions, Result, TaskId};

impl Engine {
    pub fn ftp_defaults(&self) -> FtpTaskOptions {
        self.inner.ftp_defaults.lock().unwrap().clone()
    }

    /// 默认配置不得携带站点用户名，密码也不存在于该类型中；只影响以后创建的任务。
    pub fn set_ftp_defaults(&self, options: FtpTaskOptions) -> Result<()> {
        if options.username.is_some() {
            return Err(Error::InvalidTaskUpdate("FTP 默认配置不能包含用户名".into()));
        }
        options.validate("ftp://localhost/default")?;
        let mut defaults = self.inner.ftp_defaults.lock().unwrap();
        if let Some(store) = &self.inner.store {
            store.set_setting("ftp_defaults", &serde_json::to_string(&options)?)?;
        }
        *defaults = options;
        Ok(())
    }

    pub(super) fn ftp_options_for_url(&self, url: &Url) -> Result<FtpTaskOptions> {
        let mut options = self.ftp_defaults();
        if url.scheme() == "ftps" {
            options.tls = crate::FtpTlsMode::Implicit;
        }
        options.validate(url.as_str())?;
        Ok(options)
    }

    pub fn ftp_task_options(&self, id: TaskId) -> Result<Option<FtpTaskOptions>> {
        let tasks = self.inner.tasks.lock().unwrap();
        let task = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        Ok(task.shared.ftp_options.lock().unwrap().clone())
    }

    pub fn ftp_auth_required(&self, id: TaskId) -> Result<bool> {
        let tasks = self.inner.tasks.lock().unwrap();
        let task = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        Ok(task.shared.ftp_options.lock().unwrap().as_ref().is_some_and(|options| options.username.is_some())
            && task.shared.ftp_credentials.lock().unwrap().is_none())
    }

    /// 同时更新配置与会话认证，不给已排队任务留下配置/密码不一致的调度窗口。
    /// credentials=None 仅在用户名和 TLS 模式未变时保留已有会话凭据。
    pub fn configure_ftp_task(
        &self,
        id: TaskId,
        mut options: FtpTaskOptions,
        credentials: Option<FtpCredentials>,
    ) -> Result<()> {
        let tasks = self.inner.tasks.lock().unwrap();
        let task = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        if task.running {
            return Err(Error::InvalidTaskUpdate("请先暂停 FTP 任务并等待运行结束".into()));
        }
        if let Some(credentials) = &credentials {
            if options.username.as_deref().is_some_and(|name| name != credentials.username()) {
                return Err(Error::InvalidTaskUpdate("FTP 凭据用户名与任务配置不符".into()));
            }
            options.username = Some(credentials.username().to_owned());
        }
        options.validate(task.shared.url.as_str())?;
        let previous = task
            .shared
            .ftp_options
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::InvalidTaskUpdate("不是 FTP 任务".into()))?;
        let credentials = credentials.or_else(|| {
            (previous.username == options.username && previous.tls == options.tls)
                .then(|| task.shared.ftp_credentials.lock().unwrap().clone())
                .flatten()
        });
        let mut record = record_of(&task.shared);
        record.ftp_options = Some(options.clone());
        record.source_changed = true;
        if let Some(store) = &self.inner.store {
            store.save_task(&record)?;
        }
        *task.shared.ftp_options.lock().unwrap() = Some(options);
        *task.shared.ftp_credentials.lock().unwrap() = credentials;
        task.shared.source_changed.store(true, Ordering::Relaxed);
        let _ = self.inner.events.send(super::Event::Changed(id));
        Ok(())
    }

    /// 只修改未运行任务。用户名或 TLS 模式变化时清除会话密码。
    pub fn set_ftp_task_options(&self, id: TaskId, options: FtpTaskOptions) -> Result<()> {
        self.configure_ftp_task(id, options, None)
    }

    /// 会话凭据只进内存；首次指定用户名时仅把用户名记入任务，重启后要求再次认证。
    /// 设置凭据不会自动启动任务，调用方仍需显式 resume。
    pub fn set_ftp_credentials(&self, id: TaskId, credentials: Option<FtpCredentials>) -> Result<()> {
        let tasks = self.inner.tasks.lock().unwrap();
        let task = tasks.get(&id).ok_or(Error::TaskNotFound)?;
        if task.running {
            return Err(Error::InvalidTaskUpdate("请先暂停 FTP 任务并等待运行结束".into()));
        }
        let mut options = task
            .shared
            .ftp_options
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::InvalidTaskUpdate("不是 FTP 任务".into()))?;
        if let Some(credentials) = &credentials {
            if options.username.as_deref().is_some_and(|name| name != credentials.username()) {
                return Err(Error::InvalidTaskUpdate("FTP 凭据用户名与任务配置不符".into()));
            }
            options.username = Some(credentials.username().to_owned());
        }
        let mut record = record_of(&task.shared);
        record.ftp_options = Some(options.clone());
        record.source_changed = true;
        if let Some(store) = &self.inner.store {
            store.save_task(&record)?;
        }
        *task.shared.ftp_options.lock().unwrap() = Some(options);
        *task.shared.ftp_credentials.lock().unwrap() = credentials;
        task.shared.source_changed.store(true, Ordering::Relaxed);
        let _ = self.inner.events.send(super::Event::Changed(id));
        Ok(())
    }
}

pub(super) struct Runtime {
    client: FtpClient,
    request: FtpRequest,
    url: Url,
}
impl Runtime {
    pub(super) fn new(
        url: &Url,
        options: &FtpTaskOptions,
        credentials: Option<FtpCredentials>,
        network: &NetworkPolicy,
    ) -> Result<Self> {
        options.validate(url.as_str())?;
        if let Some(username) = &options.username
            && credentials.as_ref().is_none_or(|credentials| credentials.username() != username)
        {
            return Err(Error::FtpAuthenticationRequired);
        }
        let mut config = options.config.clone();
        config.proxy = match &options.proxy {
            FtpProxyPolicy::Direct => None,
            FtpProxyPolicy::Custom(proxy) => {
                if let Some(rule) = &network.proxy_auth {
                    let endpoint = Url::parse(&rule.endpoint)?;
                    let (scheme, default_port) = match proxy.kind {
                        FtpProxyKind::HttpTunnel => ("http", 80),
                        FtpProxyKind::HttpsTunnel => ("https", 443),
                        FtpProxyKind::Socks5 => ("socks5", 1080),
                        FtpProxyKind::Socks5Hostname => ("socks5h", 1080),
                    };
                    if endpoint.scheme() == scheme
                        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case(&proxy.host))
                        && endpoint.port().unwrap_or(default_port) == proxy.port
                    {
                        return Err(Error::InvalidNetwork(
                            "FTP 暂不支持此代理账号，请选择 FTP 直连或其他无认证代理".into(),
                        ));
                    }
                }
                Some(proxy.clone())
            }
            FtpProxyPolicy::Network => match &network.proxy {
                Some(proxy) => {
                    if network.proxy_auth.is_some() {
                        return Err(Error::InvalidNetwork(
                            "FTP 暂不支持继承代理账号，请选择 FTP 直连或无认证代理".into(),
                        ));
                    }
                    let url = Url::parse(&proxy.url).map_err(|_| Error::InvalidNetwork("代理地址无效".into()))?;
                    let kind = match url.scheme() {
                        "http" => FtpProxyKind::HttpTunnel,
                        "https" => FtpProxyKind::HttpsTunnel,
                        "socks5" => FtpProxyKind::Socks5,
                        "socks5h" => FtpProxyKind::Socks5Hostname,
                        _ => return Err(Error::InvalidNetwork("FTP 代理类型无效".into())),
                    };
                    Some(FtpProxy {
                        kind,
                        host: url.host_str().ok_or_else(|| Error::InvalidNetwork("代理地址无效".into()))?.to_owned(),
                        port: url.port().unwrap_or(match kind {
                            FtpProxyKind::HttpTunnel => 80,
                            FtpProxyKind::HttpsTunnel => 443,
                            _ => 1080,
                        }),
                        bypass_hosts: proxy.bypass.clone(),
                    })
                }
                None if network.use_system_proxy => {
                    return Err(Error::InvalidNetwork(
                        "FTP 暂不支持自动系统代理，请明确选择 FTP 直连或指定代理".into(),
                    ));
                }
                None => None,
            },
        };
        let client = FtpClient::new(config)?;
        let mut request = FtpRequest::new(url.as_str())?.with_tls(options.tls)?;
        if let Some(credentials) = credentials {
            request = request.with_credentials(credentials);
        }
        Ok(Self { client, request, url: url.clone() })
    }

    pub(super) async fn probe_with_retry(
        &self,
        policy: &NetworkPolicy,
        cancel: &CancellationToken,
    ) -> Result<ProbeInfo> {
        let mut attempt = 0;
        loop {
            let mut probe = self.client.probe(self.request.clone())?;
            let result = tokio::select! {
                result = probe.finish() => result.map_err(Error::from),
                _ = cancel.cancelled() => {
                    probe.cancel().await?;
                    return Err(Error::Cancelled);
                }
            };
            match result {
                Ok(info) => {
                    return Ok(ProbeInfo {
                        final_url: self.url.clone(),
                        file_name: sanitize_file_name(
                            &file_name_from_url(&self.url).unwrap_or_else(|| "download".into()),
                        ),
                        size: info.size,
                        resumable: false,
                        // SIZE/MDTM 均不是强内容身份，不能让同步快捷分支跳过实际下载。
                        etag: None,
                        last_modified: None,
                        content_type: None,
                    });
                }
                Err(error) if attempt < policy.retries && retryable(&error) => {
                    attempt += 1;
                    wait_retry(policy, attempt, cancel).await?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub(super) async fn download(
    inner: &Inner,
    shared: &TaskShared,
    file: &File,
    idx: usize,
    cancel: &CancellationToken,
    runtime: &Runtime,
) -> Result<()> {
    // 每次失败重试都完整重读；已有片段从不凭 SIZE/MDTM 或 REST 能力被复用。
    let expected = shared.sync_probe.lock().unwrap().as_ref().and_then(|info| info.size);
    {
        let mut map = shared.segments.lock().unwrap();
        map.segments[idx].start = 0;
        map.segments[idx].pos = 0;
        map.segments[idx].state = SegmentState::Active;
        shared.downloaded.store(0, Ordering::Relaxed);
    }
    // 每次重置也先存进度；失败时禁止截断旧的部分文件。
    if let Some(store) = &inner.store {
        store.save_task(&record_of(shared))?;
    }
    file.set_len(expected.unwrap_or(0))?;
    let mut download = runtime.client.download(runtime.request.clone(), 0)?;
    let result = async {
        let mut pos = 0;
        loop {
            let chunk = tokio::select! {
                chunk = download.next_chunk() => chunk,
                _ = cancel.cancelled() => return Err(Error::Cancelled),
            };
            let Some(chunk) = chunk else { break };
            if expected.is_some_and(|size| pos + chunk.len() as u64 > size) {
                return Err(Error::FtpContentChanged);
            }
            super::write_chunk(inner, shared, file, idx, &chunk, &mut pos, cancel).await?;
        }
        let metadata = tokio::select! {
            result = download.finish() => result?,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        if expected.is_some_and(|size| size != pos) || metadata.size.is_some_and(|size| size != pos) {
            return Err(Error::FtpContentChanged);
        }
        file.set_len(pos)?;
        let mut map = shared.segments.lock().unwrap();
        let segment = &mut map.segments[idx];
        segment.end = pos;
        segment.state = SegmentState::Done;
        shared.total.store(pos, Ordering::Relaxed);
        Ok(())
    }
    .await;
    if result.is_err() {
        // 包括 quota/限速等待取消、写入故障和最终 FTP 响应失败；租约释放前确认线程退出。
        download.cancel().await?;
    }
    result
}
