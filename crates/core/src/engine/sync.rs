use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use reqwest::header;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::probe::{ProbeInfo, content_range_total};
use crate::segment::{SegmentMap, SegmentState};
use crate::sync::{SyncBaseline, strong_etag};
use crate::util::{
    MAX_NUMBERED, numbered_name, open_resume_file, part_path, rename_noclobber, reserve_file, sanitize_file_name,
};
use crate::{Error, FileConflictPolicy, Result, TaskStatus};

use super::{Inner, RunNetwork, RunTarget, TaskShared, from_unix, probe_task, record_of, run_task, unix_secs};

struct Fingerprint {
    canonical_path: PathBuf,
    sha256: String,
    size: u64,
    metadata: std::fs::Metadata,
}

/// 不跟随目标符号链接；父目录重定向后规范路径也必须仍匹配。
fn regular_path(path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(Error::UnsafeSync("目标不是普通文件或已变成符号链接".into()));
    }
    Ok(std::fs::canonicalize(path)?)
}

fn fingerprint_sync(path: &Path, cancel: &CancellationToken) -> Result<Fingerprint> {
    let canonical_path = regular_path(path)?;
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
    let after = std::fs::metadata(path)?;
    if regular_path(path)? != canonical_path || before.len() != size || !same_metadata(&before, &after)? {
        return Err(Error::UnsafeSync("校验过程中目标文件已被修改或替换".into()));
    }
    Ok(Fingerprint { canonical_path, sha256: format!("{:x}", hash.finalize()), size, metadata: after })
}

