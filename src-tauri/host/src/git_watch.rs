//! Bounded metadata invalidation, shared by native and remote repositories.
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub struct GitWatcher {
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for GitWatcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
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

fn directory_identity(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let created = metadata
            .created()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some((created.as_secs(), u64::from(created.subsec_nanos())))
    }
}

fn build_metadata_watcher(
    mut dirs: Vec<PathBuf>,
    on_change: impl Fn() + Send + 'static,
) -> Result<GitWatcher, String> {
    dirs.sort();
    dirs.dedup();
    let dirty = Arc::new(AtomicBool::new(false));
    let changed = dirty.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Reads performed by status/refresh must not cause a notification loop.
        if !matches!(event, Ok(ref event) if matches!(event.kind, notify::EventKind::Access(_))) {
            changed.store(true, Ordering::Release);
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
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            if !dirty.swap(false, Ordering::AcqRel) {
                continue;
            }
            // Fixed coalescing window; continuous output cannot starve refresh.
            std::thread::sleep(Duration::from_millis(200));
            dirty.store(false, Ordering::Release);
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
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_service::{run_ok, DEFAULT_TIMEOUT};

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
        rx.recv_timeout(Duration::from_secs(5))
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
        rx.recv_timeout(Duration::from_secs(5))
            .expect("common-dir refs change");
        drop(watcher);
        while rx.try_recv().is_ok() {}
        git(&repo, &["branch", "after-close"]);
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }
}
