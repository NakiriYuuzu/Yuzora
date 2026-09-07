pub use yuzora_host::git_watch::{build_git_watcher, build_repository_watcher, GitWatcher};

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
