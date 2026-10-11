//! 进程内共享的目标名称占用；文件与媒体下载须共用同一个 registry。
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct TargetRegistry(Arc<Mutex<HashSet<PathBuf>>>);

pub struct TargetLease {
    registry: TargetRegistry,
    key: PathBuf,
}

fn target_key(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "目标文件名无效"))?;
    // 对大小写敏感卷也保守合并同名，避免默认 macOS / Windows 文件系统的别名竞争。
    Ok(std::fs::canonicalize(parent)?.join(name.to_string_lossy().to_lowercase()))
}

impl TargetRegistry {
    /// 父目录须已存在。调用方必须持有返回值，直到全部文件写入与发布结束。
    pub fn reserve(&self, path: &Path) -> io::Result<TargetLease> {
        let key = target_key(path)?;
        if !self.0.lock().unwrap().insert(key.clone()) {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "同名文件正由另一个任务使用，请重命名或稍后重试"));
        }
        Ok(TargetLease { registry: self.clone(), key })
    }

    pub fn is_reserved(&self, path: &Path) -> io::Result<bool> {
        Ok(self.0.lock().unwrap().contains(&target_key(path)?))
    }
}

impl Drop for TargetLease {
    fn drop(&mut self) {
        self.registry.0.lock().unwrap().remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_registry_excludes_aliases_until_worker_releases_lease() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TargetRegistry::default();
        let worker_registry = registry.clone();
        let path = dir.path().join("Shared.bin");
        let lease = worker_registry.reserve(&path).unwrap();
        assert!(registry.is_reserved(&path).unwrap());
        assert!(registry.reserve(&dir.path().join("./shared.bin")).is_err());
        std::thread::spawn(move || drop(lease)).join().unwrap();
        assert!(registry.reserve(&path).is_ok());
    }
}