fn same_metadata(before: &std::fs::Metadata, after: &std::fs::Metadata) -> Result<bool> {
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.created().ok() != after.created().ok()
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn fingerprint(path: PathBuf, cancel: CancellationToken) -> Result<Fingerprint> {
    tokio::task::spawn_blocking(move || fingerprint_sync(&path, &cancel))
        .await
        .map_err(|e| Error::Internal(format!("文件校验线程失败: {e}")))?
}

fn matches_fingerprint(actual: &Fingerprint, baseline: &SyncBaseline) -> bool {
    actual.canonical_path == baseline.canonical_path && actual.sha256 == baseline.sha256 && actual.size == baseline.size
}

async fn verify(path: PathBuf, baseline: &SyncBaseline, cancel: CancellationToken) -> Result<std::fs::Metadata> {
    let actual = fingerprint(path, cancel).await?;
    if !matches_fingerprint(&actual, baseline) {
        return Err(Error::UnsafeSync("本地文件已修改或不再属于此任务，已保留原文件".into()));
    }
    Ok(actual.metadata)
}

fn request_fingerprint(shared: &TaskShared) -> String {
    let mut hash = Sha256::new();
    hash.update(shared.url.as_str().as_bytes());
    hash.update(serde_json::to_vec(&shared.headers).expect("string pairs serialize"));
    format!("{:x}", hash.finalize())
}

fn unchanged(shared: &TaskShared, baseline: &SyncBaseline, info: &ProbeInfo) -> bool {
    if shared.source_changed.load(Ordering::Relaxed)
        || shared.url.as_str() != baseline.url
        || request_fingerprint(shared) != baseline.request_sha256
        || info.size != Some(baseline.size)
    {
        return false;
    }
    // 如果两边都有 ETag，则它优先；不以相同时间戳掩盖已变化的 ETag。
    match (&baseline.etag, &info.etag) {
        (Some(old), Some(new)) if strong_etag(old) && strong_etag(new) => old == new,
        (Some(old), Some(new)) if old != new => false,
        _ => {
            matches!((&baseline.last_modified, &info.last_modified), (Some(old), Some(new)) if old == new)
        }
    }
}

fn restore_baseline(shared: &TaskShared, baseline: &SyncBaseline) {
    shared.total.store(baseline.size, Ordering::Relaxed);
    shared.downloaded.store(baseline.size, Ordering::Relaxed);
    *shared.etag.lock().unwrap() = baseline.etag.clone();
    *shared.completed_at.lock().unwrap() = Some(from_unix(baseline.completed_at));
    let mut map = SegmentMap::single_known(baseline.size);
    for segment in &mut map.segments {
        segment.pos = segment.end - segment.start;
        segment.state = SegmentState::Done;
    }
    *shared.segments.lock().unwrap() = map;
}

enum Destination {
    Ready(Option<File>, crate::TargetLease),
    Overwrite(crate::TargetLease),
}

pub(super) fn check_unfinished_owner(
    tasks: &std::collections::BTreeMap<crate::TaskId, super::Entry>,
    shared: &TaskShared,
    path: &Path,
) -> Result<()> {
    let canonical = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in tasks.values() {
        let other = &entry.shared;
        if other.id != shared.id
            && other.status() != TaskStatus::Completed
            && !other.segments.lock().unwrap().segments.is_empty()
            && std::fs::canonicalize(other.path()).ok().as_ref() == Some(&canonical)
        {
            return Err(Error::InvalidTaskUpdate("同名文件属于另一个未完成任务，请重命名或先移除该任务记录".into()));
        }
    }
    Ok(())
}

async fn prepare_destination(
    inner: &Inner,
    shared: &TaskShared,
    info: &ProbeInfo,
    cancel: &CancellationToken,
) -> Result<Destination> {
    if !shared.segments.lock().unwrap().segments.is_empty() {
        return Ok(Destination::Ready(None, inner.target_registry.reserve(&shared.path())?));
    }
    std::fs::create_dir_all(&shared.dir)?;
    let name = {
        let mut name = shared.file_name.lock().unwrap();
        if name.contains(['/', '\\']) {
            return Err(Error::InvalidTaskUpdate("文件名不能包含路径".into()));
        }
        *name = sanitize_file_name(if name.is_empty() { &info.file_name } else { &name });
        name.clone()
    };
    let decision = shared.conflict_resolution.lock().unwrap().take();
    let policy = decision.unwrap_or_else(|| *inner.file_conflict_policy.lock().unwrap());
    if policy == FileConflictPolicy::Rename {
        let (name, file, lease) = reserve_file(&shared.dir, &name, &inner.target_registry)?;
        *shared.file_name.lock().unwrap() = name;
        shared.part_file.store(true, Ordering::Relaxed);
        return Ok(Destination::Ready(Some(file), lease));
    }
    let path = shared.path();
    let lease = match inner.target_registry.reserve(&path) {
        Ok(lease) => lease,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            return Err(if policy == FileConflictPolicy::Skip { Error::FileSkipped } else { Error::FileConflict });
        }
        Err(error) => return Err(error.into()),
    };
    // 新下载写 `<名称>.swoop`，完成后改名；目标本身已存在才进入同名处理。
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let part = part_path(&path);
            // 上次运行在保存区段前就退出时留下的自己的临时文件，直接重用。
            if shared.part_file.load(Ordering::Relaxed)
                && let Ok(file) = open_resume_file(&part)
            {
                return Ok(Destination::Ready(Some(file), lease));
            }
            match OpenOptions::new().create_new(true).read(true).write(true).open(&part) {
                Ok(file) => {
                    shared.part_file.store(true, Ordering::Relaxed);
                    return Ok(Destination::Ready(Some(file), lease));
                }
                // 只是另一个未完成任务的临时文件，不算同名文件：顺延编号。
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let (name, file, lease) = reserve_file(&shared.dir, &name, &inner.target_registry)?;
                    *shared.file_name.lock().unwrap() = name;
                    shared.part_file.store(true, Ordering::Relaxed);
                    return Ok(Destination::Ready(Some(file), lease));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    let current = std::fs::symlink_metadata(&path)?;
    if policy == FileConflictPolicy::Skip {
        return Err(Error::FileSkipped);
    }
    let changed_since_question = decision == Some(FileConflictPolicy::Overwrite)
        && shared
            .conflict_target
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|previous| !same_metadata(previous, &current).unwrap_or(false));
    if policy == FileConflictPolicy::Ask || changed_since_question {
        *shared.conflict_target.lock().unwrap() = Some(current);
        return Err(Error::FileConflict);
    }
    if let Err(error) = check_unfinished_owner(&inner.tasks.lock().unwrap(), shared, &path) {
        return Err(match error {
            Error::InvalidTaskUpdate(_) => Error::FileConflict,
            error => error,
        });
    }
    let actual = fingerprint(path, cancel.clone()).await?;
    if !same_metadata(&current, &actual.metadata)? {
        *shared.conflict_target.lock().unwrap() = Some(actual.metadata);
        return Err(Error::FileConflict);
    }
    let state = crate::sync::SyncState {
        baseline: Some(SyncBaseline {
            canonical_path: actual.canonical_path,
            sha256: actual.sha256,
            size: actual.size,
            // 原文件是用户授权覆盖的文件，不假定它对应当前远端或验证器。
            url: String::new(),
            request_sha256: String::new(),
            etag: None,
            last_modified: None,
            completed_at: unix_secs(SystemTime::now()),
        }),
        pending: true,
        overwrite: true,
        prepared: None,
        recovery_path: None,
    };
    let mut record = record_of(shared);
    record.sync_state = state.clone();
    if let Some(store) = &inner.store {
        store.save_task(&record)?;
    }
    *shared.sync_state.lock().unwrap() = state;
    Ok(Destination::Overwrite(lease))
}

