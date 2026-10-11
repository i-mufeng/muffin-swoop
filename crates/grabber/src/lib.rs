//! UI 无关、默认同源的可恢复站点抓取器。所有网络请求均为不带凭据的 GET。
mod css;
mod html;
mod manifest;
mod renderer;
mod srcset;
mod text_encoding;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use url::Url;

const PRIVATE: &str = ".swoop-grabber";
const USER_AGENT: &str = "SwoopGrabber/0.1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("配置错误：{0}")]
    Config(String),
    #[error("项目文件错误：{0}")]
    Project(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Resource discovery choices; old projects receive the compatible defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureOptions {
    pub scripts: bool,
    pub media: bool,
    pub embedded_documents: bool,
    pub manifests: bool,
    /// Common data-src/data-original/data-srcset image attributes.
    pub lazy_images: bool,
    /// Removes script elements only; this is not an HTML sanitizer or sandbox.
    pub strip_scripts_from_mirror: bool,
}
impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            scripts: true,
            media: true,
            embedded_documents: true,
            manifests: true,
            lazy_images: true,
            strip_scripts_from_mirror: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum PageSource {
    #[default]
    Network,
    RenderedSnapshot,
    BrowserRendered,
}

/// Optional isolated Chromium rendering. Only enabled through explicit project configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RenderOptions {
    pub enabled: bool,
    pub executable: PathBuf,
    pub every_html_page: bool,
    pub timeout_secs: u64,
    pub virtual_time_ms: u64,
    pub max_dom_bytes: u64,
    /// Browser proxy response bytes, including TLS framing, separate from downloaded object bytes.
    pub max_network_bytes: u64,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: PathBuf::new(),
            every_html_page: false,
            timeout_secs: 30,
            virtual_time_ms: 3000,
            max_dom_bytes: 8 * 1024 * 1024,
            max_network_bytes: 100 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GrabberConfig {
    pub start_url: String,
    pub output_dir: PathBuf,
    pub max_depth: u32,
    /// 包括 HTML 页面和资源文件。
    pub max_pages: usize,
    /// 接纳到预算中的响应体字节，包括 robots、失败/取消请求；重启后累计。
    pub max_total_bytes: u64,
    pub concurrency: usize,
    /// 额外允许的精确域名；空列表表示仅起始 origin（包括端口）。
    pub allowed_domains: Vec<String>,
    /// 空列表允许所有静态资源；非空仅允许这些扩展名，不限制页面导航。
    pub resource_extensions: Vec<String>,
    pub respect_robots: bool,
    pub max_file_bytes: u64,
    pub request_timeout_secs: u64,
    pub capture: CaptureOptions,
    /// Case-sensitive URL or path globs; '*' matches any sequence, '?' one character.
    pub excluded_url_patterns: Vec<String>,
    pub max_redirects: u32,
    pub render: RenderOptions,
}
impl Default for GrabberConfig {
    fn default() -> Self {
        Self {
            start_url: String::new(),
            output_dir: PathBuf::new(),
            max_depth: 2,
            max_pages: 200,
            max_total_bytes: 100 * 1024 * 1024,
            concurrency: 4,
            allowed_domains: vec![],
            resource_extensions: vec![],
            respect_robots: true,
            max_file_bytes: 16 * 1024 * 1024,
            request_timeout_secs: 20,
            capture: CaptureOptions::default(),
            excluded_url_patterns: vec![],
            max_redirects: 5,
            render: RenderOptions::default(),
        }
    }
}
impl GrabberConfig {
    fn validate(&self) -> Result<Url> {
        let start = canonical(&self.start_url).map_err(Error::Config)?;
        if self.output_dir.as_os_str().is_empty() {
            return Err(Error::Config("请选择输出目录".into()));
        }
        if self.max_pages == 0
            || self.max_pages > 100_000
            || self.max_depth > 100
            || !(1..=16).contains(&self.concurrency)
            || self.max_total_bytes == 0
            || !(1..=256 * 1024 * 1024).contains(&self.max_file_bytes)
            || !(1..=120).contains(&self.request_timeout_secs)
        {
            return Err(Error::Config(
                "页数需 1–100000、深度 0–100、并发 1–16、字节上限大于零、单文件不超过 256 MiB、超时 1–120 秒".into(),
            ));
        }
        if self.max_redirects > 20
            || self.excluded_url_patterns.len() > 100
            || self
                .excluded_url_patterns
                .iter()
                .any(|pattern| pattern.is_empty() || pattern.len() > 2048 || pattern.chars().any(char::is_control))
        {
            return Err(Error::Config("重定向上限 0–20；最多 100 条非空 URL 排除规则，每条不超过 2048 字节".into()));
        }
        if self.render.enabled
            && (!self.render.executable.is_absolute()
                || !self.render.executable.is_file()
                || !(1..=120).contains(&self.render.timeout_secs)
                || !(100..=60_000).contains(&self.render.virtual_time_ms)
                || !(1..=64 * 1024 * 1024).contains(&self.render.max_dom_bytes)
                || self.render.max_network_bytes == 0)
        {
            return Err(Error::Config("动态渲染需浏览器可执行文件绝对路径，超时 1–120 秒、虚拟时间 100–60000 毫秒、DOM 1 字节–64 MiB、浏览器流量预算大于零".into()));
        }
        for domain in &self.allowed_domains {
            if domain.is_empty() || domain.contains(['/', ':', '@', '*']) || url::Host::parse(domain).is_err() {
                return Err(Error::Config(format!("允许域名无效：{domain}")));
            }
        }
        Ok(start)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PageState {
    Pending,
    Downloaded,
    Failed(String),
    Skipped(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRecord {
    pub url: String,
    #[serde(default)]
    pub source: PageSource,
    /// Declared by link rel=manifest; not guessed from arbitrary JSON bodies.
    #[serde(default)]
    pub manifest: bool,
    pub depth: u32,
    pub state: PageState,
    pub final_url: Option<String>,
    pub content_type: Option<String>,
    /// 已确认的 HTML/CSS 原始字符集；旧项目默认为重新检查声明。
    #[serde(default)]
    pub encoding: Option<String>,
    pub bytes: u64,
    /// 项目私有对象名，仅哈希字符；不接受网页文件名作为本地路径。
    pub object: Option<String>,
    pub sha256: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectStatus {
    Ready,
    Running,
    Complete,
    Cancelled,
    Limited,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub config: GrabberConfig,
    pub revision: u64,
    pub status: ProjectStatus,
    pub pages: Vec<PageRecord>,
    pub received_bytes: u64,
    #[serde(default)]
    pub rendered_network_bytes: u64,
    pub skipped_links: u64,
    #[serde(default)]
    pub page_limit_reached: bool,
    pub mirror_dir: Option<PathBuf>,
    pub entry_path: Option<PathBuf>,
}
impl ProjectSnapshot {
    pub fn downloaded(&self) -> usize {
        self.pages.iter().filter(|page| page.state == PageState::Downloaded).count()
    }
    pub fn failed(&self) -> usize {
        self.pages.iter().filter(|page| matches!(page.state, PageState::Failed(_))).count()
    }
}

pub struct Grabber {
    state: ProjectSnapshot,
    updates: watch::Sender<ProjectSnapshot>,
}
impl Grabber {
    /// 含文件系统访问，请在应用后台线程调用。
    pub fn create(mut config: GrabberConfig) -> Result<Self> {
        let start = config.validate()?;
        config.start_url = start.to_string();
        std::fs::create_dir_all(&config.output_dir)?;
        config.output_dir = config.output_dir.canonicalize()?;
        let private = config.output_dir.join(PRIVATE);
        std::fs::create_dir(&private)
            .map_err(|e| Error::Project(format!("无法创建项目（已有项目请使用 open）：{e}")))?;
        std::fs::create_dir(private.join("objects"))?;
        let state = ProjectSnapshot {
            config,
            revision: 0,
            status: ProjectStatus::Ready,
            pages: vec![pending(start, 0)],
            received_bytes: 0,
            rendered_network_bytes: 0,
            skipped_links: 0,
            page_limit_reached: false,
            mirror_dir: None,
            entry_path: None,
        };
        let (updates, _) = watch::channel(state.clone());
        let mut grabber = Self { state, updates };
        grabber.save()?;
        Ok(grabber)
    }

    /// 从最近一个完整状态快照恢复；不会改写已有下载文件。
    pub fn open(project_dir: impl AsRef<Path>) -> Result<Self> {
        let root = project_dir.as_ref().canonicalize()?;
        let private = root.join(PRIVATE);
        ensure_real_dir(&private)?;
        ensure_real_dir(&private.join("objects"))?;
        let mut snapshots = std::fs::read_dir(&private)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let revision = name.strip_prefix("state-")?.strip_suffix(".json")?.parse::<u64>().ok()?;
                (!entry.file_type().ok()?.is_symlink()).then_some((revision, entry.path()))
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|(revision, _)| std::cmp::Reverse(*revision));
        let mut state = None;
        for (revision, path) in snapshots {
            if std::fs::metadata(&path)?.len() > 128 * 1024 * 1024 {
                continue;
            }
            if let Ok(parsed) = serde_json::from_slice::<ProjectSnapshot>(&std::fs::read(path)?)
                && parsed.revision == revision
            {
                state = Some(parsed);
                break;
            }
        }
        let mut state = state.ok_or_else(|| Error::Project("没有可恢复的完整状态快照".into()))?;
        state.config.output_dir = root;
        state.config.validate()?;
        if state.mirror_dir.as_ref().is_some_and(|path| !path.starts_with(&state.config.output_dir)) {
            state.mirror_dir = None;
            state.entry_path = None;
        }
        if state
            .entry_path
            .as_ref()
            .is_some_and(|path| state.mirror_dir.as_ref().is_none_or(|dir| path.parent() != Some(dir.as_path())))
        {
            state.entry_path = None;
        }
        if state.pages.len() > state.config.max_pages {
            return Err(Error::Project("任务条目超过配置上限".into()));
        }
        let mut seen = HashSet::new();
        for page in &mut state.pages {
            let parsed = canonical(&page.url).map_err(Error::Project)?;
            if parsed.as_str() != page.url || !seen.insert(page.url.clone()) {
                return Err(Error::Project("状态含不规范或重复 URL".into()));
            }
            if page.state == PageState::Downloaded
                && let Err(error) = verified_object(&private, page)
            {
                page.state = PageState::Failed(error.to_string());
            }
        }
        if state.status == ProjectStatus::Running {
            state.status = ProjectStatus::Ready;
        }
        let (updates, _) = watch::channel(state.clone());
        Ok(Self { state, updates })
    }

    pub fn snapshot(&self) -> ProjectSnapshot {
        self.state.clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<ProjectSnapshot> {
        self.updates.subscribe()
    }

    /// Import a DOM snapshot exported after browser rendering. This never starts a
    /// browser, executes JavaScript or imports browser cookies. `url` must already
    /// be a project entry so snapshots cannot bypass page/depth limits. Network
    /// discovery happens during the next `run` with the normal scope/robots rules.
    pub fn import_rendered_html(&mut self, url: &str, html: &str) -> Result<()> {
        let _lock = project_lock(&self.private())?;
        if Self::open(&self.state.config.output_dir)?.state.revision != self.state.revision {
            return Err(Error::Project("项目已被其他实例更新，请重新打开后导入快照".into()));
        }
        let url = canonical(url).map_err(Error::Config)?;
        let start = self.state.config.validate()?;
        if !allowed_url(&self.state.config, &start, &url) {
            return Err(Error::Config("快照 URL 超出项目允许范围或被排除".into()));
        }
        let index = self
            .state
            .pages
            .iter()
            .position(|page| page.url == url.as_str())
            .ok_or_else(|| Error::Config("先将该页面加入项目；快照仅可替换已有页面条目".into()))?;
        let bytes = html.as_bytes();
        if bytes.is_empty() || bytes.len() as u64 > self.state.config.max_file_bytes {
            return Err(Error::Config("HTML 快照不能为空或超过单文件上限".into()));
        }
        html::links(html, &url, &self.state.config.capture).map_err(Error::Config)?;
        let sha256 = hash(bytes);
        let page = &self.state.pages[index];
        if page.source == PageSource::RenderedSnapshot
            && page.sha256.as_deref() == Some(&sha256)
            && page.state == PageState::Downloaded
        {
            verified_object(&self.private(), page)?;
            return Ok(());
        }
        let used = self
            .state
            .received_bytes
            .checked_add(bytes.len() as u64)
            .filter(|bytes| *bytes <= self.state.config.max_total_bytes)
            .ok_or_else(|| Error::Config("HTML 快照超过项目剩余字节预算".into()))?;
        let object = format!("{}-{sha256}.raw", hash(url.as_str().as_bytes()));
        write_unique_or_same(&self.private().join("objects").join(&object), bytes)?;
        let previous = self.state.clone();
        let page = &mut self.state.pages[index];
        page.source = PageSource::RenderedSnapshot;
        page.manifest = false;
        page.state = PageState::Downloaded;
        page.final_url = Some(url.to_string());
        page.content_type = Some("text/html".into());
        page.encoding = Some("UTF-8".into());
        page.bytes = bytes.len() as u64;
        page.object = Some(object);
        page.sha256 = Some(sha256);
        self.state.received_bytes = used;
        self.state.status = ProjectStatus::Ready;
        // The preceding mirror represents a previous snapshot. Generate a new
        // generation on run rather than claiming that old files are up to date.
        self.state.entry_path = None;
        self.state.mirror_dir = None;
        if let Err(error) = self.save() {
            self.state = previous;
            return Err(error);
        }
        Ok(())
    }

    /// 仅改变限制/过滤配置，起始网址与项目目录不能改变；可提高预算后继续。
    pub fn update_config(&mut self, mut config: GrabberConfig) -> Result<()> {
        let _lock = project_lock(&self.private())?;
        if Self::open(&self.state.config.output_dir)?.state.revision != self.state.revision {
            return Err(Error::Project("项目已被其他实例更新，请重新打开后调整配置".into()));
        }
        config.validate()?;
        if canonical(&config.start_url).map_err(Error::Config)?.as_str() != self.state.config.start_url
            || config.output_dir.canonicalize()? != self.state.config.output_dir
            || config.max_pages < self.state.pages.len()
        {
            return Err(Error::Config("已有项目不能改变起始网址/目录，页数上限不能小于现有条目".into()));
        }
        config.start_url = self.state.config.start_url.clone();
        config.output_dir = self.state.config.output_dir.clone();
        if config.render.enabled && config.render != self.state.config.render {
            for page in &mut self.state.pages {
                if page.source == PageSource::BrowserRendered
                    && (config.render.every_html_page || page.url == config.start_url)
                {
                    page.state = PageState::Pending;
                }
            }
        }
        self.state.config = config;
        self.save()
    }

    /// 下载失败可在下次 run 重试；完成对象校验后复用。取消不删除已完成对象。
    pub async fn run(&mut self, cancel: CancellationToken) -> Result<ProjectSnapshot> {
        let _lock = project_lock(&self.private())?;
        // 取得独占锁后重新载入，防止旧实例在另一实例完成后重置预算/重复抓取。
        self.state = Self::open(&self.state.config.output_dir)?.state;
        let config = self.state.config.clone();
        let start = config.validate()?;
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|error| Error::Config(error.to_string()))?;
        let stop = cancel.child_token();
        let context = Arc::new(Context {
            config,
            start,
            client,
            stop: stop.clone(),
            used: AtomicU64::new(self.state.received_bytes),
            rendered_used: AtomicU64::new(self.state.rendered_network_bytes),
            limited: AtomicBool::new(false),
            robots: Mutex::new(HashMap::new()),
        });
        for page in &mut self.state.pages {
            if page.source == PageSource::Network
                && context.config.render.enabled
                && is_html(page.content_type.as_deref().unwrap_or(""))
                && (context.config.render.every_html_page || page.url == context.config.start_url)
            {
                page.state = PageState::Pending;
            }
            if matches!(page.state, PageState::Failed(_) | PageState::Skipped(_)) {
                page.state = PageState::Pending;
            }
        }
        self.state.skipped_links = 0;
        self.state.page_limit_reached = false;
        // 从已完成 HTML/CSS 重建链接，以便提高预算后继续，无需重复下载。
        let completed = self
            .state
            .pages
            .iter()
            .filter(|page| page.state == PageState::Downloaded && is_text_resource(page))
            .cloned()
            .collect::<Vec<_>>();
        for page in completed {
            if cancel.is_cancelled() {
                break;
            }
            let bytes = verified_object(&self.private(), &page)?;
            let kind = text_kind(&page).expect("filtered text resource");
            let html = kind == TextKind::Html;
            let (text, _) = text_encoding::decode(&bytes, page.encoding.as_deref(), html).map_err(Error::Project)?;
            self.discover(
                &text,
                &canonical(page.final_url.as_deref().unwrap_or(&page.url)).map_err(Error::Project)?,
                page.depth,
                kind,
                &context,
            )?;
        }
        self.state.status = ProjectStatus::Running;
        self.save()?;
        loop {
            if cancel.is_cancelled() || context.limited.load(Ordering::Acquire) {
                break;
            }
            let Some(depth) =
                self.state.pages.iter().filter(|page| page.state == PageState::Pending).map(|page| page.depth).min()
            else {
                break;
            };
            let indices = self
                .state
                .pages
                .iter()
                .enumerate()
                .filter(|(_, page)| page.state == PageState::Pending && page.depth == depth)
                .map(|(index, _)| index)
                .take(context.config.concurrency)
                .collect::<Vec<_>>();
            let mut jobs = tokio::task::JoinSet::new();
            for index in indices {
                let ctx = context.clone();
                let url = self.state.pages[index].url.clone();
                jobs.spawn(async move { (index, fetch(&ctx, &url).await) });
            }
            while let Some(result) = jobs.join_next().await {
                let (index, result) = result.map_err(|error| Error::Project(format!("抓取任务中断：{error}")))?;
                match result {
                    Ok(fetched) => self.accept(index, fetched, &context)?,
                    Err(FetchError::Cancelled) => {}
                    Err(FetchError::Denied(error)) => self.state.pages[index].state = PageState::Skipped(error),
                    Err(FetchError::Failed(error)) => self.state.pages[index].state = PageState::Failed(error),
                }
                self.state.received_bytes = context.used.load(Ordering::Acquire);
                self.state.rendered_network_bytes = context.rendered_used.load(Ordering::Acquire);
                self.save()?;
            }
        }
        self.state.received_bytes = context.used.load(Ordering::Acquire);
        self.state.rendered_network_bytes = context.rendered_used.load(Ordering::Acquire);
        self.state.status = if cancel.is_cancelled() {
            ProjectStatus::Cancelled
        } else if context.limited.load(Ordering::Acquire) || self.state.page_limit_reached {
            ProjectStatus::Limited
        } else {
            ProjectStatus::Complete
        };
        self.render_mirror()?;
        self.save()?;
        Ok(self.snapshot())
    }

    fn accept(&mut self, index: usize, fetched: Fetched, context: &Context) -> Result<()> {
        let url_hash = hash(self.state.pages[index].url.as_bytes());
        let sha256 = hash(&fetched.body);
        let object = format!("{url_hash}-{sha256}.raw");
        let path = self.private().join("objects").join(&object);
        write_unique_or_same(&path, &fetched.body)?;
        let page = &mut self.state.pages[index];
        page.state = PageState::Downloaded;
        page.source = fetched.source;
        page.final_url = Some(fetched.url.to_string());
        page.content_type = Some(fetched.content_type.clone());
        page.bytes = fetched.body.len() as u64;
        page.object = Some(object);
        page.sha256 = Some(sha256);
        let depth = page.depth;
        if is_text_resource(page) {
            let kind = text_kind(page).expect("filtered text resource");
            let html = kind == TextKind::Html;
            let (text, encoding) = match text_encoding::decode(&fetched.body, fetched.charset.as_deref(), html) {
                Ok(decoded) => decoded,
                Err(error) => {
                    page.state = PageState::Failed(error);
                    return Ok(());
                }
            };
            page.encoding = Some(encoding);
            if let Err(error) = self.discover(&text, &fetched.url, depth, kind, context) {
                self.state.pages[index].state = PageState::Failed(error.to_string());
            }
        }
        Ok(())
    }

    fn discover(&mut self, text: &str, page_url: &Url, depth: u32, kind: TextKind, context: &Context) -> Result<()> {
        let links = match kind {
            TextKind::Html | TextKind::Svg => {
                html::links(text, page_url, &context.config.capture).map_err(Error::Project)?
            }
            TextKind::Css => css::links(text, page_url),
            TextKind::Manifest => manifest::links(text, page_url).map_err(Error::Project)?,
        };
        let mut seen = self
            .state
            .pages
            .iter()
            .enumerate()
            .map(|(index, page)| (page.url.clone(), index))
            .collect::<HashMap<_, _>>();
        for link in links {
            let depth = depth.saturating_add(u32::from(link.page));
            let Ok(url) = canonical(link.url.as_str()) else {
                continue;
            };
            if !allowed(context, &url)
                || depth > context.config.max_depth
                || !resource_allowed(&context.config, &url, link.page)
            {
                self.state.skipped_links += 1;
                continue;
            }
            if let Some(index) = seen.get(url.as_str()) {
                self.state.pages[*index].depth = self.state.pages[*index].depth.min(depth);
                self.state.pages[*index].manifest |= link.manifest;
                continue;
            }
            if self.state.pages.len() >= context.config.max_pages {
                self.state.skipped_links += 1;
                self.state.page_limit_reached = true;
                continue;
            }
            seen.insert(url.to_string(), self.state.pages.len());
            let mut record = pending(url, depth);
            record.manifest = link.manifest;
            self.state.pages.push(record);
        }
        Ok(())
    }

    fn private(&self) -> PathBuf {
        self.state.config.output_dir.join(PRIVATE)
    }
    fn save(&mut self) -> Result<()> {
        ensure_real_dir(&self.private())?;
        loop {
            self.state.revision += 1;
            let path = self.private().join(format!("state-{:020}.json", self.state.revision));
            let bytes = serde_json::to_vec(&self.state)?;
            if bytes.len() > 128 * 1024 * 1024 {
                return Err(Error::Project("项目元数据超过 128 MiB 上限".into()));
            }
            match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        self.updates.send_replace(self.state.clone());
        Ok(())
    }

    fn render_mirror(&mut self) -> Result<()> {
        let completed = self.state.pages.iter().filter(|page| page.state == PageState::Downloaded).collect::<Vec<_>>();
        if completed.is_empty() {
            return Ok(());
        }
        let mut generation = self.state.revision;
        let mirror = loop {
            let path = self.state.config.output_dir.join(format!("mirror-{generation:020}"));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => generation += 1,
                Err(error) => return Err(error.into()),
            }
        };
        let mut mapping = HashMap::new();
        for page in &completed {
            let file = mirror_name(page, &self.state.config.start_url);
            mapping.insert(page.url.clone(), file.clone());
            if let Some(final_url) = &page.final_url {
                mapping.entry(final_url.clone()).or_insert(file);
            }
        }
        for page in completed {
            let bytes = verified_object(&self.private(), page)?;
            let content = if is_text_resource(page) {
                let kind = text_kind(page).expect("filtered text resource");
                let html = kind == TextKind::Html;
                let (text, _) =
                    text_encoding::decode(&bytes, page.encoding.as_deref(), html).map_err(Error::Project)?;
                let url = canonical(page.final_url.as_deref().unwrap_or(&page.url)).map_err(Error::Project)?;
                let rewritten = match kind {
                    TextKind::Html | TextKind::Svg => {
                        html::rewrite(&text, &url, &mapping, &self.state.config.capture).map_err(Error::Project)?
                    }
                    TextKind::Css => css::rewrite(&text, &url, &mapping),
                    TextKind::Manifest => manifest::rewrite(&text, &url, &mapping).map_err(Error::Project)?,
                };
                if kind == TextKind::Manifest { rewritten.into_bytes() } else { text_encoding::utf8_mirror(rewritten) }
            } else {
                bytes
            };
            let path = mirror.join(mirror_name(page, &self.state.config.start_url));
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
            file.write_all(&content)?;
            if page.url == self.state.config.start_url {
                self.state.entry_path = Some(path);
            }
        }
        self.state.mirror_dir = Some(mirror);
        Ok(())
    }
}

fn pending(url: Url, depth: u32) -> PageRecord {
    PageRecord {
        url: url.to_string(),
        source: PageSource::Network,
        manifest: false,
        depth,
        state: PageState::Pending,
        final_url: None,
        content_type: None,
        encoding: None,
        bytes: 0,
        object: None,
        sha256: None,
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn canonical(raw: &str) -> std::result::Result<Url, String> {
    let mut url = Url::parse(raw).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 8192
    {
        return Err("只支持无用户名/密码且不超过 8192 字符的 HTTP(S) URL".into());
    }
    url.set_fragment(None);
    Ok(url)
}
fn is_html(content_type: &str) -> bool {
    matches!(content_type, "text/html" | "application/xhtml+xml")
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Html,
    Css,
    Svg,
    Manifest,
}
fn text_kind(page: &PageRecord) -> Option<TextKind> {
    if page.manifest {
        return Some(TextKind::Manifest);
    }
    let mime = page.content_type.as_deref().unwrap_or("");
    let ext = Url::parse(page.final_url.as_deref().unwrap_or(&page.url)).ok().and_then(|url| extension(&url));
    if is_html(mime) {
        return Some(TextKind::Html);
    }
    match mime {
        "text/css" => Some(TextKind::Css),
        "image/svg+xml" => Some(TextKind::Svg),
        "application/manifest+json" => Some(TextKind::Manifest),
        "application/json" if ext.as_deref() == Some("webmanifest") => Some(TextKind::Manifest),
        "application/octet-stream" if ext.as_deref() == Some("css") => Some(TextKind::Css),
        "application/octet-stream" if ext.as_deref() == Some("webmanifest") => Some(TextKind::Manifest),
        _ => None,
    }
}
fn is_text_resource(page: &PageRecord) -> bool {
    text_kind(page).is_some()
}

fn extension(url: &Url) -> Option<String> {
    url.path_segments()?
        .next_back()?
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty() && ext.len() <= 12 && ext.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}
fn resource_allowed(config: &GrabberConfig, url: &Url, page: bool) -> bool {
    page || config.resource_extensions.is_empty()
        || extension(url).is_some_and(|ext| {
            config.resource_extensions.iter().any(|allowed| allowed.trim_start_matches('.').eq_ignore_ascii_case(&ext))
        })
}
fn mirror_name(page: &PageRecord, start: &str) -> String {
    if page.url == start && is_html(page.content_type.as_deref().unwrap_or("")) {
        return "index.html".into();
    }
    let ext = if is_html(page.content_type.as_deref().unwrap_or("")) {
        "html".into()
    } else {
        Url::parse(&page.url).ok().and_then(|url| extension(&url)).unwrap_or_else(|| {
            match page.content_type.as_deref().unwrap_or("") {
                "text/css" => "css",
                "application/javascript" | "text/javascript" => "js",
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                "image/svg+xml" => "svg",
                "application/pdf" => "pdf",
                "application/manifest+json" => "webmanifest",
                "text/vtt" => "vtt",
                "font/woff" => "woff",
                "font/woff2" => "woff2",
                _ => "bin",
            }
            .into()
        })
    };
    format!("{}.{}", hash(page.url.as_bytes()), ext)
}
fn ensure_real_dir(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Project("项目目录不能是符号链接或普通文件".into()));
    }
    Ok(())
}
fn project_lock(private: &Path) -> Result<std::fs::File> {
    ensure_real_dir(private)?;
    let path = private.join("run.lock");
    if let Ok(metadata) = std::fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(Error::Project("项目锁不能是符号链接或目录".into()));
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|error| Error::Project(format!("项目已在运行或无法取得独占锁：{error}")))?;
    Ok(file)
}
fn verified_object(private: &Path, page: &PageRecord) -> Result<Vec<u8>> {
    ensure_real_dir(&private.join("objects"))?;
    let digest = page
        .sha256
        .as_ref()
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| Error::Project("对象校验和缺失".into()))?;
    let expected = format!("{}-{digest}.raw", hash(page.url.as_bytes()));
    if page.object.as_deref() != Some(&expected) {
        return Err(Error::Project("对象路径不合法".into()));
    }
    let path = private.join("objects").join(expected);
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != page.bytes
        || metadata.len() > 256 * 1024 * 1024
    {
        return Err(Error::Project("对象大小/文件类型已变化".into()));
    }
    let bytes = std::fs::read(path)?;
    if hash(&bytes) != *digest {
        return Err(Error::Project("对象内容已变化，保留原文件".into()));
    }
    Ok(bytes)
}
fn write_unique_or_same(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_real_dir(path.parent().ok_or_else(|| Error::Project("对象无父目录".into()))?)?;
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == bytes.len() as u64
                && std::fs::read(path)? == bytes
            {
                Ok(())
            } else {
                Err(Error::Project("已有对象不同，拒绝覆盖".into()))
            }
        }
        Err(error) => Err(error.into()),
    }
}

struct Context {
    config: GrabberConfig,
    start: Url,
    client: Client,
    stop: CancellationToken,
    used: AtomicU64,
    rendered_used: AtomicU64,
    limited: AtomicBool,
    robots: Mutex<HashMap<String, std::result::Result<String, String>>>,
}
fn allowed(context: &Context, url: &Url) -> bool {
    allowed_url(&context.config, &context.start, url)
}
fn allowed_url(config: &GrabberConfig, start: &Url, url: &Url) -> bool {
    !config.excluded_url_patterns.iter().any(|pattern| {
        glob_matches(pattern, if pattern.starts_with('/') { &url[url::Position::BeforePath..] } else { url.as_str() })
    }) && (url.origin() == start.origin()
        || url
            .host_str()
            .is_some_and(|host| config.allowed_domains.iter().any(|domain| domain.eq_ignore_ascii_case(host))))
}
fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v, mut star, mut saved) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            saved = v;
        } else if let Some(index) = star {
            p = index + 1;
            saved += 1;
            v = saved;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}
