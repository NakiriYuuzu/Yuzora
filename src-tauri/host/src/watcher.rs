//! Shared bounded filesystem notifications for desktop and host helpers.
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct WatcherHandle {
    _watcher: notify::RecommendedWatcher,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(".git" | "node_modules")))
}

/// Overflow becomes one root invalidation; it cannot grow the path buffer.
pub fn build_watcher(
    root: &Path,
    on_change: impl Fn(Vec<String>) + Send + 'static,
) -> Result<WatcherHandle, String> {
    const CAPACITY: usize = 4096;
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let (send, receive) = mpsc::sync_channel::<Vec<PathBuf>>(16);
    let overflow = Arc::new(AtomicBool::new(false));
    let overflow_callback = overflow.clone();
    let callback_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = &event {
            if event.need_rescan() {
                overflow_callback.store(true, Ordering::Release);
                return;
            }
            // Linux inotify reports reads/opens/closes too. Forwarding those
            // causes file reload -> read -> reload feedback loops.
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
        }
        let paths = match event {
            Ok(event) if event.paths.len() <= CAPACITY => event
                .paths
                .into_iter()
                .filter(|p| p.starts_with(&callback_root) && !is_ignored_path(p))
                .collect::<Vec<_>>(),
            _ => {
                overflow_callback.store(true, Ordering::Release);
                return;
            }
        };
        if !paths.is_empty() && send.try_send(paths).is_err() {
            overflow_callback.store(true, Ordering::Release);
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let thread = std::thread::spawn(move || {
        let mut paths = HashSet::new();
        let mut rescan = false;
        let mut deadline = Instant::now() + Duration::from_millis(300);
        while !stop.load(Ordering::Acquire) {
            match receive.recv_timeout(Duration::from_millis(50)) {
                Ok(batch) => {
                    if paths.len() + batch.len() > CAPACITY {
                        paths.clear();
                        rescan = true;
                    } else if !rescan {
                        paths.extend(batch);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if overflow.swap(false, Ordering::AcqRel) {
                paths.clear();
                rescan = true;
            }
            if Instant::now() < deadline {
                continue;
            }
            if !stop.load(Ordering::Acquire) && (rescan || !paths.is_empty()) {
                let changes = if rescan {
                    vec![root.to_string_lossy().into_owned()]
                } else {
                    paths
                        .drain()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect()
                };
                on_change(changes);
            }
            rescan = false;
            deadline = Instant::now() + Duration::from_millis(300);
        }
    });
    Ok(WatcherHandle {
        _watcher: watcher,
        stopped,
        thread: Some(thread),
    })
}