/// 下载完成后把 `.swoop` 改名为目标文件名。下载期间目标被别的程序占用时顺延编号，绝不覆盖。
/// 原名的租约由调用方持有。
fn publish_part(inner: &Inner, shared: &TaskShared) -> Result<()> {
    if !shared.part_file.load(Ordering::Relaxed) {
        return Ok(());
    }
    if shared.discard.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let part = shared.write_path();
    let name = shared.file_name.lock().unwrap().clone();
    for n in 0..MAX_NUMBERED {
        let candidate = numbered_name(&name, n);
        let target = shared.dir.join(&candidate);
        let _lease = if n == 0 {
            None
        } else {
            match inner.target_registry.reserve(&target) {
                Ok(lease) => Some(lease),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error.into()),
            }
        };
        match rename_noclobber(&part, &target) {
            Ok(()) => {
                *shared.file_name.lock().unwrap() = candidate;
                shared.part_file.store(false, Ordering::Relaxed);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "同名文件过多，请更换下载文件名").into())
}

pub(super) async fn run(
    inner: &Arc<Inner>,
    shared: &Arc<TaskShared>,
    cancel: CancellationToken,
    connections: usize,
    generation: u64,
    network: Arc<RunNetwork>,
) -> Result<bool> {
    if !shared.set_run_status(generation, TaskStatus::Probing) || cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let _ = inner.events.send(super::Event::Changed(shared.id));
    let mut state = shared.sync_state.lock().unwrap().clone();
    let _target_lease;
    if !state.pending {
        let info = probe_task(&network, shared, &cancel).await?;
        match prepare_destination(inner, shared, &info, &cancel).await? {
            Destination::Overwrite(lease) => {
                _target_lease = lease;
                state = shared.sync_state.lock().unwrap().clone();
            }
            Destination::Ready(file, _lease) => {
                let mut validator_info = info.clone();
                if shared.segments.lock().unwrap().total_downloaded() > 0 {
                    // 旧片段不保证记录了每次响应的验证器，首次同步保守地完整下载。
                    validator_info.etag = None;
                    validator_info.last_modified = None;
                }
                *shared.sync_probe.lock().unwrap() = Some(validator_info);
                let downloaded = run_task(
                    inner,
                    shared,
                    cancel.clone(),
                    connections,
                    generation,
                    network,
                    RunTarget { file, info: info.clone(), staging: false },
                )
                .await;
                let verified_info = shared.sync_probe.lock().unwrap().take().unwrap_or(info);
                downloaded?;
                publish_part(inner, shared)?;
                let actual = fingerprint(shared.path(), cancel.clone()).await?;
                let baseline = SyncBaseline {
                    canonical_path: actual.canonical_path,
                    sha256: actual.sha256,
                    size: actual.size,
                    url: shared.url.to_string(),
                    request_sha256: request_fingerprint(shared),
                    etag: verified_info.etag,
                    last_modified: verified_info.last_modified,
                    completed_at: unix_secs(SystemTime::now()),
                };
                shared.sync_state.lock().unwrap().baseline = Some(baseline);
                return Ok(true);
            }
        }
    } else {
        _target_lease = inner.target_registry.reserve(&shared.path())?;
    }
    let baseline =
        state.baseline.ok_or_else(|| Error::UnsafeSync("此历史任务没有完成文件指纹，请重新下载后再启用同步".into()))?;
    if let Some(recovery) = state.recovery_path {
        let path = shared.path();
        let old = baseline.clone();
        let prepared = state.prepared.clone();
        let token = cancel.clone();
        tokio::task::spawn_blocking(move || recover_publication(&path, &recovery, &old, prepared.as_ref(), &token))
            .await
            .map_err(|error| Error::Internal(format!("文件恢复线程失败: {error}")))??;
        shared.sync_state.lock().unwrap().recovery_path = None;
    }
    let baseline = if let Some(prepared) = state.prepared {
        // 发布前/后任意时刻崩溃都可恢复；不对未知的第三份内容执行回滚覆盖。
        let actual = fingerprint(shared.path(), cancel.clone()).await?;
        let selected = if matches_fingerprint(&actual, &prepared) {
            prepared
        } else if matches_fingerprint(&actual, &baseline) {
            baseline
        } else {
            return Err(Error::UnsafeSync("同步发布后本地文件已被修改，已保留用户文件".into()));
        };
        {
            let mut state = shared.sync_state.lock().unwrap();
            state.baseline = Some(selected.clone());
            state.prepared = None;
        }
        restore_baseline(shared, &selected);
        selected
    } else {
        baseline
    };
    // 所有失败路径只改变任务状态，目标内容一直保留；重试从新的临时文件开始。
    let result = run_sync(inner, shared, &baseline, &cancel, generation, network).await;
    *shared.sync_probe.lock().unwrap() = None;
    if result.is_err() && !state.overwrite {
        let current = shared.sync_state.lock().unwrap().baseline.clone().unwrap_or(baseline);
        restore_baseline(shared, &current);
    }
    result.map(|changed| changed || state.overwrite)
}

