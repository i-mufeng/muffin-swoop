//! Versioned, strongly validated prefixes. A prefix is never a completed cache hit.
use super::{Cache, digest, regular, valid_etag};
use crate::MediaError;
use reqwest::{
    StatusCode,
    header::{self, HeaderMap},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) struct Lease {
    _file: File,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Partial {
    version: u8,
    pub etag: String,
    final_url_digest: String,
    pub start: u64,
    pub end: u64,
    pub total: u64,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_range: Option<String>,
    size: usize,
    digest: String,
    created: u64,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}
impl Partial {
    pub fn new(url: &Url, status: StatusCode, headers: &HeaderMap, limit: usize) -> Option<Self> {
        let etag = headers.get(header::ETAG)?.to_str().ok()?.to_owned();
        if !valid_etag(&etag) || !storable(headers) {
            return None;
        }
        let length = headers.get(header::CONTENT_LENGTH)?.to_str().ok()?.parse::<u64>().ok()?;
        if length == 0 || length > limit as u64 {
            return None;
        }
        let (start, end, total) = match status {
            StatusCode::OK => (0, length - 1, length),
            StatusCode::PARTIAL_CONTENT => content_range(headers.get(header::CONTENT_RANGE)?.to_str().ok()?)?,
            _ => return None,
        };
        if end.checked_sub(start)?.checked_add(1)? != length {
            return None;
        }
        Some(Self {
            version: 1,
            etag,
            final_url_digest: digest(url.as_str().as_bytes()),
            start,
            end,
            total,
            status: status.as_u16(),
            content_type: headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned),
            content_range: headers.get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).map(str::to_owned),
            size: 0,
            digest: digest(&[]),
            created: now(),
            bytes: Vec::new(),
        })
    }
    pub fn length(&self) -> usize {
        (self.end - self.start + 1) as usize
    }
    pub fn matches_request(&self, headers: &HeaderMap) -> bool {
        if self.status == 200 {
            return true;
        }
        let Some(range) =
            headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes="))
        else {
            return false;
        };
        let Some((start, end)) = range.split_once('-') else {
            return false;
        };
        let expected = if start.is_empty() {
            end.parse::<u64>().ok().filter(|n| *n > 0).map(|n| (self.total.saturating_sub(n), self.total - 1))
        } else {
            start.parse::<u64>().ok().and_then(|start| {
                let end = if end.is_empty() {
                    Some(self.total - 1)
                } else {
                    end.parse::<u64>().ok().map(|n| n.min(self.total - 1))
                }?;
                Some((start, end))
            })
        };
        expected == Some((self.start, self.end))
    }
    pub fn range(&self) -> String {
        format!("bytes={}-{}", self.start + self.bytes.len() as u64, self.end)
    }
    pub fn accepts(&self, url: &Url, status: StatusCode, headers: &HeaderMap) -> bool {
        status == StatusCode::PARTIAL_CONTENT
            && self.final_url_digest == digest(url.as_str().as_bytes())
            && headers.get(header::ETAG).is_some_and(|v| v.as_bytes() == self.etag.as_bytes())
            && storable(headers)
            && headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<usize>().ok())
                == Some(self.length() - self.bytes.len())
            && headers.get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(content_range)
                == Some((self.start + self.bytes.len() as u64, self.end, self.total))
    }
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, self.etag.parse().unwrap());
        headers.insert(header::CONTENT_LENGTH, self.length().into());
        headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        for (name, value) in [(header::CONTENT_TYPE, &self.content_type), (header::CONTENT_RANGE, &self.content_range)]
        {
            if let Some(value) = value.as_ref().and_then(|v| v.parse().ok()) {
                headers.insert(name, value);
            }
        }
        headers
    }
}
fn storable(headers: &HeaderMap) -> bool {
    !headers.get(header::CONTENT_ENCODING).is_some_and(|v| v != "identity")
        && !headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|part| part.trim().eq_ignore_ascii_case("no-store")))
        && !headers.get(header::VARY).is_some_and(|v| v == "*")
}
fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (span, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
impl Cache {
    fn open_lock(&self, name: &str) -> Result<File, MediaError> {
        std::fs::create_dir_all(&self.directory)?;
        if std::fs::symlink_metadata(&self.directory)?.file_type().is_symlink() {
            return Err(MediaError::UnsafeDestination);
        }
        let path = self.directory.join(name);
        if std::fs::symlink_metadata(&path).is_ok() && !regular(&path) {
            return Err(MediaError::UnsafeDestination);
        }
        Ok(OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?)
    }
    pub(super) fn budget_lock(&self) -> Result<Lease, MediaError> {
        let file = self.open_lock("budget.lock")?;
        file.lock()?;
        Ok(Lease { _file: file })
    }
    pub(super) fn try_lease(&self, key: &str) -> Result<Option<Lease>, MediaError> {
        let file = self.open_lock(&format!("{key}.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Lease { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }
    pub async fn lease(self: &Arc<Self>, key: &str, cancel: &CancellationToken) -> Result<Arc<Lease>, MediaError> {
        loop {
            let cache = self.clone();
            let key = key.to_owned();
            if let Some(lease) = tokio::task::spawn_blocking(move || cache.try_lease(&key))
                .await
                .map_err(|_| MediaError::GatewayFailed)??
            {
                return Ok(Arc::new(lease));
            }
            tokio::select! { biased;
                _ = cancel.cancelled() => return Err(MediaError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {},
            }
        }
    }
    pub async fn read_partial(self: &Arc<Self>, key: String, lease: Arc<Lease>) -> Option<Partial> {
        let cache = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            cache.read_partial_sync(&key, true)
        })
        .await
        .ok()
        .flatten()
    }
    fn read_partial_sync(&self, key: &str, check_age: bool) -> Option<Partial> {
        let meta = self.directory.join(format!("{key}.partial.json"));
        let data = self.directory.join(format!("{key}.part"));
        if !regular(&meta) || !regular(&data) || std::fs::metadata(&meta).ok()?.len() > 8192 {
            return None;
        }
        let mut entry: Partial = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
        if entry.version != 1
            || !valid_etag(&entry.etag)
            || !matches!(entry.status, 200 | 206)
            || entry.start > entry.end
            || entry.end >= entry.total
            || entry.end - entry.start >= self.resource_limit as u64
            || entry.size == 0
            || entry.size >= entry.length()
            || (check_age && now().saturating_sub(entry.created) > 7 * 86400)
            || std::fs::metadata(&data).ok()?.len() < entry.size as u64
        {
            return None;
        }
        File::open(data).ok()?.take(entry.size as u64).read_to_end(&mut entry.bytes).ok()?;
        if digest(&entry.bytes) != entry.digest {
            return None;
        }
        Some(entry)
    }
    pub async fn checkpoint(
        self: &Arc<Self>,
        key: String,
        mut entry: Partial,
        lease: Arc<Lease>,
    ) -> Result<(), MediaError> {
        entry.size = entry.bytes.len();
        entry.digest = digest(&entry.bytes);
        let cache = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let _writer = cache.writer.lock().unwrap();
            let _budget = cache.budget_lock()?;
            if entry.size == 0 || entry.size >= entry.length() || entry.length() > cache.resource_limit {
                return Ok(());
            }
            let data = cache.directory.join(format!("{key}.part"));
            let meta = cache.directory.join(format!("{key}.partial.json"));
            for path in [&data, &meta] {
                if std::fs::symlink_metadata(path).is_ok() && !regular(path) {
                    return Err(MediaError::UnsafeDestination);
                }
            }
            if !cache.make_room(&key, "part", entry.size as u64)? {
                return Err(MediaError::GatewayFailed);
            }
            // Data first, then metadata. A crash can leave additional suffix bytes;
            // the last committed prefix size and digest remain authoritative.
            let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&data)?;
            file.write_all(&entry.bytes)?;
            file.set_len(entry.size as u64)?;
            file.sync_data()?;
            let mut temp = tempfile::NamedTempFile::new_in(&cache.directory)?;
            temp.write_all(&serde_json::to_vec(&entry).map_err(|_| MediaError::InvalidConfig)?)?;
            temp.as_file().sync_all()?;
            temp.persist(meta).map_err(|error| MediaError::Io(error.error))?;
            Ok(())
        })
        .await
        .map_err(|_| MediaError::GatewayFailed)?
    }
    pub(super) fn make_room(&self, key: &str, extension: &str, incoming: u64) -> Result<bool, MediaError> {
        let replacing = self.directory.join(format!("{key}.{extension}"));
        let mut size = 0u64;
        let mut candidates = Vec::new();
        for (index, file) in std::fs::read_dir(&self.directory)?.enumerate() {
            if index == 16_384 {
                return Ok(false);
            }
            let file = file?;
            let name = file.file_name();
            let name = name.to_string_lossy();
            if !(((name.len() == 68 && name.ends_with(".bin")) || (name.len() == 69 && name.ends_with(".part")))
                && name[..64].bytes().all(|b| b.is_ascii_hexdigit())
                && file.file_type()?.is_file())
            {
                continue;
            }
            if file.path() == replacing {
                continue;
            }
            let meta = file.metadata()?;
            size = size.saturating_add(meta.len());
            if &name[..64] != key {
                candidates.push((
                    meta.modified().unwrap_or(UNIX_EPOCH),
                    name[..64].to_owned(),
                    name.ends_with(".part"),
                    meta.len(),
                ));
            }
        }
        candidates.sort_by_key(|entry| entry.0);
        for (_, candidate, partial, length) in candidates {
            if size.saturating_add(incoming) <= self.limit {
                break;
            }
            // Never evict an active resource; locks are non-blocking while the
            // directory budget lock is held, avoiding lock-order inversions.
            let Some(_lease) = self.try_lease(&candidate)? else {
                continue;
            };
            let owned = if partial {
                self.read_partial_sync(&candidate, false).is_some()
            } else {
                self.read_sync(&candidate, false).is_some()
            };
            if !owned {
                continue;
            }
            let (data, meta) = if partial { ("part", "partial.json") } else { ("bin", "json") };
            std::fs::remove_file(self.directory.join(format!("{candidate}.{data}")))?;
            let _ = std::fs::remove_file(self.directory.join(format!("{candidate}.{meta}")));
            size = size.saturating_sub(length);
        }
        Ok(size.saturating_add(incoming) <= self.limit)
    }
    pub async fn discard_partial(self: &Arc<Self>, key: String, lease: Arc<Lease>) {
        let cache = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let _writer = cache.writer.lock().unwrap();
            let Ok(_budget) = cache.budget_lock() else {
                return;
            };
            for suffix in ["partial.json", "part"] {
                let path = cache.directory.join(format!("{key}.{suffix}"));
                if regular(&path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prefix() -> Partial {
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, "\"v1\"".parse().unwrap());
        headers.insert(header::CONTENT_LENGTH, 8192.into());
        let mut prefix =
            Partial::new(&Url::parse("https://test.invalid/media").unwrap(), StatusCode::OK, &headers, 16384).unwrap();
        prefix.bytes = vec![7; 2048];
        prefix
    }
    #[tokio::test]
    async fn partial_budget_never_evicts_active_resources_and_reclaims_inactive_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::new(dir.path().into(), 16, 1);
        cache.limit = 3000;
        let cache = Arc::new(cache);
        let cancel = CancellationToken::new();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let first = cache.lease(&a, &cancel).await.unwrap();
        cache.checkpoint(a.clone(), prefix(), first.clone()).await.unwrap();
        let second = cache.lease(&b, &cancel).await.unwrap();
        assert!(cache.checkpoint(b.clone(), prefix(), second.clone()).await.is_err());
        assert!(cache.read_partial(a.clone(), first.clone()).await.is_some());
        assert!(cache.read(a.clone()).await.is_none(), "a prefix must never be a completed hit");
        drop(first);
        cache.checkpoint(b.clone(), prefix(), second).await.unwrap();
        assert!(!cache.directory.join(format!("{a}.part")).exists());
    }
    #[tokio::test]
    async fn locks_coordinate_cache_instances_and_cancel_waiters() {
        let dir = tempfile::tempdir().unwrap();
        let first = Arc::new(Cache::new(dir.path().into(), 16, 1));
        let second = Arc::new(Cache::new(dir.path().into(), 16, 1));
        let key = "a".repeat(64);
        let cancel = CancellationToken::new();
        let held = first.lease(&key, &CancellationToken::new()).await.unwrap();
        let waiter = second.lease(&key, &cancel);
        tokio::pin!(waiter);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(80), &mut waiter).await.is_err());
        cancel.cancel();
        assert!(matches!(waiter.await, Err(MediaError::Cancelled)));
        drop(held);
        second.lease(&key, &CancellationToken::new()).await.unwrap();
    }
    #[tokio::test]
    async fn checkpoint_write_failure_preserves_previous_verified_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path().into(), 16, 1));
        let key = "a".repeat(64);
        let lease = cache.lease(&key, &CancellationToken::new()).await.unwrap();
        cache.checkpoint(key.clone(), prefix(), lease.clone()).await.unwrap();
        let mut next = prefix();
        next.bytes.extend_from_slice(&[8; 512]);
        // An ordinary directory at the metadata destination reproduces a write
        // failure portably, including when the tests run as an administrator.
        let meta = cache.directory.join(format!("{key}.partial.json"));
        let backup = std::fs::read(&meta).unwrap();
        std::fs::remove_file(&meta).unwrap();
        std::fs::create_dir(&meta).unwrap();
        assert!(cache.checkpoint(key.clone(), next, lease.clone()).await.is_err());
        std::fs::remove_dir(&meta).unwrap();
        std::fs::write(&meta, backup).unwrap();
        assert_eq!(cache.read_partial(key.clone(), lease).await.unwrap().bytes, prefix().bytes);
        assert!(cache.read(key).await.is_none());
    }
    #[test]
    fn cache_keys_partition_authorization_range_and_proxy_scope_without_persisting_secrets() {
        let url = Url::parse("https://test.invalid/media").unwrap();
        let mut headers = HeaderMap::new();
        let a = Cache::key_scoped(&url, &headers, "proxy-a:session-a");
        let b = Cache::key_scoped(&url, &headers, "proxy-a:session-b");
        let c = Cache::key_scoped(&url, &headers, "proxy-b:session-a");
        headers.insert(header::RANGE, "bytes=100-199".parse().unwrap());
        let d = Cache::key_scoped(&url, &headers, "proxy-a:session-a");
        let keys = std::collections::HashSet::from([a, b, c, d]);
        assert_eq!(keys.len(), 4);
        assert!(keys.iter().all(|key| key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())));
    }
}
