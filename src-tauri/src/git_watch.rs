// M2 Task 7: .git state watcher → emit git:state-changed

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::Path;
use std::time::Duration;

pub struct GitWatchState(pub std::sync::Mutex<Option<Debouncer<notify::RecommendedWatcher>>>);

/// 監看 `<root>/.git`（NonRecursive）＋ `<root>/.git/refs`（Recursive，若存在）。
/// 500ms debounce；任何事件直接 on_change()（.git 內就是我們要的，不過濾）。
pub fn build_git_watcher(
    git_dir: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Result<Debouncer<notify::RecommendedWatcher>, String> {
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: DebounceEventResult| {
            // .git 內任何事件都是我們要的，不看事件內容。
            if res.is_ok() {
                on_change();
            }
        },
    )
    .map_err(|e| format!("git watcher init failed: {e}"))?;
    debouncer
        .watcher()
        .watch(git_dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("git watch failed: {e}"))?;
    let refs_dir = git_dir.join("refs");
    if refs_dir.is_dir() {
        debouncer
            .watcher()
            .watch(&refs_dir, RecursiveMode::Recursive)
            .map_err(|e| format!("git refs watch failed: {e}"))?;
    }
    Ok(debouncer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_watcher_fires_on_head_and_refs_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/refs/heads")).unwrap();
        std::fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let _d = build_git_watcher(&tmp.path().join(".git"), move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("HEAD change not detected");
        std::fs::write(tmp.path().join(".git/refs/heads/dev"), "abc\n").unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("refs change not detected");
    }
}