async fn run_sync(
    inner: &Arc<Inner>,
    shared: &Arc<TaskShared>,
    baseline: &SyncBaseline,
    cancel: &CancellationToken,
    generation: u64,
    network: Arc<RunNetwork>,
) -> Result<bool> {
    verify(shared.path(), baseline, cancel.clone()).await?;
    let mut info = probe_task(&network, shared, cancel).await?;
    if unchanged(shared, baseline, &info) {
        // 网络探测期间本地文件仍可能被编辑或删除，远端未变不代表本地仍有效。
        verify(shared.path(), baseline, cancel.clone()).await?;
        return Ok(false);
    }
    // 同步采用一个完整 GET，避免弱/缺失验证器时把不同版本拼在一起。
    // 普通下载保持多连接；同步仍使用同一队列槽位、任务限速、全局限速和额度账本。
    info.resumable = false;
    let parent =
        std::fs::canonicalize(shared.path().parent().ok_or_else(|| Error::UnsafeSync("目标目录无效".into()))?)?;
    let temp = tempfile::Builder::new().prefix(".swoop-sync-").tempfile_in(&parent)?;
    *shared.segments.lock().unwrap() = SegmentMap::default();
    *shared.sync_probe.lock().unwrap() = Some(info.clone());
    run_task(
        inner,
        shared,
        cancel.clone(),
        1,
        generation,
        network,
        RunTarget { file: Some(temp.reopen()?), info: info.clone(), staging: true },
    )
    .await?;
    // 完整暂存文件再次读回，校验长度并为下一轮建立指纹。
    temp.as_file().set_permissions(std::fs::metadata(shared.path())?.permissions())?;
    let actual = fingerprint(temp.path().to_owned(), cancel.clone()).await?;
    if info.size.is_some_and(|size| size != actual.size) {
        return Err(Error::UnsafeSync("暂存文件大小与远端不一致".into()));
    }
    let path = shared.path();
    let mut new_baseline = SyncBaseline {
        canonical_path: baseline.canonical_path.clone(),
        sha256: actual.sha256.clone(),
        size: actual.size,
        url: shared.url.to_string(),
        request_sha256: request_fingerprint(shared),
        etag: info.etag,
        last_modified: info.last_modified,
        completed_at: unix_secs(SystemTime::now()),
    };
    if new_baseline.sha256 == baseline.sha256 && new_baseline.size == baseline.size {
        // 缺少可信远端验证器时只能下载比对；相同字节仍不替换文件，也不重复完成动作。
        verify(shared.path(), baseline, cancel.clone()).await?;
        new_baseline.completed_at = baseline.completed_at;
        let mut record = record_of(shared);
        record.sync_state.baseline = Some(new_baseline.clone());
        record.sync_state.prepared = None;
        if let Some(store) = &inner.store {
            store.save_task(&record)?;
        }
        let mut state = shared.sync_state.lock().unwrap();
        state.baseline = Some(new_baseline);
        state.prepared = None;
        return Ok(false);
    }
    let verified_metadata = verify(shared.path(), baseline, cancel.clone()).await?;
    let mut prepared_record = record_of(shared);
    prepared_record.sync_state.prepared = Some(new_baseline.clone());
    let recovery = tempfile::Builder::new().prefix(".swoop-recovery-").tempfile_in(&parent)?.into_temp_path();
    std::fs::write(&recovery, recovery_marker(&recovery))?;
    prepared_record.sync_state.recovery_path = Some(recovery.to_path_buf());
    if let Some(store) = &inner.store {
        store.save_task(&prepared_record)?;
    }
    // keep 在移动用户文件前调用；之后任何错误或线程退出都不能由 TempPath::drop 删除原件。
    let recovery = recovery.keep().map_err(|error| Error::Io(error.error))?;
    {
        let mut sync = shared.sync_state.lock().unwrap();
        sync.prepared = Some(new_baseline.clone());
        sync.recovery_path = Some(recovery.clone());
    }
    let worker_shared = shared.clone();
    let worker_baseline = baseline.clone();
    let token = cancel.clone();
    tokio::task::spawn_blocking(move || {
        publish_checked(
            temp.into_temp_path(),
            &path,
            &recovery,
            &worker_baseline,
            &verified_metadata,
            &actual,
            &token,
            || {
                let status = worker_shared.status.lock().unwrap();
                if status.generation != generation || status.status == TaskStatus::Paused || token.is_cancelled() {
                    Err(Error::Cancelled)
                } else {
                    Ok(status)
                }
            },
        )
    })
    .await
    .map_err(|error| Error::Internal(format!("文件发布线程失败: {error}")))??;
    let mut sync = shared.sync_state.lock().unwrap();
    sync.baseline = Some(new_baseline);
    sync.prepared = None;
    sync.recovery_path = None;
    // 完成状态发布前仍保留 pending，快速暂停/恢复必须再次安全同步。
    sync.pending = true;
    // 至此文件发布成功。之后即使最终状态写库暂时失败，先前落盘的 prepared 仍能
    // 在重启时辨认这份文件；不把已成功的发布误报失败，也不回滚覆盖用户的新改动。
    Ok(true)
}

