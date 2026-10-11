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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Preview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
            feed_url: String::new(),
            public_key: String::new(),
            channel: Channel::Stable,
            automatic_check: false,
            interval_hours: 168,
        }
    }
}
impl UpdateConfig {
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
pub async fn check(config: &UpdateConfig, current: &str, cancel: &CancellationToken) -> Result<Option<Candidate>> {
    config.validate()?;
    if config.feed_url.is_empty() {
        return Err("请先设置更新源和签名公钥".into());
    }
    let mut response = response(&config.feed_url, cancel).await?;
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
