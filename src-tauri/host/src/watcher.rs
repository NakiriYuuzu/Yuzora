//! Shared bounded filesystem notifications for desktop and host helpers.
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct WatcherHandle {
    _watcher: notify::RecommendedWatcher,
    wake: mpsc::SyncSender<Vec<PathBuf>>,
    #[cfg(test)]
    test_overflow: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // A full queue already wakes the worker. Never block teardown on it.
        let _ = self.wake.try_send(Vec::new());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(".git" | "node_modules")))
}

const CAPACITY: usize = 4096;
const COALESCE_INTERVAL: Duration = Duration::from_millis(300);

fn request_rescan(overflow: &AtomicBool, send: &mpsc::SyncSender<Vec<PathBuf>>) {
    if !overflow.swap(true, Ordering::AcqRel) {
        // A full queue already wakes the worker; one hint is enough until flush.
        let _ = send.try_send(Vec::new());
    }
}

fn advance_idle_deadline(deadline: Instant, now: Instant) -> Instant {
    if now <= deadline {
        return deadline;
    }
    // Keep the existing window phase without waking for missed idle slots.
    let remainder = now.duration_since(deadline).as_nanos() % COALESCE_INTERVAL.as_nanos();
    if remainder == 0 {
        now
    } else {
        now + COALESCE_INTERVAL - Duration::from_nanos(remainder as u64)
    }
}