fn same_moved_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> Result<bool> {
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.created().ok() != after.created().ok()
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Ok(false);
        }
    }
    Ok(after.is_file() && !after.file_type().is_symlink())
}

fn restore_recovery(path: &Path, recovery: &Path) -> Result<()> {
    let mut saved = tempfile::TempPath::try_from_path(recovery.to_owned())?;
    saved.disable_cleanup(true);
    saved
        .persist_noclobber(path)
        .map_err(|_| Error::UnsafeSync(format!("目标已被其它文件占用，原文件保留于 {}", recovery.display())))
}

fn recovery_marker(path: &Path) -> Vec<u8> {
    format!("Swoop recovery reservation\n{}\n", path.display()).into_bytes()
}

#[allow(clippy::too_many_arguments)]
fn publish_checked<G>(
    temporary: tempfile::TempPath,
    path: &Path,
    recovery: &Path,
    baseline: &SyncBaseline,
    verified: &std::fs::Metadata,
    replacement: &Fingerprint,
    cancel: &CancellationToken,
    commit_guard: impl FnOnce() -> Result<G>,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if regular_path(path)? != baseline.canonical_path {
        return Err(Error::UnsafeSync("发布前目标路径改变".into()));
    }
    // 原子捕获此刻真正位于目标路径的文件；不能在校验后无条件覆盖一个可能刚保存的新文件。
    std::fs::rename(path, recovery)?;
    let publish = (|| {
        let moved = fingerprint_sync(recovery, cancel)?;
        if moved.sha256 != baseline.sha256
            || moved.size != baseline.size
            || !same_moved_file(verified, &moved.metadata)?
        {
            return Err(Error::UnsafeSync("发布前原文件已被修改，已停止替换".into()));
        }
        if regular_path(&temporary)? != replacement.canonical_path
            || !same_metadata(&replacement.metadata, &std::fs::symlink_metadata(&temporary)?)?
        {
            return Err(Error::UnsafeSync("暂存文件在校验后被修改或替换".into()));
        }
        let _guard = commit_guard()?;
        // 此时新出现的任何外部文件都获胜，不以回滚或发布覆盖它。
        temporary.persist_noclobber(path).map_err(|error| Error::Io(error.error))?;
        Ok(moved.metadata)
    })();
    match publish {
        Ok(saved) => {
            if !same_metadata(&saved, &std::fs::symlink_metadata(recovery)?)? {
                return Err(Error::UnsafeSync(format!("原文件在发布期间改变，恢复文件保留于 {}", recovery.display())));
            }
            std::fs::remove_file(recovery)?;
            Ok(())
        }
        Err(error) => {
            restore_recovery(path, recovery)?;
            Err(error)
        }
    }
}

