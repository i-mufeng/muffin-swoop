//! One-shot, fingerprint-checked publication of completed media files.
use std::{
    fs::{self, File, Metadata},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tempfile::TempPath;
use tokio_util::sync::CancellationToken;

use crate::MediaError;

struct Fingerprint {
    canonical_path: PathBuf,
    digest: [u8; 32],
    metadata: Metadata,
}

pub(crate) struct Publication {
    requested: PathBuf,
    target: PathBuf,
    parent_metadata: Metadata,
    original: Option<Fingerprint>,
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), MediaError> {
    if cancel.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn regular_path(path: &Path) -> Result<PathBuf, MediaError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(MediaError::UnsafeDestination);
    }
    Ok(fs::canonicalize(path)?)
}

fn same_identity(before: &Metadata, after: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        before
            .created()
            .ok()
            .zip(after.created().ok())
            .is_some_and(|(before, after)| before == after)
    }
}

fn same_metadata(before: &Metadata, after: &Metadata) -> Result<bool, MediaError> {
    if !same_metadata_after_move(before, after)? {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() {
            return Ok(false);
        }
    }
    Ok(true)
}

// Moving a file can change ctime. Its identity, contents and write time must
// still match the file for which overwrite was authorized.
fn same_metadata_after_move(before: &Metadata, after: &Metadata) -> Result<bool, MediaError> {
    if !after.is_file()
        || after.file_type().is_symlink()
        || before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.created().ok() != after.created().ok()
        || !same_identity(before, after)
    {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if before.file_attributes() != after.file_attributes() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn fingerprint(path: &Path, cancel: &CancellationToken) -> Result<Fingerprint, MediaError> {
    check_cancel(cancel)?;
    let canonical_path = regular_path(path)?;
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    let after = fs::symlink_metadata(path)?;
    if regular_path(path)? != canonical_path
        || before.len() != bytes
        || !same_metadata(&before, &after)?
    {
        return Err(MediaError::DestinationChanged);
    }
    Ok(Fingerprint {
        canonical_path,
        digest: hash.finalize().into(),
        metadata: after,
    })
}

fn check_fingerprint(path: &Path, expected: &Fingerprint) -> Result<(), MediaError> {
    if regular_path(path).map_err(|_| MediaError::DestinationChanged)? != expected.canonical_path
        || !same_metadata(
            &expected.metadata,
            &fs::symlink_metadata(path).map_err(|_| MediaError::DestinationChanged)?,
        )?
    {
        return Err(MediaError::DestinationChanged);
    }
    Ok(())
}

fn worker_error(_: tokio::task::JoinError) -> MediaError {
    MediaError::Io(std::io::Error::other("媒体文件校验线程失败"))
}

/// Owns a recovery *name*, never automatic deletion of the user's file.
struct RecoveryFile {
    path: PathBuf,
}

impl RecoveryFile {
    fn take(target: &Path) -> Result<Self, MediaError> {
        // Reserve a unique name and finish all fallible TempPath::keep work
        // before it can contain user data (notably Windows file attributes).
        let path = tempfile::Builder::new()
            .prefix(".swoop-media-recovery-")
            .suffix(".bak")
            .tempfile_in(target.parent().ok_or(MediaError::UnsafeDestination)?)?
            .into_temp_path()
            .keep()
            .map_err(|error| MediaError::Io(error.error))?;
        if let Err(error) = fs::rename(target, &path) {
            // The reserved file is still our empty placeholder.
            let _ = fs::remove_file(&path);
            return Err(if error.kind() == ErrorKind::NotFound {
                MediaError::DestinationChanged
            } else {
                error.into()
            });
        }
        Ok(Self { path })
    }

    fn verify(
        &self,
        expected: &Fingerprint,
        cancel: &CancellationToken,
    ) -> Result<Fingerprint, MediaError> {
        let moved = fingerprint(&self.path, cancel).map_err(|error| match error {
            MediaError::Cancelled => MediaError::Cancelled,
            _ => MediaError::DestinationChanged,
        })?;
        if moved.digest != expected.digest
            || !same_metadata_after_move(&expected.metadata, &moved.metadata)?
        {
            return Err(MediaError::DestinationChanged);
        }
        Ok(moved)
    }

    fn restore(self, target: &Path, reason: MediaError) -> MediaError {
        let Ok(mut path) = TempPath::try_from_path(&self.path) else {
            return MediaError::RecoveryRequired(self.path);
        };
        // Never let a persist failure delete the only copy of the user's file.
        // A late-arriving target belongs to its writer and is never overwritten.
        path.disable_cleanup(true);
        match path.persist_noclobber(target) {
            Ok(()) => reason,
            Err(_) => MediaError::RecoveryRequired(self.path),
        }
    }

    fn finish(self, verified: &Fingerprint, cancel: &CancellationToken) -> Result<(), MediaError> {
        // Publication is the commit point. A late cancel must not report a
        // cancelled download after the output was installed. Retain the old
        // file in that case, or if its identity/write metadata changed.
        if cancel.is_cancelled() {
            return Ok(());
        }
        if check_fingerprint(&self.path, verified).is_err() || fs::remove_file(&self.path).is_err()
        {
            return Err(MediaError::RecoveryRequired(self.path));
        }
        Ok(())
    }
}

impl Publication {
    pub async fn prepare(
        requested: PathBuf,
        overwrite: bool,
        cancel: CancellationToken,
    ) -> Result<Self, MediaError> {
        tokio::task::spawn_blocking(move || {
            check_cancel(&cancel)?;
            let requested = if requested.is_absolute() {
                requested
            } else {
                std::env::current_dir()?.join(requested)
            };
            let parent = requested.parent().ok_or(MediaError::UnsafeDestination)?;
            let canonical_parent = fs::canonicalize(parent)?;
            let parent_metadata = fs::metadata(&canonical_parent)?;
            if !parent_metadata.is_dir() {
                return Err(MediaError::UnsafeDestination);
            }
            let target =
                canonical_parent.join(requested.file_name().ok_or(MediaError::UnsafeDestination)?);
            let original = match fs::symlink_metadata(&requested) {
                Ok(_) if !overwrite => return Err(MediaError::DestinationExists),
                Ok(_) => Some(fingerprint(&requested, &cancel)?),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if original
                .as_ref()
                .is_some_and(|value| value.canonical_path != target)
            {
                return Err(MediaError::DestinationChanged);
            }
            Ok(Self {
                requested,
                target,
                parent_metadata,
                original,
            })
        })
        .await
        .map_err(worker_error)?
    }

    pub fn parent(&self) -> &Path {
        self.target.parent().expect("resolved parent")
    }

    fn check_parent(&self) -> Result<(), MediaError> {
        let requested_parent = self.requested.parent().expect("resolved parent");
        let canonical =
            fs::canonicalize(requested_parent).map_err(|_| MediaError::DestinationChanged)?;
        let metadata = fs::metadata(&canonical).map_err(|_| MediaError::DestinationChanged)?;
        if canonical != self.parent()
            || !metadata.is_dir()
            || !same_identity(&self.parent_metadata, &metadata)
        {
            return Err(MediaError::DestinationChanged);
        }
        Ok(())
    }

    pub async fn publish(
        self,
        temporary: TempPath,
        cancel: CancellationToken,
    ) -> Result<(), MediaError> {
        // Keep all potentially large hashes off the async runtime. Cancellation
        // is checked every 64 KiB, then immediately before publication.
        tokio::task::spawn_blocking(move || {
            check_cancel(&cancel)?;
            self.check_parent()?;
            if let Some(original) = &self.original {
                let replacement = fingerprint(&temporary, &cancel)?;
                let current =
                    fingerprint(&self.requested, &cancel).map_err(|error| match error {
                        MediaError::Cancelled => MediaError::Cancelled,
                        _ => MediaError::DestinationChanged,
                    })?;
                if current.canonical_path != original.canonical_path
                    || current.digest != original.digest
                    || !same_metadata(&original.metadata, &current.metadata)?
                {
                    return Err(MediaError::DestinationChanged);
                }
                self.check_parent()?;
                check_fingerprint(&self.requested, &current)?;
                check_fingerprint(&temporary, &replacement)?;
                check_cancel(&cancel)?;
                if current.digest == replacement.digest
                    && current.metadata.len() == replacement.metadata.len()
                {
                    // Identical output is complete without replacing the inode.
                    return Ok(());
                }
                // A final path check followed by persist() is insufficient:
                // another application can atomically save between them. Move
                // the actual occupant aside first, then inspect what we moved.
                let recovery = RecoveryFile::take(&self.target)?;
                let ready = (|| {
                    self.check_parent()?;
                    let moved = recovery.verify(original, &cancel)?;
                    check_fingerprint(&temporary, &replacement)?;
                    check_cancel(&cancel)?;
                    Ok(moved)
                })();
                let moved = match ready {
                    Ok(moved) => moved,
                    Err(error) => return Err(recovery.restore(&self.target, error)),
                };
                if let Err(error) = temporary.persist_noclobber(&self.target) {
                    let reason = if error.error.kind() == ErrorKind::AlreadyExists {
                        MediaError::DestinationChanged
                    } else {
                        MediaError::Io(error.error)
                    };
                    return Err(recovery.restore(&self.target, reason));
                }
                recovery.finish(&moved, &cancel)?;
            } else {
                // Authorization never applies to a file that appeared later.
                check_cancel(&cancel)?;
                temporary.persist_noclobber(&self.target).map_err(|error| {
                    if error.error.kind() == ErrorKind::AlreadyExists {
                        MediaError::DestinationExists
                    } else {
                        MediaError::Io(error.error)
                    }
                })?;
            }
            Ok(())
        })
        .await
        .map_err(worker_error)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stage(dir: &Path, bytes: &[u8]) -> TempPath {
        let file = tempfile::NamedTempFile::new_in(dir)
            .unwrap()
            .into_temp_path();
        fs::write(&file, bytes).unwrap();
        file
    }
    #[tokio::test]
    async fn explicit_overwrite_is_checked_and_identical_content_preserves_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        fs::write(&target, b"original").unwrap();
        assert!(matches!(
            Publication::prepare(target.clone(), false, CancellationToken::new()).await,
            Err(MediaError::DestinationExists)
        ));
        let before = fs::metadata(&target).unwrap();
        let publication = Publication::prepare(target.clone(), true, CancellationToken::new())
            .await
            .unwrap();
        publication
            .publish(stage(dir.path(), b"original"), CancellationToken::new())
            .await
            .unwrap();
        assert!(same_metadata(&before, &fs::metadata(&target).unwrap()).unwrap());
        let publication = Publication::prepare(target.clone(), true, CancellationToken::new())
            .await
            .unwrap();
        publication
            .publish(stage(dir.path(), b"replacement"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(fs::read(target).unwrap(), b"replacement");
    }
    #[tokio::test]
    async fn changed_late_and_cancelled_destinations_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        for existed in [false, true] {
            if existed {
                fs::write(&target, b"original").unwrap();
            }
            let publication = Publication::prepare(target.clone(), true, CancellationToken::new())
                .await
                .unwrap();
            fs::write(&target, b"user edit").unwrap();
            let result = publication
                .publish(stage(dir.path(), b"replacement"), CancellationToken::new())
                .await;
            assert!(matches!(
                result,
                Err(MediaError::DestinationExists) | Err(MediaError::DestinationChanged)
            ));
            assert_eq!(fs::read(&target).unwrap(), b"user edit");
        }
        let publication = Publication::prepare(target.clone(), true, CancellationToken::new())
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            publication
                .publish(stage(dir.path(), b"replacement"), cancel.clone())
                .await,
            Err(MediaError::Cancelled)
        ));
        assert!(matches!(
            Publication::prepare(target.clone(), true, cancel).await,
            Err(MediaError::Cancelled)
        ));
        assert_eq!(fs::read(target).unwrap(), b"user edit");
    }

    #[tokio::test]
    async fn moved_occupant_is_verified_even_if_saved_after_the_last_path_check() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        fs::write(&target, b"original").unwrap();
        let cancel = CancellationToken::new();
        let publication = Publication::prepare(target.clone(), true, cancel.clone())
            .await
            .unwrap();
        let original = publication.original.as_ref().unwrap();
        check_fingerprint(&target, original).unwrap();
        let user_save = stage(dir.path(), b"user atomic save");
        fs::rename(&user_save, &target).unwrap();
        // Exercise exactly the old check/rename race: the occupant moved to
        // recovery is the user's new file, not the authorized original inode.
        let recovery = RecoveryFile::take(&target).unwrap();
        let result = recovery.verify(original, &cancel);
        assert!(matches!(result, Err(MediaError::DestinationChanged)));
        assert!(matches!(
            recovery.restore(&target, MediaError::DestinationChanged),
            MediaError::DestinationChanged
        ));
        assert_eq!(fs::read(target).unwrap(), b"user atomic save");
    }

    #[tokio::test]
    async fn cancellation_after_move_restores_without_clobbering_a_late_save() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        for late_save in [false, true] {
            fs::write(&target, b"original").unwrap();
            let cancel = CancellationToken::new();
            let publication = Publication::prepare(target.clone(), true, cancel.clone())
                .await
                .unwrap();
            let recovery = RecoveryFile::take(&target).unwrap();
            // Rename-induced ctime changes alone must not reject the file.
            recovery
                .verify(publication.original.as_ref().unwrap(), &cancel)
                .unwrap();
            if late_save {
                fs::write(&target, b"late user save").unwrap();
            }
            cancel.cancel();
            assert!(matches!(
                recovery.verify(publication.original.as_ref().unwrap(), &cancel),
                Err(MediaError::Cancelled)
            ));
            let outcome = recovery.restore(&target, MediaError::Cancelled);
            if late_save {
                let MediaError::RecoveryRequired(path) = outcome else {
                    panic!("expected a retained recovery file, got {outcome:?}");
                };
                assert_eq!(fs::read(path).unwrap(), b"original");
                assert_eq!(fs::read(&target).unwrap(), b"late user save");
            } else {
                assert!(matches!(outcome, MediaError::Cancelled));
                assert_eq!(fs::read(&target).unwrap(), b"original");
            }
        }
    }

    #[tokio::test]
    async fn cancellation_after_commit_keeps_output_and_old_recovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        fs::write(&target, b"original").unwrap();
        let cancel = CancellationToken::new();
        let publication = Publication::prepare(target.clone(), true, cancel.clone())
            .await
            .unwrap();
        let recovery = RecoveryFile::take(&target).unwrap();
        let recovery_path = recovery.path.clone();
        let moved = recovery
            .verify(publication.original.as_ref().unwrap(), &cancel)
            .unwrap();
        stage(dir.path(), b"completed")
            .persist_noclobber(&target)
            .unwrap();
        cancel.cancel();
        recovery.finish(&moved, &cancel).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"completed");
        assert_eq!(fs::read(recovery_path).unwrap(), b"original");
    }

    #[tokio::test]
    async fn a_changed_recovery_file_is_never_cleaned_up_after_commit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        fs::write(&target, b"original").unwrap();
        let cancel = CancellationToken::new();
        let publication = Publication::prepare(target.clone(), true, cancel.clone())
            .await
            .unwrap();
        let recovery = RecoveryFile::take(&target).unwrap();
        let moved = recovery
            .verify(publication.original.as_ref().unwrap(), &cancel)
            .unwrap();
        stage(dir.path(), b"completed")
            .persist_noclobber(&target)
            .unwrap();
        fs::write(&recovery.path, b"user edited retained file").unwrap();
        let Err(MediaError::RecoveryRequired(path)) = recovery.finish(&moved, &cancel) else {
            panic!("changed recovery file must be retained");
        };
        assert_eq!(fs::read(target).unwrap(), b"completed");
        assert_eq!(fs::read(path).unwrap(), b"user edited retained file");
    }

    #[tokio::test]
    async fn atomic_external_saves_are_never_discarded() {
        use std::{sync::mpsc, time::Duration};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("movie.mp4");
        let writer_target = target.clone();
        let (tx, rx) = mpsc::channel::<(PathBuf, u64)>();
        let (ack_tx, ack_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            while let Ok((source, delay)) = rx.recv() {
                let until = std::time::Instant::now() + Duration::from_micros(delay);
                while std::time::Instant::now() < until {
                    std::hint::spin_loop();
                }
                fs::rename(source, &writer_target).unwrap();
                ack_tx.send(()).unwrap();
            }
        });
        // This is a real filesystem race, without a publication test hook.
        // Deterministic tests above also cover the formerly vulnerable gap.
        for attempt in 0..2_000 {
            fs::write(&target, b"original").unwrap();
            let publication = Publication::prepare(target.clone(), true, CancellationToken::new())
                .await
                .unwrap();
            let downloaded = stage(dir.path(), b"downloaded");
            let user_save = stage(dir.path(), b"user edit");
            tx.send((user_save.to_path_buf(), attempt % 128)).unwrap();
            let result = publication
                .publish(downloaded, CancellationToken::new())
                .await;
            ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(
                fs::read(&target).unwrap(),
                b"user edit",
                "external atomic save was lost at attempt {attempt}: {result:?}"
            );
            match result {
                Ok(()) | Err(MediaError::DestinationChanged) => {}
                Err(MediaError::RecoveryRequired(path)) => {
                    // A blocked rollback must preserve both versions.
                    assert_eq!(fs::read(&path).unwrap(), b"original");
                    fs::remove_file(path).unwrap();
                }
                Err(error) => panic!("unexpected publication error: {error:?}"),
            }
        }
        drop(tx);
        writer.join().unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_directories_and_parent_replacement_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.mp4");
        fs::write(&real, b"user data").unwrap();
        let link = dir.path().join("linked.mp4");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            Publication::prepare(link, true, CancellationToken::new()).await,
            Err(MediaError::UnsafeDestination)
        ));
        assert!(matches!(
            Publication::prepare(dir.path().into(), true, CancellationToken::new()).await,
            Err(MediaError::UnsafeDestination)
        ));
        let parent = dir.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let publication =
            Publication::prepare(parent.join("movie.mp4"), true, CancellationToken::new())
                .await
                .unwrap();
        let temporary = stage(&parent, b"downloaded");
        fs::rename(&parent, dir.path().join("moved-parent")).unwrap();
        fs::create_dir(&parent).unwrap();
        assert!(matches!(
            publication
                .publish(temporary, CancellationToken::new())
                .await,
            Err(MediaError::DestinationChanged)
        ));
        assert!(!parent.join("movie.mp4").exists());
        assert_eq!(fs::read(real).unwrap(), b"user data");
    }

    #[tokio::test]
    async fn large_fingerprint_does_not_block_the_runtime_and_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("large.mp4");
        File::create(&target)
            .unwrap()
            .set_len(1024 * 1024 * 1024)
            .unwrap();
        let cancel = CancellationToken::new();
        let worker = tokio::spawn(Publication::prepare(target.clone(), true, cancel.clone()));
        // This timer shares the single-thread runtime with prepare(). A sync
        // hash in the async task would prevent cancellation until completion.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(MediaError::Cancelled)));
        assert_eq!(fs::metadata(target).unwrap().len(), 1024 * 1024 * 1024);
    }
}
