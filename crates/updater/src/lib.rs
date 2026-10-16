//! Signed update feeds, bounded downloads and an explicit, recoverable installer.
//! The signing public key is configured out of band, never accepted from the feed.
mod install;
pub use install::{InstallPlan, apply, prepare, run_helper, run_helper_ready};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_FEED_BYTES: usize = 256 * 1024;

/// 国内主更新源（又拍云 CDN），构建时内置；用户留空即可使用，无需手动导入。
/// 注意：又拍云绑定自有域名后请在控制台开放默认域名访问，或把下方域名替换为绑定的域名。
pub const DEFAULT_FEED_URL: &str =
    "https://muffin-cdn.b0.upaiyun.com/swoop-release/feed.json";
/// 官方备用更新源（GitHub Releases）。主更新源请求失败时自动回退；
/// 两个源的清单使用同一把内置 Ed25519 公钥验签。
pub const FALLBACK_FEED_URL: &str =
    "https://github.com/i-mufeng/muffin-swoop/releases/latest/download/feed.json";
/// 官方 Ed25519 公钥，构建时内置；清单永远不能用来自报或替换它。
/// v0.7.0 起使用；v0.6.0 及更早版本内置的旧公钥见 LEGACY_PUBLIC_KEY。
pub const OFFICIAL_PUBLIC_KEY: &str =
    "20cf8eda8be4fd6654093f05c5bd5abe794b60c12762832152ffe440c08a3673";
