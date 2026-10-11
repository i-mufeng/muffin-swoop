//! Optional completed-resource recovery. Every hit is revalidated by the origin
//! and verified against a local SHA-256; partial bytes are never considered a hit.
use crate::MediaError;
#[path = "partial.rs"]
mod partial;
pub(crate) use partial::{Lease, Partial};
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, ETAG, HeaderMap};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub(crate) struct Cache {
    directory: PathBuf,
    limit: u64,
    pub resource_limit: usize,
    writer: Arc<Mutex<()>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    version: u8,
    pub etag: String,
    final_url_digest: String,
    digest: String,
    size: usize,
    created: u64,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_range: Option<String>,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}
impl Entry {
    pub fn matches_url(&self, url: &url::Url) -> bool {
        self.final_url_digest == digest(url.as_str().as_bytes())
    }
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}
impl Cache {
    pub fn new(directory: PathBuf, limit_mib: usize, resource_mib: usize) -> Self {
        Self {
            directory: directory.join("swoop-media-cache-v1"),
            limit: (limit_mib as u64) * 1024 * 1024,
            resource_limit: resource_mib * 1024 * 1024,
            writer: Arc::new(Mutex::new(())),
        }
    }
    pub fn key_scoped(url: &url::Url, headers: &HeaderMap, scope: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(url.as_str().as_bytes());
        hash.update([0]);
        hash.update(scope.as_bytes());
        let mut names = headers.keys().map(|name| name.as_str()).collect::<Vec<_>>();
        names.sort_unstable();
        for name in names {
            hash.update([0]);
            hash.update(name.as_bytes());
            hash.update([0]);
            hash.update(headers[name].as_bytes());
        }
        format!("{hash:x}", hash = hash.finalize())
    }
    pub async fn read(self: &Arc<Self>, key: String) -> Option<Entry> {
        let cache = self.clone();
        tokio::task::spawn_blocking(move || cache.read_sync(&key, true)).await.ok().flatten()
    }
    fn read_sync(&self, key: &str, check_age: bool) -> Option<Entry> {
        let meta = self.directory.join(format!("{key}.json"));
        let data = self.directory.join(format!("{key}.bin"));
        if !regular(&meta) || !regular(&data) || std::fs::metadata(&meta).ok()?.len() > 8192 {
            return None;
        }
        let mut entry: Entry = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
        if entry.version != 1
            || entry.size > self.resource_limit
            || !valid_etag(&entry.etag)
            || !matches!(entry.status, 200 | 206)
            || std::fs::metadata(&data).ok()?.len() != entry.size as u64
        {
            return None;
        }
        if check_age
            && SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs().saturating_sub(entry.created) > 7 * 86400
        {
            return None;
        }
        entry.bytes = std::fs::read(data).ok()?;
        if digest(&entry.bytes) != entry.digest {
            return None;
        }
        Some(entry)
    }
    #[cfg(test)]
    pub async fn put(
        self: &Arc<Self>,
        key: String,
        final_url: &url::Url,
        status: u16,
        headers: &HeaderMap,
        bytes: Vec<u8>,
    ) {
        self.put_leased(key, final_url, status, headers, bytes, None).await;
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn put_leased(
        self: &Arc<Self>,
        key: String,
        final_url: &url::Url,
        status: u16,
        headers: &HeaderMap,
        bytes: Vec<u8>,
        lease: Option<Arc<Lease>>,
    ) {
        let Some(etag) = headers
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_etag(value))
            .map(str::to_owned)
        else {
            return;
        };
        let entry = Entry {
            version: 1,
            final_url_digest: digest(final_url.as_str().as_bytes()),
            etag,
            digest: digest(&bytes),
            size: bytes.len(),
            created: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            status,
            content_type: headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned),
            content_range: headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()).map(str::to_owned),
            bytes,
        };
        let cache = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            cache.put_sync(&key, entry)
        })
        .await;
    }
    fn put_sync(&self, key: &str, entry: Entry) -> Result<(), MediaError> {
        use std::io::Write;
        let _writer = self.writer.lock().unwrap();
        if entry.size > self.resource_limit {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory)?;
        if std::fs::symlink_metadata(&self.directory)?.file_type().is_symlink() {
            return Err(MediaError::UnsafeDestination);
        }
        let _budget = self.budget_lock()?;
        if !self.make_room(key, "bin", entry.size as u64)? {
            return Ok(());
        }
        for (extension, bytes) in
            [("bin", entry.bytes.clone()), ("json", serde_json::to_vec(&entry).map_err(|_| MediaError::InvalidConfig)?)]
        {
            let path = self.directory.join(format!("{key}.{extension}"));
            if path.exists() && !regular(&path) {
                return Err(MediaError::UnsafeDestination);
            }
            let mut temp = tempfile::NamedTempFile::new_in(&self.directory)?;
            temp.write_all(&bytes)?;
            temp.as_file().sync_all()?;
            temp.persist(path).map_err(|e| MediaError::Io(e.error))?;
        }
        Ok(())
    }
}
pub(crate) fn valid_etag(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 1024
        && value.starts_with('"')
        && value.ends_with('"')
        && !value[1..value.len() - 1].contains(['"', '\r', '\n', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn completed_cache_survives_restart_and_rejects_corruption_weak_etags_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path().into(), 16, 1));
        let key = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.insert(ETAG, "\"version-1\"".parse().unwrap());
        cache
            .put(
                key.clone(),
                &url::Url::parse("https://example.org/a").unwrap(),
                200,
                &headers,
                b"complete data".to_vec(),
            )
            .await;
        let restored = Arc::new(Cache::new(dir.path().into(), 16, 1));
        let entry = restored.read(key.clone()).await.unwrap();
        assert_eq!(entry.bytes, b"complete data");
        assert!(entry.matches_url(&url::Url::parse("https://example.org/a").unwrap()));
        assert!(!entry.matches_url(&url::Url::parse("https://example.org/changed-redirect").unwrap()));
        let metadata = std::fs::read_to_string(cache.directory.join(format!("{key}.json"))).unwrap();
        assert!(!metadata.contains("example.org"), "cache metadata must not persist source URLs");
        std::fs::write(cache.directory.join(format!("{key}.bin")), b"changed data!").unwrap();
        assert!(restored.read(key.clone()).await.is_none());
        headers.insert(ETAG, "W/\"version-2\"".parse().unwrap());
        cache
            .put("b".repeat(64), &url::Url::parse("https://example.org/a").unwrap(), 200, &headers, b"weak".to_vec())
            .await;
        assert!(cache.read("b".repeat(64)).await.is_none());
        assert!(!valid_etag("unquoted"));
        #[cfg(unix)]
        {
            let target = dir.path().join("outside.bin");
            std::fs::write(&target, b"complete data").unwrap();
            let path = cache.directory.join(format!("{key}.bin"));
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(cache.read(key.clone()).await.is_none());
            headers.insert(ETAG, "\"version-3\"".parse().unwrap());
            cache
                .put(key, &url::Url::parse("https://example.org/a").unwrap(), 200, &headers, b"replacement".to_vec())
                .await;
            assert_eq!(std::fs::read(target).unwrap(), b"complete data");
            assert!(std::fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        }
    }
}
