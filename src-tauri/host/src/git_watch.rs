//! Bounded metadata invalidation, shared by native and remote repositories.
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct GitWatcher {
    stopped: Arc<AtomicBool>,
    wake: mpsc::SyncSender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for GitWatcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // A full slot already wakes the worker; never block the dropping thread.
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn build_repository_watcher(
    root: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Result<GitWatcher, String> {
    let dirs = crate::git_service::metadata_dirs(root)?;
    build_metadata_watcher(vec![dirs.git_dir, dirs.common_dir], on_change)
}

pub fn build_git_watcher(
    git_dir: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Result<GitWatcher, String> {
    build_metadata_watcher(vec![git_dir.to_owned()], on_change)
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_dir().then(|| (metadata.dev(), metadata.ino()))
}

/// Volume serial and file index. A creation time is no identity here: NTFS
/// tunnelling can hand a recreated directory its predecessor's timestamp.
#[cfg(windows)]
fn directory_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_SHARE_ALL: u32 = 0x0000_0007;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let (volume, index, is_directory) =
        crate::path_capability::windows_file_identity(&file).ok()?;
    is_directory.then_some((u64::from(volume), index))
}

fn build_metadata_watcher(
    mut dirs: Vec<PathBuf>,
    on_change: impl Fn() + Send + 'static,
) -> Result<GitWatcher, String> {
    dirs.sort();
    dirs.dedup();
    let (wake, receive) = mpsc::sync_channel(1);
    let changed = wake.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Reads performed by status/refresh must not cause a notification loop.
        if !matches!(event, Ok(ref event) if matches!(event.kind, notify::EventKind::Access(_))) {
            let _ = changed.try_send(());
        }
    })
    .map_err(|e| e.to_string())?;
    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
    }
    let refs: Vec<_> = dirs.iter().map(|dir| dir.join("refs")).collect();
    let mut identities = Vec::with_capacity(refs.len());
    for path in &refs {
        let identity = directory_identity(path);
        if identity.is_some() {
            watcher
                .watch(path, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
        }
        identities.push(identity);
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let thread = std::thread::spawn(move || {
        while receive.recv().is_ok() {
            if stop.load(Ordering::Acquire) {
                break;
            }
            // Block indefinitely while idle. During activity only, use a fixed
            // coalescing deadline so continuous output cannot starve refresh.
            let deadline = Instant::now() + Duration::from_millis(200);
            while !stop.load(Ordering::Acquire) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() || receive.recv_timeout(remaining).is_err() {
                    break;
                }
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            // refs can be removed/recreated or absent at open. Reattach before
            // invalidating the snapshot; never recurse into the object store.
            for (path, previous) in refs.iter().zip(&mut identities) {
                let current = directory_identity(path);
                if *previous == current {
                    continue;
                }
                // Re-register only a replaced directory. On macOS every
                // unwatch/watch restarts the FSEvents stream; doing this for
                // normal HEAD updates creates gaps and can lose ref changes.
                if previous.is_some() {
                    let _ = watcher.unwatch(path);
                }
                *previous = None;
                if current.is_some() && watcher.watch(path, RecursiveMode::Recursive).is_ok() {
                    *previous = current;
                }
            }
            if !stop.load(Ordering::Acquire) {
                on_change();
            }
        }
    });
    Ok(GitWatcher {
        stopped,
        wake,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_service::{run_ok, DEFAULT_TIMEOUT};

    /// Upper bound for an expected notification; loaded runners deliver FSEvents and
    /// ReadDirectoryChangesW batches seconds late, and a hit returns at once.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

    #[test]
    fn git_watcher_idle_has_no_periodic_callbacks_and_drop_wakes_worker() {
        let temp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let watcher = build_git_watcher(temp.path(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        // Drain any platform-specific initial notification before measuring idle.
        while rx.recv_timeout(Duration::from_millis(500)).is_ok() {}
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(650)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let start = Instant::now();
        drop(watcher);
        // Generous for loaded runners: a worker that never woke would hang the join instead.
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn directory_identity_tells_a_recreated_directory_apart() {
        let temp = tempfile::tempdir().unwrap();
        let refs = temp.path().join("refs");
        std::fs::create_dir(&refs).unwrap();
        let original = directory_identity(&refs).expect("directory identity");
        assert_eq!(directory_identity(&refs), Some(original));
        // Same name in the same parent right away: what NTFS tunnelling targets.
        std::fs::rename(&refs, temp.path().join("old-refs")).unwrap();
        std::fs::create_dir(&refs).unwrap();
        let replaced = directory_identity(&refs).expect("replacement identity");
        assert_ne!(replaced, original);
        std::fs::write(temp.path().join("file"), "").unwrap();
        assert_eq!(directory_identity(&temp.path().join("file")), None);
        assert_eq!(directory_identity(&temp.path().join("missing")), None);
    }

    #[test]
    fn git_watcher_reattaches_recreated_refs() {
        let temp = tempfile::tempdir().unwrap();
        let refs = temp.path().join("refs");
        std::fs::create_dir(&refs).unwrap();
        let (tx, rx) = mpsc::channel();
        let _watcher = build_git_watcher(temp.path(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        // Keep the old directory alive so a replacement cannot reuse its inode.
        std::fs::rename(&refs, temp.path().join("old-refs")).unwrap();
        std::fs::create_dir_all(refs.join("heads")).unwrap();
        rx.recv_timeout(EVENT_TIMEOUT).expect("refs replacement");
        while rx.recv_timeout(Duration::from_millis(500)).is_ok() {}
        std::fs::write(refs.join("heads/new-branch"), "fixture").unwrap();
        rx.recv_timeout(EVENT_TIMEOUT)
            .expect("reattached refs change");
    }

    #[test]
    fn linked_worktree_private_index_shared_refs_and_operation_state() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |root: &Path, args: &[&str]| {
            run_ok(root, args, DEFAULT_TIMEOUT, &[]).unwrap();
        };
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        let linked = temp.path().join("中文 worktree");
        git(
            &repo,
            &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        );
        assert!(linked.join(".git").is_file());
        let dirs = crate::git_service::metadata_dirs(&linked).unwrap();
        assert_ne!(dirs.git_dir, dirs.common_dir);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let watcher = build_repository_watcher(&linked, move || {
            let _ = tx.try_send(());
        })
        .unwrap();
        std::fs::write(dirs.git_dir.join("MERGE_HEAD"), "fixture").unwrap();
        rx.recv_timeout(EVENT_TIMEOUT)
            .expect("private git-dir change");
        assert_eq!(
            crate::git_service::status_of(&linked, None)
                .unwrap()
                .in_progress
                .as_deref(),
            Some("merge")
        );
        std::fs::remove_file(dirs.git_dir.join("MERGE_HEAD")).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        while rx.try_recv().is_ok() {}
        git(&repo, &["branch", "shared-ref"]);
        rx.recv_timeout(EVENT_TIMEOUT)
            .expect("common-dir refs change");
        drop(watcher);
        while rx.try_recv().is_ok() {}
        git(&repo, &["branch", "after-close"]);
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }
}