/// v0.6.0 内置、v0.7.0 起被 OFFICIAL_PUBLIC_KEY 轮换替换的旧公钥。
/// 生产私钥已不可用；存量配置中等于该值的公钥在加载时迁移到内置新公钥。
/// 注意：0.6.0 及更早版本的内置公钥仍是旧值，无法验签 v0.7.0 清单，
/// 需手动下载 v0.7.0 安装包升级，之后即可恢复自动更新。
pub const LEGACY_PUBLIC_KEY: &str =
    "76f5ee41fcec2fdadf59972b78351add0dfe44cf33baa18e7744599f763d40c1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Preview,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct UpdateConfig {
    pub feed_url: String,
    pub public_key: String,
    pub channel: Channel,
    pub automatic_check: bool,
    pub interval_hours: u32,
}
impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            feed_url: DEFAULT_FEED_URL.into(),
            public_key: OFFICIAL_PUBLIC_KEY.into(),
            channel: Channel::Stable,
            automatic_check: false,
            interval_hours: 168,
        }
    }
}
impl UpdateConfig {
    /// 把留空的更新源/公钥和已轮换的旧公钥迁移到内置官方值；自定义值原样保留。
    /// 幂等，可在每次加载和每次检查前调用。
    pub fn migrate(&mut self) {
        if self.feed_url.is_empty() {
            self.feed_url = DEFAULT_FEED_URL.into();
        }
        if self.public_key.is_empty() || self.public_key.eq_ignore_ascii_case(LEGACY_PUBLIC_KEY) {
            self.public_key = OFFICIAL_PUBLIC_KEY.into();
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=8760).contains(&self.interval_hours) {
            return Err("更新间隔必须为 1–8760 小时".into());
        }
        if self.feed_url.is_empty() && self.public_key.is_empty() && !self.automatic_check {
            return Ok(());
        }
        validate_url(&self.feed_url)?;
        let bytes: [u8; 32] = decode_hex(&self.public_key)?.try_into().map_err(|_| "更新公钥需要 64 位十六进制字符")?;
        VerifyingKey::from_bytes(&bytes).map_err(|_| "更新公钥无效")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedFeed {
    pub payload: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub channel: Channel,
    pub notes: String,
    pub platforms: BTreeMap<String, Artifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub feed: SignedFeed,
    pub public_key: String,
    pub release: Release,
    pub artifact: Artifact,
    pub platform: String,
}

pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
pub fn verify(
    feed: &SignedFeed,
    key: &str,
    current: &str,
    channel: &Channel,
    target: &str,
) -> Result<Option<Candidate>> {
    if feed.payload.len() > MAX_FEED_BYTES {
        return Err("更新清单过大".into());
    }
    let public: [u8; 32] = decode_hex(key)?.try_into().map_err(|_| "更新公钥长度无效")?;
    let signature = Signature::try_from(decode_hex(&feed.signature)?.as_slice()).map_err(|_| "更新签名长度无效")?;
    VerifyingKey::from_bytes(&public)
        .map_err(|_| "更新公钥无效")?
        .verify_strict(feed.payload.as_bytes(), &signature)
        .map_err(|_| "更新清单签名验证失败")?;
    let release: Release = serde_json::from_str(&feed.payload).map_err(|_| "更新清单格式无效")?;
    let version = semver::Version::parse(&release.version).map_err(|_| "更新版本号无效")?;
    let current = semver::Version::parse(current).map_err(|_| "当前版本号无效")?;
    if version <= current || &release.channel != channel || (*channel == Channel::Stable && !version.pre.is_empty()) {
        return Ok(None);
    }
    let artifact = release.platforms.get(target).ok_or("此版本没有当前平台的安装包")?.clone();
    validate_url(&artifact.url)?;
    if artifact.size == 0 || artifact.size > MAX_ARCHIVE_BYTES || decode_hex(&artifact.sha256)?.len() != 32 {
        return Err("安装包大小或哈希无效".into());
    }
    Ok(Some(Candidate { feed: feed.clone(), public_key: key.into(), release, artifact, platform: target.into() }))
}

fn validate_url(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|_| "更新地址无效")?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("更新地址必须为 HTTPS；仅本机测试允许 HTTP".into());
    }
    Ok(url)
}
async fn response(value: &str, cancel: &CancellationToken) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .user_agent(concat!("Swoop-updater/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let mut url = validate_url(value)?;
    for _ in 0..=5 {
        let result = tokio::select! { biased; _ = cancel.cancelled() => return Err("更新已取消".into()), result = client.get(url.clone()).send() => result.map_err(|_| "无法连接更新服务器")? };
        if result.status().is_redirection() {
            let location = result
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("更新重定向缺少地址")?;
            let next = validate_url(url.join(location).map_err(|_| "更新重定向无效")?.as_str())?;
            if url.scheme() == "https" && next.scheme() != "https" {
                return Err("更新不允许降级为 HTTP".into());
            }
            url = next;
        } else {
            return result
                .error_for_status()
                .map_err(|e| format!("更新服务器返回 {}", e.status().map_or(0, |s| s.as_u16())));
        }
    }
    Err("更新重定向次数过多".into())
}
/// 检查更新：先请求主更新源，传输或清单失败时自动回退到备用更新源
/// （[`FALLBACK_FEED_URL`]）；两个源用同一把内置公钥验签。用户主动取消时不回退。
pub async fn check(config: &UpdateConfig, current: &str, cancel: &CancellationToken) -> Result<Option<Candidate>> {
    check_sources(config, current, cancel, &config.feed_url, Some(FALLBACK_FEED_URL)).await
}

/// 依次尝试主更新源与备用更新源；备用源与主源相同时只请求一次。
/// 暴露给集成测试与自定义镜像场景；正常使用请走 [`check`]。
pub async fn check_sources(
    config: &UpdateConfig,
    current: &str,
    cancel: &CancellationToken,
    primary: &str,
    fallback: Option<&str>,
) -> Result<Option<Candidate>> {
    let mut config = config.clone();
    config.migrate();
    config.validate()?;
    match check_feed(&config, primary, current, cancel).await {
        Ok(candidate) => Ok(candidate),
        Err(error) => {
            if cancel.is_cancelled() {
                return Err(error);
            }
            let Some(fallback) = fallback.filter(|url| *url != primary) else {
                return Err(error);
            };
            check_feed(&config, fallback, current, cancel)
                .await
                .map_err(|fallback_error| format!("{error}；备用更新源也失败：{fallback_error}"))
        }
    }
}
async fn check_feed(
    config: &UpdateConfig,
    feed_url: &str,
    current: &str,
    cancel: &CancellationToken,
) -> Result<Option<Candidate>> {
    let mut response = response(feed_url, cancel).await?;
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! { biased; _ = cancel.cancelled() => return Err("更新已取消".into()), chunk = response.chunk() => chunk.map_err(|_| "更新清单读取失败")? };
        let Some(chunk) = chunk else { break };
        if bytes.len() + chunk.len() > MAX_FEED_BYTES {
            return Err("更新清单过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let feed = serde_json::from_slice(&bytes).map_err(|_| "更新清单格式无效")?;
    verify(&feed, &config.public_key, current, &config.channel, &platform())
}
pub async fn download(candidate: &Candidate, directory: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    // Re-derive all trusted fields; a deserialized Candidate is not itself trusted.
    let trusted = verify(
        &candidate.feed,
        &candidate.public_key,
        env!("CARGO_PKG_VERSION"),
        &candidate.release.channel,
        &platform(),
    )?
    .ok_or("此更新已不适用")?;
    tokio::fs::create_dir_all(directory).await.map_err(|e| e.to_string())?;
    let path = directory.join(format!("swoop-update-{}.zip", uuid::Uuid::new_v4()));
    let mut output =
        tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await.map_err(|e| e.to_string())?;
    let result = async {
        let mut response = response(&trusted.artifact.url, cancel).await?;
        if response.content_length().is_some_and(|n| n != trusted.artifact.size) { return Err("安装包长度不匹配".into()); }
        let mut bytes = 0_u64;
        let mut hash = Sha256::new();
        loop {
            let chunk = tokio::select! { biased; _ = cancel.cancelled() => return Err("更新已取消".into()), chunk = response.chunk() => chunk.map_err(|_| "安装包传输中断")? };
            let Some(chunk) = chunk else { break };
            bytes += chunk.len() as u64;
            if bytes > trusted.artifact.size { return Err("安装包超出声明大小".into()); }
            hash.update(&chunk);
            output.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        if bytes != trusted.artifact.size || hex(&hash.finalize()) != trusted.artifact.sha256.to_ascii_lowercase() { return Err("安装包完整性验证失败".into()); }
        output.sync_all().await.map_err(|e| e.to_string())?;
        Ok(())
    }.await;
    drop(output);
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }
    Ok(path)
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err("十六进制数据无效".into());
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).map_err(|_| "十六进制数据无效".into()))
        .collect()
}
pub(crate) fn file_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex(&hash.finalize()))
}