fn recover_publication(
    path: &Path,
    recovery: &Path,
    baseline: &SyncBaseline,
    prepared: Option<&SyncBaseline>,
    cancel: &CancellationToken,
) -> Result<()> {
    if recovery.parent() != baseline.canonical_path.parent()
        || !recovery.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".swoop-recovery-"))
    {
        return Err(Error::UnsafeSync("恢复文件路径无效".into()));
    }
    let saved = match std::fs::symlink_metadata(recovery) {
        Ok(_) => Some(fingerprint_sync(recovery, cancel)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let saved = saved.ok_or_else(|| Error::UnsafeSync("目标和恢复文件均不存在".into()))?;
            restore_recovery(path, recovery)?;
            if saved.sha256 != baseline.sha256 || saved.size != baseline.size {
                return Err(Error::UnsafeSync("已恢复用户修改后的文件，未继续覆盖".into()));
            }
        }
        Ok(_) => {
            let current = fingerprint_sync(path, cancel)?;
            if !matches_fingerprint(&current, baseline)
                && !prepared.is_some_and(|candidate| matches_fingerprint(&current, candidate))
            {
                return Err(Error::UnsafeSync(format!("目标已被用户修改，恢复文件保留于 {}", recovery.display())));
            }
            if let Some(saved) = saved {
                let marker = recovery_marker(recovery);
                let is_reservation =
                    saved.size == marker.len() as u64 && saved.sha256 == format!("{:x}", Sha256::digest(&marker));
                if !is_reservation && (saved.sha256 != baseline.sha256 || saved.size != baseline.size) {
                    return Err(Error::UnsafeSync(format!("恢复文件包含其它内容，已保留于 {}", recovery.display())));
                }
                std::fs::remove_file(recovery)?;
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// 初次多连接下载的全部响应都要支持探测的验证器，才能把它用于后续跳过下载。
/// 缺失/变化的验证器不会妨碍普通下载，但该完成记录下次必须重新完整下载。
pub(super) fn validators_match(info: &ProbeInfo, response: &crate::network::NetworkResponse) -> bool {
    let headers = response.headers();
    let matches = match (&info.etag, &info.last_modified) {
        (Some(etag), _) => headers.get(header::ETAG).and_then(|v| v.to_str().ok()) == Some(etag.as_str()),
        (_, Some(modified)) => {
            headers.get(header::LAST_MODIFIED).and_then(|v| v.to_str().ok()) == Some(modified.as_str())
        }
        _ => true,
    };
    let size = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        content_range_total(headers)
    } else {
        response.content_length()
    };
    matches && !matches!((info.size, size), (Some(a), Some(b)) if a != b)
}

/// 同步完整 GET 必须对应探测的实体，禁止忽略 Range 的服务器截断大响应后冒充成功。
pub(super) fn validate_response(info: &ProbeInfo, response: &crate::network::NetworkResponse, start: u64, end: u64) -> Result<()> {
    let headers = response.headers();
    if let Some(etag) = &info.etag {
        if headers.get(header::ETAG).and_then(|v| v.to_str().ok()) != Some(etag.as_str()) {
            return Err(Error::UnsafeSync("下载期间远端 ETag 改变或缺失".into()));
        }
    } else if let Some(modified) = &info.last_modified
        && headers.get(header::LAST_MODIFIED).and_then(|v| v.to_str().ok()) != Some(modified.as_str())
    {
        return Err(Error::UnsafeSync("下载期间远端修改时间改变或缺失".into()));
    }
    let actual_size = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        content_range_total(headers)
    } else {
        response.content_length()
    };
    if let (Some(expected), Some(actual)) = (info.size, actual_size)
        && expected != actual
    {
        return Err(Error::UnsafeSync("下载期间远端文件大小改变".into()));
    }
    if response.status() == reqwest::StatusCode::PARTIAL_CONTENT || start != 0 || end != u64::MAX {
        return Err(Error::UnsafeSync("同步下载必须返回完整文件".into()));
    }
    Ok(())
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    fn baseline(path: &Path) -> (SyncBaseline, Fingerprint) {
        let actual = fingerprint_sync(path, &CancellationToken::new()).unwrap();
        (
            SyncBaseline {
                canonical_path: actual.canonical_path.clone(),
                sha256: actual.sha256.clone(),
                size: actual.size,
                url: String::new(),
                request_sha256: String::new(),
                etag: None,
                last_modified: None,
                completed_at: 0,
            },
            actual,
        )
    }

    fn stage(parent: &Path, bytes: &[u8]) -> tempfile::TempPath {
        let path = tempfile::NamedTempFile::new_in(parent).unwrap().into_temp_path();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn recovery(parent: &Path) -> PathBuf {
        tempfile::Builder::new()
            .prefix(".swoop-recovery-")
            .tempfile_in(parent)
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap()
    }

    #[test]
    fn changed_file_captured_after_last_check_is_restored_without_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let path = parent.join("target.bin");
        std::fs::write(&path, b"original").unwrap();
        let (old, checked) = baseline(&path);
        let temp = stage(&parent, b"downloaded");
        let actual = fingerprint_sync(&temp, &CancellationToken::new()).unwrap();
        let saved = recovery(&parent);
        let user = stage(&parent, b"user atomic save");
        std::fs::rename(&user, &path).unwrap();
        assert!(
            publish_checked(
                temp,
                &path,
                &saved,
                &old,
                &checked.metadata,
                &actual,
                &CancellationToken::new(),
                || Ok(())
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"user atomic save");
        assert!(!saved.exists());
    }

    #[test]
    fn file_created_at_commit_wins_and_recovery_file_is_never_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let path = parent.join("target.bin");
        std::fs::write(&path, b"original").unwrap();
        let (old, checked) = baseline(&path);
        let temp = stage(&parent, b"downloaded");
        let actual = fingerprint_sync(&temp, &CancellationToken::new()).unwrap();
        let saved = recovery(&parent);
        let result =
            publish_checked(temp, &path, &saved, &old, &checked.metadata, &actual, &CancellationToken::new(), || {
                std::fs::write(&path, b"late user save")?;
                Ok(())
            });
        assert!(result.unwrap_err().to_string().contains(saved.to_str().unwrap()));
        assert_eq!(std::fs::read(&path).unwrap(), b"late user save");
        assert_eq!(std::fs::read(&saved).unwrap(), b"original");
    }

    #[test]
    fn cancellation_at_commit_restores_original_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let path = parent.join("target.bin");
        std::fs::write(&path, b"original").unwrap();
        let (old, checked) = baseline(&path);
        let temp = stage(&parent, b"downloaded");
        let actual = fingerprint_sync(&temp, &CancellationToken::new()).unwrap();
        let saved = recovery(&parent);
        let result =
            publish_checked(temp, &path, &saved, &old, &checked.metadata, &actual, &CancellationToken::new(), || {
                Err::<(), _>(Error::Cancelled)
            });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(!saved.exists());
    }

    #[test]
    fn durable_recovery_restores_missing_target_and_never_clobbers_late_file() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let path = parent.join("target.bin");
        std::fs::write(&path, b"original").unwrap();
        let (old, _) = baseline(&path);
        let saved = recovery(&parent);
        std::fs::rename(&path, &saved).unwrap();
        recover_publication(&path, &saved, &old, None, &CancellationToken::new()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(!saved.exists());
        let saved = recovery(&parent);
        std::fs::rename(&path, &saved).unwrap();
        std::fs::write(&path, b"late user save").unwrap();
        assert!(recover_publication(&path, &saved, &old, None, &CancellationToken::new()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"late user save");
        assert_eq!(std::fs::read(&saved).unwrap(), b"original");
    }

    #[test]
    fn real_atomic_saves_racing_publication_never_disappear() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let path = parent.join("target.bin");
        let writer_path = path.clone();
        let (tx, rx) = mpsc::channel::<(PathBuf, u64)>();
        let (ack, received) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            while let Ok((source, delay)) = rx.recv() {
                let deadline = Instant::now() + Duration::from_micros(delay);
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
                std::fs::rename(source, &writer_path).unwrap();
                ack.send(()).unwrap();
            }
        });
        for attempt in 0..1000 {
            std::fs::write(&path, b"original").unwrap();
            let (old, checked) = baseline(&path);
            let temp = stage(&parent, b"downloaded");
            let actual = fingerprint_sync(&temp, &CancellationToken::new()).unwrap();
            let saved = recovery(&parent);
            let user = stage(&parent, b"user edit");
            tx.send((user.to_path_buf(), attempt % 100)).unwrap();
            let result = publish_checked(
                temp,
                &path,
                &saved,
                &old,
                &checked.metadata,
                &actual,
                &CancellationToken::new(),
                || Ok(()),
            );
            received.recv().unwrap();
            let target = std::fs::read(&path).unwrap();
            let recovery = std::fs::read(&saved).ok();
            assert!(
                target == b"user edit" || recovery.as_deref() == Some(b"user edit"),
                "user edit lost in attempt {attempt}: {result:?}"
            );
            if saved.exists() {
                std::fs::remove_file(&saved).unwrap();
            }
        }
        drop(tx);
        writer.join().unwrap();
    }
}
