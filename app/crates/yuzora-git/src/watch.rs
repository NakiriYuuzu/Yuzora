//! Watching a local repo: the work tree and `.git`, debounced 300 ms
//! (IntelliJ's `UpdateRequestsQueue`), turned into dirty scopes.
//!
//! - `index`, `HEAD`, refs, `packed-refs`, merge or rebase state, `config`:
//!   everything is dirty and the refs moved (the log reloads);
//! - a `.gitignore`: everything is dirty;
//! - a work tree path: that path is dirty, unless it is inside an ignored
//!   directory (a build writing into `target/` refreshes nothing);
//! - objects, logs, locks and the fsmonitor daemon's files: nothing.
//!
//! A machine's repo isn't watched; its views refresh on a timer. A repo no
//! view has called or waited on for [`crate::repo::IDLE`] stops being
//! watched until the next call.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

use crate::repo::Repo;
use crate::status::{Scope, covers};

/// IntelliJ's status delay.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
const TICK: Duration = Duration::from_secs(1);

/// A running watch; dropping it stops the thread.
pub struct Watch {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Change {
    pub all: bool,
    pub refs: bool,
    pub paths: Vec<String>,
}

/// Sorts one path into a change.
pub fn classify(change: &mut Change, path: &Path, root: &Path, git_dirs: &[PathBuf], ignored: &[String]) {
    for g in git_dirs {
        if let Ok(rel) = path.strip_prefix(g) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if git_noise(&rel) {
                return;
            }
            change.all = true;
            if rel != "index" {
                change.refs = true;
            }
            return;
        }
    }
    let Ok(rel) = path.strip_prefix(root) else { return };
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() {
        change.all = true;
        return;
    }
    if rel == ".git" || rel.starts_with(".git/") {
        // A gitdir file (worktree, submodule) or a git dir not in git_dirs.
        return;
    }
    if ignored.iter().any(|d| covers(d, &rel)) {
        return;
    }
    if rel == ".gitignore" || rel.ends_with("/.gitignore") {
        change.all = true;
        return;
    }
    change.paths.push(rel);
}

/// What under `.git` changes without changing status or refs.
fn git_noise(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    matches!(first, "objects" | "logs" | "hooks" | "fsmonitor--daemon" | "yuzora-rebase" | "lfs" | "worktrees" | "modules" | "COMMIT_EDITMSG" | "FETCH_HEAD" | "gc.pid" | "gc.log" | "description" | "sequencer")
        || rel.ends_with(".lock")
        || rel.contains("/.tmp")
        || rel.is_empty()
}

/// Starts watching a local repo; the watch lives in `repo.watch`.
pub fn start(repo: &Arc<Repo>) {
    let (tx, rx) = mpsc::channel();
    let Ok(mut watcher) = notify::recommended_watcher(tx) else { return };
    let root = PathBuf::from(&repo.place.root);
    if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
        return;
    }
    let mut git_dirs = vec![PathBuf::from(&repo.git_dir)];
    if repo.common_dir != repo.git_dir {
        git_dirs.push(PathBuf::from(&repo.common_dir));
    }
    for g in &git_dirs {
        if !g.starts_with(&root) {
            let _ = watcher.watch(g, RecursiveMode::Recursive);
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let weak = Arc::downgrade(repo);
    let stopped = stop.clone();
    let spawned = std::thread::Builder::new().name("yuzora-git-watch".into()).spawn(move || run(weak, rx, root, git_dirs, stopped));
    if spawned.is_ok() {
        *repo.watch.lock().unwrap_or_else(|e| e.into_inner()) = Some(Watch { _watcher: watcher, stop });
    }
}

fn run(repo: Weak<Repo>, rx: mpsc::Receiver<notify::Result<notify::Event>>, root: PathBuf, git_dirs: Vec<PathBuf>, stop: Arc<AtomicBool>) {
    let mut change = Change::default();
    let mut due: Option<Instant> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let wait = due.map_or(TICK, |d| d.saturating_duration_since(Instant::now()).min(TICK));
        match rx.recv_timeout(wait) {
            Ok(Ok(event)) => {
                let Some(repo) = repo.upgrade() else { return };
                let ignored = repo.status.lock().unwrap_or_else(|e| e.into_inner()).ignored_dirs.clone();
                if event.need_rescan() {
                    change.all = true;
                    change.refs = true;
                }
                for p in &event.paths {
                    classify(&mut change, p, &root, &git_dirs, &ignored);
                }
                if (change.all || !change.paths.is_empty()) && due.is_none() {
                    due = Some(Instant::now() + DEBOUNCE);
                }
            }
            Ok(Err(_)) => {
                change.all = true;
                due.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let Some(repo) = repo.upgrade() else { return };
        repo.expire_waiters();
        // No view uses it: let go until the next call.
        if repo.release_if_idle() {
            return;
        }
        if due.is_some_and(|d| Instant::now() >= d) {
            due = None;
            let c = std::mem::take(&mut change);
            if c.refs {
                repo.invalidate_refs();
            } else if c.all {
                repo.invalidate(Scope::All);
            } else if !c.paths.is_empty() {
                repo.invalidate(Scope::Paths(c.paths));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_sort_into_scopes() {
        let root = Path::new("/r");
        let git = vec![PathBuf::from("/r/.git")];
        let ignored = vec!["target".to_string()];
        let mut c = Change::default();
        classify(&mut c, Path::new("/r/src/a.rs"), root, &git, &ignored);
        classify(&mut c, Path::new("/r/target/debug/x"), root, &git, &ignored);
        classify(&mut c, Path::new("/r/.git/objects/ab/cd"), root, &git, &ignored);
        classify(&mut c, Path::new("/r/.git/index.lock"), root, &git, &ignored);
        assert_eq!(c, Change { all: false, refs: false, paths: vec!["src/a.rs".into()] });
        classify(&mut c, Path::new("/r/.git/index"), root, &git, &ignored);
        assert!(c.all && !c.refs);
        classify(&mut c, Path::new("/r/.git/refs/heads/main"), root, &git, &ignored);
        assert!(c.refs);
        let mut g = Change::default();
        classify(&mut g, Path::new("/r/sub/.gitignore"), root, &git, &ignored);
        assert!(g.all && !g.refs);
    }

    #[test]
    fn a_change_on_disk_reaches_the_repo() {
        let s = crate::repo::testing::scratch();
        s.commit("a.txt", "1\n", "first");
        crate::status::refresh(&s.repo).unwrap();
        start(&s.repo);
        // FSEvents can report the folder's creation late; let it settle.
        std::thread::sleep(Duration::from_millis(400));
        let g = s.repo.generation();
        let waiting = s.repo.wait(g);
        s.write("a.txt", "2\n");
        let got = crate::repo::tests::futures_lite_block(waiting);
        assert!(got > g, "woken with {got}");
        crate::status::refresh(&s.repo).unwrap();
        let v = crate::status::snapshot(&s.repo);
        assert_eq!(v["counts"]["unstaged"], 1);
    }
}