// Root rules only: nested .gitignore files and global Git excludes are not
// loaded. Keep file-only ignores observable so open editors still reload.
fn ignore_matcher(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    // Read the whole bounded, pinned regular file before accepting any rules.
    // A replaced ancestor, symlink, FIFO or oversized file contributes no rules.
    for relative in [".git/info/exclude", ".gitignore"] {
        match read_ignore_file(root, relative) {
            Ok(Some(text)) => {
                for (index, line) in text.lines().enumerate() {
                    let line = if index == 0 {
                        line.trim_start_matches('\u{feff}')
                    } else {
                        line
                    };
                    let _ = builder.add_line(Some(root.join(relative)), line);
                }
            }
            Ok(None) => {}
            Err(_) => warn_ignored_file_once(&root.join(relative)),
        }
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn read_ignore_file(root: &Path, relative: &str) -> Result<Option<String>, String> {
    use crate::path_capability::{PinnedDir, SafeLeafName};
    let mut directory = PinnedDir::open_dir(root)?;
    let (parents, leaf) = relative.rsplit_once('/').unwrap_or(("", relative));
    for parent in parents.split('/').filter(|part| !part.is_empty()) {
        let Some(next) = directory.open_subdir_optional(&SafeLeafName::parse(parent)?)? else {
            return Ok(None);
        };
        directory = next;
    }
    let Some(opened) = directory.open_file_optional(&SafeLeafName::parse(leaf)?)? else {
        return Ok(None);
    };
    let limit = crate::protocol::MAX_FILE_BYTES;
    if opened.len > limit {
        return Err("ignore-file-too-large".into());
    }
    let mut bytes = Vec::new();
    opened
        .file
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("ignore-file-too-large".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "ignore-file-not-utf8".into())
}

fn warn_ignored_file_once(path: &Path) {
    // Bounded process-wide diagnostic deduplication, including watcher restarts.
    static WARNED: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<PathBuf>>> =
        std::sync::OnceLock::new();
    if let Ok(mut warned) = WARNED.get_or_init(Default::default).lock() {
        if warned.iter().any(|seen| seen == path) {
            return;
        }
        if warned.len() == 128 {
            warned.pop_front();
        }
        warned.push_back(path.to_owned());
        eprintln!(
            "watcher skipped unsafe or oversized ignore file: {}",
            path.display()
        );
    }
}

/// Returns the (possibly coalesced) path and whether root rules ignore it.
fn coalesce_path(root: &Path, matcher: &Gitignore, path: PathBuf) -> (PathBuf, bool) {
    let mut directories: Vec<_> = path
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != root && parent.starts_with(root))
        .collect();
    directories.reverse();
    for directory in directories {
        if matcher.matched(directory, true).is_ignore() {
            return (directory.to_owned(), true);
        }
    }
    // Only stat the leaf if no ancestor matched; build storms usually exit above.
    if path != root && matcher.matched(&path, true).is_ignore() && path.is_dir() {
        return (path, true);
    }
    let ignored = path != root && matcher.matched(&path, false).is_ignore();
    (path, ignored)
}

/// Most ignored paths a single batch may verify; larger batches just refresh.
const TRACKED_CHECK_LIMIT: usize = 256;

/// True only when Git confirms every path is really ignored and nothing tracked
/// lives at or under it. An ignore rule never hides edits to force-added files,
/// and the root-only matcher can over-ignore (nested `.gitignore` negations), so
/// ignored-only batches must still prove they cannot change `git status`. Any
/// doubt (including `cancel`, set while the watcher is being dropped) means
/// "relevant".
fn all_untracked(root: &Path, paths: &[&PathBuf], cancel: &Arc<AtomicBool>) -> bool {
    if paths.is_empty() || paths.len() > TRACKED_CHECK_LIMIT || cancel.load(Ordering::Acquire) {
        return false;
    }
    let Some(specs) = paths
        .iter()
        .map(|path| path.strip_prefix(root).ok()?.to_str())
        .map(|relative| {
            relative.map(|r| {
                if cfg!(windows) {
                    r.replace('\\', "/")
                } else {
                    r.to_owned()
                }
            })
        })
        .collect::<Option<Vec<String>>>()
    else {
        return false;
    };
    if specs
        .iter()
        .any(|spec| spec.is_empty() || spec.starts_with(':'))
    {
        return false;
    }
    crate::cancellation::with_cancellation(cancel.clone(), || {
        let mut args = vec!["ls-files", "-z", "--cached", "--"];
        args.extend(specs.iter().map(String::as_str));
        let tracked = crate::git_service::run_git(root, &args, Duration::from_secs(10), &[]);
        if !matches!(&tracked, Ok(out) if out.code == 0 && out.stdout.is_empty()) {
            return false;
        }
        let mut input = Vec::new();
        for spec in &specs {
            input.extend_from_slice(spec.as_bytes());
            input.push(0);
        }
        // Exit 1 = nothing ignored; both that and a short answer mean "relevant".
        // check-ignore rejects literal pathspecs, so magic parsing is enabled; a
        // spec starting with ':' would be read as magic and is refused above.
        let magic = [(
            crate::git_service::ALLOW_PATHSPEC_MAGIC_ENV.to_string(),
            "1".to_string(),
        )];
        let r = crate::git_service::run_git_with_stdin(
            root,
            &["check-ignore", "-z", "--stdin"],
            Duration::from_secs(10),
            &magic,
            &input,
        );
        let Ok(out) = r else {
            return false;
        };
        out.code == 0
            && out
                .stdout
                .split(|byte| *byte == 0)
                .filter(|item| !item.is_empty())
                .count()
                == specs.len()
    })
}

/// Classify and hand over the pending batch. Stop is re-checked afterwards: a
/// cancelled classification reports "relevant", which must not reach the callback.
fn deliver(
    pending: &mut PendingChanges,
    root: &Path,
    stop: &Arc<AtomicBool>,
    on_change: &impl Fn(Vec<String>, bool),
) {
    if let Some((changes, git_relevant)) = pending.take_classified(root, stop) {
        if !stop.load(Ordering::Acquire) {
            on_change(changes, git_relevant);
        }
    }
}

#[derive(Default)]
struct PendingChanges {
    /// Coalesced path -> whether root ignore rules cover it.
    paths: HashMap<PathBuf, bool>,
    rescan: bool,
}

impl PendingChanges {
    fn invalidate_root(&mut self) {
        self.paths.clear();
        self.rescan = true;
    }

    fn extend(&mut self, root: &Path, matcher: &mut Gitignore, batch: Vec<PathBuf>) {
        // Reload before classifying any paths in this batch. A rescan reloads
        // too, since an overflow may have lost the ignore-file notification.
        if batch
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == ".gitignore"))
        {
            *matcher = ignore_matcher(root);
        }
        if self.rescan {
            return;
        }
        for path in batch {
            let (path, ignored) = coalesce_path(root, matcher, path);
            let known = self.paths.entry(path).or_insert(ignored);
            *known &= ignored;
            if self.paths.len() > CAPACITY {
                self.invalidate_root();
                break;
            }
        }
    }

    #[cfg(test)]
    fn take(&mut self, root: &Path) -> Option<Vec<String>> {
        self.take_classified(root, &Arc::default())
            .map(|(changes, _)| changes)
    }

    /// Changed paths plus whether the batch can affect `git status`.
    fn take_classified(
        &mut self,
        root: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Option<(Vec<String>, bool)> {
        if !self.rescan && self.paths.is_empty() {
            return None;
        }
        let (changes, git_relevant) = if self.rescan {
            (vec![root.to_string_lossy().into_owned()], true)
        } else {
            let drained: Vec<_> = self.paths.drain().collect();
            let ignored: Vec<_> = drained
                .iter()
                .filter(|(_, ignored)| *ignored)
                .map(|(path, _)| path)
                .collect();
            // An ignore-file edit can expose or hide other files even when it ignores itself.
            let ignore_rules_changed = drained
                .iter()
                .any(|(path, _)| path.file_name().is_some_and(|name| name == ".gitignore"));
            let relevant = ignore_rules_changed
                || ignored.len() != drained.len()
                || !all_untracked(root, &ignored, cancel);
            (
                drained
                    .iter()
                    .map(|(p, _)| p.to_string_lossy().into_owned())
                    .collect(),
                relevant,
            )
        };
        self.rescan = false;
        Some((changes, git_relevant))
    }
}