enum FetchError {
    Cancelled,
    Denied(String),
    Failed(String),
}
struct Fetched {
    source: PageSource,
    url: Url,
    content_type: String,
    charset: Option<String>,
    body: Vec<u8>,
}

async fn body(
    context: &Context,
    response: &mut reqwest::Response,
    max: u64,
) -> std::result::Result<Vec<u8>, FetchError> {
    let mut bytes = Vec::new();
    loop {
        let next = tokio::select! { biased; _ = context.stop.cancelled() => return Err(FetchError::Cancelled), result = response.chunk() => result };
        let chunk = next.map_err(|error| FetchError::Failed(error.to_string()))?;
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if context
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(chunk.len() as u64).filter(|total| *total <= context.config.max_total_bytes)
            })
            .is_err()
        {
            context.limited.store(true, Ordering::Release);
            context.stop.cancel();
            return Err(FetchError::Cancelled);
        }
        if bytes.len() as u64 + chunk.len() as u64 > max {
            return Err(FetchError::Failed("响应超过单文件字节上限".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
}
async fn request(context: &Context, url: &Url) -> std::result::Result<reqwest::Response, FetchError> {
    tokio::select! { biased; _ = context.stop.cancelled() => Err(FetchError::Cancelled),
    response = context.client.get(url.clone()).send() => response.map_err(|error| FetchError::Failed(error.to_string())) }
}
async fn robots_allowed(context: &Context, url: &Url) -> std::result::Result<(), FetchError> {
    if !context.config.respect_robots {
        return Ok(());
    }
    let origin = url.origin().ascii_serialization();
    let mut cache = tokio::select! { biased; _ = context.stop.cancelled() => return Err(FetchError::Cancelled), lock = context.robots.lock() => lock };
    if !cache.contains_key(&origin) {
        let robots =
            Url::parse(&format!("{origin}/robots.txt")).map_err(|error| FetchError::Failed(error.to_string()))?;
        let result = match request(context, &robots).await {
            Ok(response) if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) => Ok(String::new()),
            Ok(mut response) if response.status().is_success() => {
                match body(context, &mut response, 512 * 1024).await {
                    Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(FetchError::Cancelled) => return Err(FetchError::Cancelled),
                    Err(FetchError::Failed(error) | FetchError::Denied(error)) => Err(error),
                }
            }
            Ok(response) => Err(format!("robots.txt 返回 HTTP {}，未授权继续", response.status())),
            Err(FetchError::Cancelled) => return Err(FetchError::Cancelled),
            Err(FetchError::Failed(error) | FetchError::Denied(error)) => Err(format!("无法检查 robots.txt：{error}")),
        };
        cache.insert(origin.clone(), result);
    }
    match &cache[&origin] {
        Ok(text) => {
            if robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(text, "SwoopGrabber", url.as_str()) {
                Ok(())
            } else {
                Err(FetchError::Denied("robots.txt 禁止抓取".into()))
            }
        }
        Err(error) => Err(FetchError::Denied(error.clone())),
    }
}
async fn fetch(context: &Arc<Context>, raw: &str) -> std::result::Result<Fetched, FetchError> {
    let mut url = canonical(raw).map_err(FetchError::Failed)?;
    for _ in 0..=context.config.max_redirects {
        if !allowed(context, &url) {
            return Err(FetchError::Denied("URL 超出允许域或被排除规则过滤".into()));
        }
        robots_allowed(context, &url).await?;
        let mut response = request(context, &url).await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| FetchError::Failed("重定向缺少有效 Location".into()))?;
            url = url.join(location).map_err(|error| FetchError::Failed(error.to_string()))?;
            url = canonical(url.as_str()).map_err(FetchError::Failed)?;
            continue;
        }
        if !response.status().is_success() {
            return Err(FetchError::Failed(format!("HTTP {}", response.status())));
        }
        let raw_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream");
        let charset = text_encoding::header_charset(raw_type);
        let content_type = raw_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let bytes = body(context, &mut response, context.config.max_file_bytes).await?;
        if context.config.render.enabled
            && is_html(&content_type)
            && (context.config.render.every_html_page || raw == context.config.start_url)
        {
            let rendered = renderer::render(context, &url).await?;
            if context
                .used
                .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(rendered.len() as u64).filter(|bytes| *bytes <= context.config.max_total_bytes)
                })
                .is_err()
            {
                context.limited.store(true, Ordering::Release);
                context.stop.cancel();
                return Err(FetchError::Cancelled);
            }
            return Ok(Fetched {
                source: PageSource::BrowserRendered,
                url,
                content_type: "text/html".into(),
                charset: Some("UTF-8".into()),
                body: rendered,
            });
        }
        return Ok(Fetched { source: PageSource::Network, url, content_type, charset, body: bytes });
    }
    Err(FetchError::Failed(format!("重定向超过 {} 次", context.config.max_redirects)))
}