/// Overflow becomes one root invalidation; it cannot grow the path buffer.
pub fn build_watcher(
    root: &Path,
    on_change: impl Fn(Vec<String>) + Send + 'static,
) -> Result<WatcherHandle, String> {
    build_classified_watcher(root, move |paths, _| on_change(paths))
}

/// Like `build_watcher`, plus `git_relevant`: false when Git confirms paths are
/// ignored/untracked in the workspace-root repository, not independent nested repositories.
pub fn build_classified_watcher(
    root: &Path,
    on_change: impl Fn(Vec<String>, bool) + Send + 'static,
) -> Result<WatcherHandle, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut matcher = ignore_matcher(&root);
    let (send, receive) = mpsc::sync_channel::<Vec<PathBuf>>(1024);
    let wake = send.clone();
    let overflow = Arc::new(AtomicBool::new(false));
    let overflow_callback = overflow.clone();
    #[cfg(test)]
    let test_overflow = overflow.clone();
    let callback_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = &event {
            if event.need_rescan() {
                request_rescan(&overflow_callback, &send);
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
                request_rescan(&overflow_callback, &send);
                return;
            }
        };
        if !paths.is_empty() && send.try_send(paths).is_err() {
            request_rescan(&overflow_callback, &send);
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let thread = std::thread::spawn(move || {
        let mut pending = PendingChanges::default();
        let mut deadline = Instant::now() + COALESCE_INTERVAL;
        while !stop.load(Ordering::Acquire) {
            let received = if !pending.rescan && pending.paths.is_empty() {
                let received = receive
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected);
                deadline = advance_idle_deadline(deadline, Instant::now());
                received
            } else {
                receive.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            };
            if stop.load(Ordering::Acquire) {
                break;
            }
            match received {
                Ok(batch) => pending.extend(&root, &mut matcher, batch),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if overflow.load(Ordering::Acquire) {
                pending.invalidate_root();
            }
            if Instant::now() < deadline {
                continue;
            }
            // Reload once per overflow window, before publishing its rescan.
            // Clearing before the callback lets later errors queue a new hint.
            if overflow.swap(false, Ordering::AcqRel) {
                matcher = ignore_matcher(&root);
                pending.invalidate_root();
            }
            if !stop.load(Ordering::Acquire) {
                deliver(&mut pending, &root, &stop, &on_change);
            }
            deadline = Instant::now() + COALESCE_INTERVAL;
        }
    });
    Ok(WatcherHandle {
        _watcher: watcher,
        wake,
        #[cfg(test)]
        test_overflow,
        stopped,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_deadline_preserves_window_phase_without_restarting_the_delay() {
        let start = Instant::now();
        let deadline = start + COALESCE_INTERVAL;
        assert_eq!(advance_idle_deadline(deadline, start), deadline);
        assert_eq!(
            advance_idle_deadline(deadline, start + Duration::from_millis(1100)),
            start + Duration::from_millis(1200)
        );
        let boundary = start + Duration::from_secs(24 * 60 * 60);
        assert_eq!(advance_idle_deadline(deadline, boundary), boundary);
        assert_eq!(
            advance_idle_deadline(deadline, boundary + Duration::from_millis(1)),
            boundary + COALESCE_INTERVAL
        );
    }

    #[test]
    fn overflow_wakes_an_idle_receiver_once_and_survives_a_full_queue() {
        let (send, receive) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        for _ in 0..100 {
            request_rescan(&overflow, &send);
        }
        assert!(overflow.load(Ordering::Acquire));
        assert!(receive.try_recv().unwrap().is_empty());
        assert!(receive.try_recv().is_err());
        overflow.store(false, Ordering::Release);
        send.try_send(vec![PathBuf::from("queued-change")]).unwrap();
        request_rescan(&overflow, &send);
        assert_eq!(
            receive.try_recv().unwrap(),
            vec![PathBuf::from("queued-change")]
        );
        assert!(overflow.swap(false, Ordering::AcqRel));
        request_rescan(&overflow, &send);
        assert!(receive.try_recv().unwrap().is_empty());
    }

    #[test]
    fn an_overflow_without_paths_reaches_the_idle_watcher() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let (send, receive) = mpsc::channel();
        let watcher = build_watcher(&root, move |paths| {
            let _ = send.send(paths);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(700));
        // FSEvents may still report the temp directory's own creation late (seen on Intel runners).
        while receive.try_recv().is_ok() {}
        request_rescan(&watcher.test_overflow, &watcher.wake);
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(2)).unwrap(),
            vec![root.to_string_lossy().into_owned()]
        );
        assert!(receive.recv_timeout(Duration::from_millis(350)).is_err());
        drop(watcher);
    }

    #[test]
    fn bounded_ignore_preserves_bom_precedence_and_skips_oversized_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git/info")).unwrap();
        std::fs::write(root.path().join(".git/info/exclude"), "build/\n").unwrap();
        std::fs::write(
            root.path().join(".gitignore"),
            "\u{feff}!build/\r\ncache/\r\n",
        )
        .unwrap();
        let matcher = ignore_matcher(root.path());
        assert!(!matcher.matched(root.path().join("build"), true).is_ignore());
        assert!(matcher.matched(root.path().join("cache"), true).is_ignore());
        let file = std::fs::File::create(root.path().join(".gitignore")).unwrap();
        file.set_len(crate::protocol::MAX_FILE_BYTES + 1).unwrap();
        assert!(read_ignore_file(root.path(), ".gitignore").is_err());
        assert!(ignore_matcher(root.path())
            .matched(root.path().join("build"), true)
            .is_ignore());
    }

    #[cfg(unix)]
    #[test]
    fn ignore_files_never_follow_symlinks_or_open_special_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".gitignore");
        std::os::unix::fs::symlink("/dev/zero", &path).unwrap();
        assert!(read_ignore_file(root.path(), ".gitignore").is_err());
        std::fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_ignore_file(root.path(), ".gitignore").is_err());
        assert!(!ignore_matcher(root.path())
            .matched(root.path().join("file"), false)
            .is_ignore());
    }

    #[test]
    fn watcher_stop_wakeup_does_not_emit_a_change() {
        let root = tempfile::tempdir().unwrap();
        let (send, receive) = mpsc::channel();
        let watcher = build_watcher(root.path(), move |paths| {
            let _ = send.send(paths);
        })
        .unwrap();
        drop(watcher);
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn watcher_stops_with_a_full_pending_queue() {
        let root = tempfile::tempdir().unwrap();
        let (entered_send, entered_receive) = mpsc::channel();
        let (release_send, release_receive) = mpsc::channel();
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = callbacks.clone();
        let watcher = build_watcher(root.path(), move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            let _ = entered_send.send(());
            let _ = release_receive.recv();
        })
        .unwrap();
        watcher
            .wake
            .send(vec![root.path().join("pending.txt")])
            .unwrap();
        entered_receive
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        // Hold the callback while filling the real bounded input queue, then
        // request shutdown before letting that callback finish.
        while watcher
            .wake
            .try_send(vec![root.path().join("queued.txt")])
            .is_ok()
        {}
        let stopped = watcher.stopped.clone();
        let (closed_send, closed_receive) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            drop(watcher);
            let _ = closed_send.send(());
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !stopped.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let saw_stop = stopped.load(Ordering::Acquire);
        release_send.send(()).unwrap();
        // Dropping also stops notify's FSEvents stream, which takes seconds
        // while fseventsd is busy; a watcher that never closes still fails.
        closed_receive
            .recv_timeout(Duration::from_secs(20))
            .expect("dropping the watcher must finish once its callback returns");
        thread.join().unwrap();
        assert!(saw_stop);
        assert_eq!(callbacks.load(Ordering::Relaxed), 1);
        assert_eq!(Arc::strong_count(&callbacks), 1);
        assert_eq!(Arc::strong_count(&stopped), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "manual owned filesystem watcher performance measurement"]
    fn performance_owned_watcher_lifecycle() {
        use std::sync::atomic::AtomicUsize;

        if cfg!(debug_assertions) {
            panic!("Use --release");
        }
        fn usage() -> (f64, i64, u64, u64, u64) {
            let (cpu, task) = unsafe {
                let mut cpu = std::mem::MaybeUninit::<libc::rusage>::uninit();
                assert_eq!(libc::getrusage(libc::RUSAGE_SELF, cpu.as_mut_ptr()), 0);
                let mut task = std::mem::MaybeUninit::<libc::rusage_info_v0>::uninit();
                assert_eq!(
                    libc::proc_pid_rusage(
                        std::process::id() as libc::c_int,
                        libc::RUSAGE_INFO_V0,
                        task.as_mut_ptr().cast(),
                    ),
                    0
                );
                (cpu.assume_init(), task.assume_init())
            };
            (
                (cpu.ru_utime.tv_sec + cpu.ru_stime.tv_sec) as f64 * 1000.0
                    + (cpu.ru_utime.tv_usec + cpu.ru_stime.tv_usec) as f64 / 1000.0,
                cpu.ru_nvcsw,
                task.ri_interrupt_wkups,
                task.ri_resident_size,
                task.ri_phys_footprint,
            )
        }
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let extended = std::env::var_os("YUZORA_PERF_WATCHER_SOAK").is_some();
        if !extended {
            for count in [0, 1, 8] {
                let callbacks = Arc::new(AtomicUsize::new(0));
                let mut watchers = Vec::new();
                for index in 0..count {
                    let path = root.join(format!("idle-{count}-{index}"));
                    std::fs::create_dir(&path).unwrap();
                    let callbacks = callbacks.clone();
                    watchers.push(
                        build_watcher(&path, move |_| {
                            callbacks.fetch_add(1, Ordering::Relaxed);
                        })
                        .unwrap(),
                    );
                }
                std::thread::sleep(Duration::from_secs(2));
                for sample in 0..3 {
                    let before_callbacks = callbacks.load(Ordering::Relaxed);
                    let before = usage();
                    let started = Instant::now();
                    std::thread::sleep(Duration::from_secs(2));
                    let wall = started.elapsed().as_secs_f64() * 1000.0;
                    let after = usage();
                    let events = callbacks.load(Ordering::Relaxed) - before_callbacks;
                    assert_eq!(events, 0, "idle fixture received filesystem activity");
                    println!(
                        "WATCHER_IDLE {}",
                        serde_json::json!({
                            "watchers": count, "sample": sample, "wallMs": wall,
                            "cpuMs": after.0 - before.0,
                            "voluntaryContextSwitches": after.1 - before.1,
                            "interruptWakeups": after.2 - before.2,
                            "rssBytes": after.3, "footprintBytes": after.4,
                            "fds": std::fs::read_dir("/dev/fd").unwrap().count(), "callbacks": events,
                        })
                    );
                }
                drop(watchers);
                assert_eq!(Arc::strong_count(&callbacks), 1);
            }

            let event_root = root.join("events");
            std::fs::create_dir(&event_root).unwrap();
            let (send, receive) = mpsc::channel();
            let watcher = build_watcher(&event_root, move |paths| {
                let _ = send.send(paths);
            })
            .unwrap();
            let mut latencies = Vec::new();
            for index in 0..32 {
                std::thread::sleep(Duration::from_millis([0, 35, 115, 225][index % 4]));
                let path = event_root.join(format!("change-{index}"));
                let started = Instant::now();
                std::fs::write(&path, b"owned fixture").unwrap();
                let deadline = started + Duration::from_secs(5);
                loop {
                    let paths = receive
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .unwrap();
                    if paths.contains(&path.to_string_lossy().into_owned()) {
                        break;
                    }
                }
                if index >= 4 {
                    latencies.push(started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            drop(watcher);
            println!(
                "WATCHER_EVENTS {}",
                serde_json::json!({ "warmup": 4, "operations": latencies.len(), "latencyMs": latencies })
            );

            let burst_root = root.join("burst");
            std::fs::create_dir(&burst_root).unwrap();
            let (send, receive) = mpsc::channel();
            let watcher = build_watcher(&burst_root, move |paths| {
                let _ = send.send((Instant::now(), paths));
            })
            .unwrap();
            for sample in 0..4 {
                let before = usage();
                let started = Instant::now();
                for index in 0..500 {
                    std::fs::write(burst_root.join(format!("{sample}-{index}")), b"burst").unwrap();
                }
                let marker = burst_root.join(format!("marker-{sample}"));
                std::fs::write(&marker, b"complete").unwrap();
                let written = Instant::now();
                let deadline = started + Duration::from_secs(5);
                let root_invalidation;
                loop {
                    let (notified, paths) = receive
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .unwrap();
                    if paths.contains(&marker.to_string_lossy().into_owned()) {
                        root_invalidation = false;
                        break;
                    }
                    // Overflow is explicitly a root invalidation. Only accept one
                    // emitted after all writes, never an older queued prelude.
                    if notified >= written
                        && paths.contains(&burst_root.to_string_lossy().into_owned())
                    {
                        root_invalidation = true;
                        break;
                    }
                }
                let after = usage();
                println!(
                    "WATCHER_BURST {}",
                    serde_json::json!({
                        "sample": sample, "warmup": sample == 0, "writes": 501,
                        "cpuMs": after.0 - before.0, "wallMs": started.elapsed().as_secs_f64() * 1000.0,
                        "rootInvalidation": root_invalidation,
                        "scope": "includes fixture filesystem writes and actual marker delivery",
                    })
                );
            }
            drop(watcher);
        }

        let lifecycle_root = root.join("lifecycle");
        std::fs::create_dir(&lifecycle_root).unwrap();
        let mut drops = Vec::new();
        let warmup: usize = if extended { 100 } else { 10 };
        let measured = if extended { 1000 } else { 100 };
        let sample_every = if extended { 100 } else { 10 };
        for cycle in 0..warmup + measured {
            let callbacks = Arc::new(AtomicUsize::new(0));
            let retained = callbacks.clone();
            let watcher = build_watcher(&lifecycle_root, move |_| {
                retained.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();
            std::thread::sleep(Duration::from_millis(10));
            let started = Instant::now();
            drop(watcher);
            drops.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(Arc::strong_count(&callbacks), 1);
            if (cycle + 1) % sample_every == 0 {
                let resource = usage();
                println!(
                    "WATCHER_LIFECYCLE {}",
                    serde_json::json!({
                        "completedCycles": (cycle + 1).saturating_sub(warmup), "warmup": warmup,
                        "warmupSample": cycle < warmup,
                        "dropMs": drops, "rssBytes": resource.3, "footprintBytes": resource.4,
                        "fds": std::fs::read_dir("/dev/fd").unwrap().count(),
                        "callbackOwnersAfterClose": Arc::strong_count(&callbacks),
                    })
                );
                drops.clear();
            }
        }
    }

    #[test]
    fn watcher_coalesces_tracked_then_ignored_modification_reported_by_status() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        std::fs::create_dir(root.join("generated")).unwrap();
        crate::git_service::test_repo::write_and_commit(
            root,
            "generated/config.json",
            "{}",
            "track config",
        );
        std::fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        std::fs::write(root.join("generated/config.json"), "{\"changed\":true}").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(root, &mut matcher, vec![root.join("generated/config.json")]);
        assert_eq!(
            pending.take(root).unwrap(),
            vec![root.join("generated").to_string_lossy()]
        );
        let status = crate::git_service::status_of(root, None).unwrap();
        assert!(status
            .parsed
            .unstaged
            .iter()
            .any(|entry| entry.path == "generated/config.json"));
    }

    #[test]
    fn watcher_coalesces_ignored_directory_storm_before_capacity_check() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        for offset in 0..10 {
            pending.extend(
                root,
                &mut matcher,
                (0..1000)
                    .map(|i| root.join(format!("target/debug/{offset}-{i}.o")))
                    .collect(),
            );
        }
        assert_eq!(pending.paths.len(), 1, "10,000 paths coalesce to one");
        assert!(!pending.rescan);
        let paths = pending.take(root).unwrap();
        assert_eq!(paths, vec![root.join("target").to_string_lossy()]);
        assert!(pending.take(root).is_none());
    }

    fn classify(root: &Path, changed: &[&str]) -> bool {
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(
            root,
            &mut matcher,
            changed.iter().map(|p| root.join(p)).collect(),
        );
        pending.take_classified(root, &Arc::default()).unwrap().1
    }

    #[test]
    fn ignored_only_batches_are_not_git_relevant_but_mixed_or_unverifiable_are() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // Not a repository: Git cannot vouch for the ignored paths, so refresh.
        std::fs::write(root.join(".gitignore"), "target/\n.env\n").unwrap();
        assert!(classify(root, &["target/debug/a.o"]));
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, "src.txt", "x", "seed");
        // Git only vouches for ignored directories that still exist.
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".env"), "A=1").unwrap();
        assert!(!classify(root, &["target/debug/a.o", "target/b.o"]));
        assert!(!classify(root, &[".env"]));
        assert!(classify(root, &["target/debug/a.o", "src.txt"]));
        assert!(classify(root, &["src.txt"]));
        assert!(classify(root, &[".gitignore"]));
        let mut pending = PendingChanges::default();
        pending.invalidate_root();
        assert!(pending.take_classified(root, &Arc::default()).unwrap().1);
    }

    #[test]
    fn a_self_ignored_gitignore_edit_stays_git_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, "src.txt", "x", "seed");
        // Untracked and ignoring itself, yet editing it still changes what status lists.
        std::fs::write(root.join(".gitignore"), ".gitignore\n*.log\n").unwrap();
        assert!(classify(root, &[".gitignore"]));
    }

    #[test]
    fn nested_gitignore_negation_keeps_untracked_file_git_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, "src.txt", "x", "seed");
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/.gitignore"), "!keep.log\n").unwrap();
        std::fs::write(root.join("sub/keep.log"), "k").unwrap();
        std::fs::write(root.join("sub/drop.log"), "d").unwrap();
        // The root-only matcher calls both ignored; only Git knows `keep.log` is not.
        assert!(classify(root, &["sub/keep.log"]));
        assert!(!classify(root, &["sub/drop.log"]));
        assert!(classify(root, &["sub/drop.log", "sub/keep.log"]));
        // A deleted untracked non-ignored file also changes `git status`.
        std::fs::remove_file(root.join("sub/keep.log")).unwrap();
        assert!(classify(root, &["sub/keep.log"]));
    }

    #[test]
    fn cancelled_watcher_skips_git_and_reports_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, "src.txt", "x", "seed");
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(root.join(".env"), "A=1").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(root, &mut matcher, vec![root.join(".env")]);
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(pending.take_classified(root, &cancel).unwrap().1);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_relative_paths_are_git_relevant() {
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // A repository whose rules ignore the lossy spelling `bad\u{fffd}.o`: a
        // lossy conversion would call the batch ignored-and-untracked.
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, ".gitignore", "*.o\n", "ignore");
        let path = root.join(std::ffi::OsStr::from_bytes(b"bad\xff.o"));
        assert!(!all_untracked(root, &[&path], &Arc::default()));
        // Sanity: the same rule does confirm a valid name.
        assert!(all_untracked(
            root,
            &[&root.join("good.o")],
            &Arc::default()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn tracked_file_with_a_backslash_in_its_name_is_git_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, ".gitignore", "foo*\n", "ignore");
        std::fs::write(root.join("foo\\bar"), "x").unwrap();
        crate::git_service::test_repo::git(root, &["add", "-f", "--", "foo\\bar"]);
        crate::git_service::test_repo::git(root, &["commit", "-m", "track"]);
        std::fs::write(root.join("foo\\bar"), "y").unwrap();
        assert!(!all_untracked(
            root,
            &[&root.join("foo\\bar")],
            &Arc::default()
        ));
        // Control: an untracked name matching the same rule is still skippable.
        assert!(all_untracked(
            root,
            &[&root.join("foo.txt")],
            &Arc::default()
        ));
    }

    #[test]
    fn colon_specs_are_git_relevant_without_running_git() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, ".gitignore", "*.o\n", "ignore");
        assert!(all_untracked(root, &[&root.join("a.o")], &Arc::default()));
        for name in [":(icase)A.o", ":x.o"] {
            assert!(!all_untracked(root, &[&root.join(name)], &Arc::default()));
        }
        assert!(!all_untracked(
            root,
            &[&root.join("a.o"), &root.join(":(literal)b.o")],
            &Arc::default()
        ));
    }

    #[test]
    fn no_callback_is_delivered_after_stop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        crate::git_service::test_repo::write_and_commit(root, "src.txt", "x", "seed");
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(root, &mut matcher, vec![root.join(".env")]);
        let calls = std::cell::Cell::new(0);
        let stop = Arc::new(AtomicBool::new(true));
        deliver(&mut pending, root, &stop, &|_, _| {
            calls.set(calls.get() + 1)
        });
        assert_eq!(calls.get(), 0);
        // Control: the same batch is delivered while running.
        let mut pending = PendingChanges::default();
        pending.extend(root, &mut matcher, vec![root.join(".env")]);
        deliver(&mut pending, root, &Arc::default(), &|_, _| {
            calls.set(calls.get() + 1)
        });
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn tracked_files_inside_ignored_locations_stay_git_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::git_service::test_repo::init(root);
        std::fs::create_dir(root.join("generated")).unwrap();
        crate::git_service::test_repo::write_and_commit(
            root,
            "generated/config.json",
            "{}",
            "track config",
        );
        crate::git_service::test_repo::write_and_commit(root, ".env", "A=1", "track env");
        std::fs::write(root.join(".gitignore"), "generated/\n.env\n").unwrap();
        assert!(classify(root, &["generated/config.json"]));
        assert!(classify(
            root,
            &["generated/new.json", "generated/config.json"]
        ));
        assert!(classify(root, &[".env"]));
        // Untracked siblings in the same ignored directory do not change that.
        std::fs::write(root.join("generated/untracked.txt"), "u").unwrap();
        assert!(classify(root, &["generated/untracked.txt"]));
    }

    #[test]
    fn watcher_preserves_ignored_files_in_mixed_batches() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "target/\n.env\n*.local\n").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(
            root,
            &mut matcher,
            vec![
                root.join("target/out"),
                root.join(".env"),
                root.join("src/dev.local"),
            ],
        );
        let paths = pending.take(root).unwrap();
        assert_eq!(paths.len(), 3);
        for path in ["target", ".env", "src/dev.local"] {
            assert!(paths.contains(&root.join(path).to_string_lossy().into_owned()));
        }
        pending.extend(root, &mut matcher, vec![root.join("target/out")]);
        assert_eq!(
            pending.take(root).unwrap(),
            vec![root.join("target").to_string_lossy()]
        );
    }

    #[test]
    fn watcher_gitignore_edit_reloads_rules_before_classifying_batch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        pending.extend(root, &mut matcher, vec![root.join(".gitignore")]);
        assert_eq!(
            pending.take(root).unwrap(),
            vec![root.join(".gitignore").to_string_lossy()]
        );
        pending.extend(root, &mut matcher, vec![root.join("dist/out")]);
        let paths = pending.take(root).unwrap();
        assert_eq!(paths, vec![root.join("dist").to_string_lossy()]);
        pending.extend(root, &mut matcher, vec![root.join("target/out")]);
        let paths = pending.take(root).unwrap();
        assert_eq!(paths, vec![root.join("target/out").to_string_lossy()]);
        std::fs::remove_file(root.join(".gitignore")).unwrap();
        pending.extend(
            root,
            &mut matcher,
            vec![root.join(".gitignore"), root.join("dist/out")],
        );
        assert!(pending.paths.contains_key(&root.join("dist/out")));
    }

    #[test]
    fn watcher_info_exclude_and_root_negations_apply_to_directories_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::create_dir(root.join("dist")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "dist/\nkeep/\n.env\n").unwrap();
        std::fs::write(root.join(".gitignore"), "!keep/\n").unwrap();
        let matcher = ignore_matcher(root);
        assert_eq!(
            coalesce_path(root, &matcher, root.join("dist")).0,
            root.join("dist")
        );
        assert_eq!(
            coalesce_path(root, &matcher, root.join("dist/sub/a")).0,
            root.join("dist")
        );
        assert_eq!(
            coalesce_path(root, &matcher, root.join("keep/a")).0,
            root.join("keep/a")
        );
        assert_eq!(
            coalesce_path(root, &matcher, root.join(".env")).0,
            root.join(".env")
        );
        assert_eq!(
            coalesce_path(root, &matcher, root.to_owned()).0,
            root.to_owned()
        );
    }

    #[test]
    fn watcher_capacity_and_overflow_invalidate_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut matcher = ignore_matcher(root);
        let mut pending = PendingChanges::default();
        pending.extend(
            root,
            &mut matcher,
            (0..=CAPACITY).map(|i| root.join(i.to_string())).collect(),
        );
        assert!(pending.paths.is_empty());
        assert_eq!(
            pending.take(root),
            Some(vec![root.to_string_lossy().into_owned()])
        );
        pending.invalidate_root();
        assert_eq!(
            pending.take(root),
            Some(vec![root.to_string_lossy().into_owned()])
        );
    }

    #[test]
    fn watcher_delivers_ignored_directory_and_file_events() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n.env\n").unwrap();
        let (send, receive) = mpsc::channel();
        let _watcher = build_watcher(&root, move |paths| {
            let _ = send.send(paths);
        })
        .unwrap();
        std::fs::write(root.join("target/out"), "build").unwrap();
        std::fs::write(root.join(".env"), "updated").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_directory = false;
        let mut saw_file = false;
        while !saw_directory || !saw_file {
            let paths = receive
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            assert!(!paths.contains(&root.join("target/out").to_string_lossy().into_owned()));
            saw_directory |= paths.contains(&root.join("target").to_string_lossy().into_owned());
            if paths.contains(&root.join(".env").to_string_lossy().into_owned()) {
                saw_file = true;
            }
        }
    }
}
