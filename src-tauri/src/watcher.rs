use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::Path;
use std::time::Duration;

pub type WatcherHandle = Debouncer<notify::RecommendedWatcher>;

pub fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(".git") | Some("node_modules")))
}

pub fn build_watcher(
    root: &Path,
    on_change: impl Fn(Vec<String>) + Send + 'static,
) -> Result<WatcherHandle, String> {
    let mut debouncer = new_debouncer(
        Duration::from_millis(300),
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<String> = events
                    .into_iter()
                    .filter(|e| !is_ignored_path(&e.path))
                    .map(|e| e.path.to_string_lossy().into_owned())
                    .collect();
                if !paths.is_empty() {
                    on_change(paths);
                }
            }
        },
    )
    .map_err(|e| format!("watcher init failed: {e}"))?;
    debouncer
        .watcher()
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| format!("watch failed: {e}"))?;
    Ok(debouncer)
}

pub struct WatcherState(pub std::sync::Mutex<Option<WatcherHandle>>);

#[tauri::command]
pub fn start_watch(
    app: tauri::AppHandle,
    state: tauri::State<'_, WatcherState>,
    path: String,
) -> Result<(), String> {
    use tauri::Emitter;
    let handle = build_watcher(Path::new(&path), move |paths| {
        let _ = app.emit("fs:external-change", paths);
    })?;
    *state.0.lock().map_err(|e| e.to_string())? = Some(handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn watcher_emits_debounced_paths_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let _debouncer = build_watcher(tmp.path(), move |paths| {
            let _ = tx.send(paths);
        })
        .unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        let paths = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(paths.iter().any(|p| p.ends_with("a.txt")));
    }

    #[test]
    fn watcher_filters_git_and_node_modules_events() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let _d = build_watcher(tmp.path(), move |paths| {
            let _ = tx.send(paths);
        })
        .unwrap();
        std::fs::write(tmp.path().join(".git/index"), "x").unwrap();
        std::fs::write(tmp.path().join("node_modules/pkg/a.js"), "x").unwrap();
        std::fs::write(tmp.path().join("real.txt"), "x").unwrap();
        // macOS FSEvents 會把 .git／node_modules 的寫入合併成 watch root 的目錄
        // 事件，且 real.txt 不保證落在第一個 debounce batch——契約是「被過濾的
        // 路徑永不出現」與「real.txt 終會送達」，逐 batch 驗證直到看到為止。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_real = false;
        while !saw_real {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let paths = rx
                .recv_timeout(remaining)
                .expect("real.txt change was never delivered");
            assert!(
                paths.iter().all(|p| !is_ignored_path(Path::new(p))),
                "got: {paths:?}"
            );
            saw_real = paths.iter().any(|p| p.ends_with("real.txt"));
        }
    }

    #[test]
    fn is_ignored_path_matches_components_only() {
        use std::path::Path;
        assert!(is_ignored_path(Path::new("/w/.git/index")));
        assert!(is_ignored_path(Path::new("/w/node_modules/a/b.js")));
        assert!(!is_ignored_path(Path::new("/w/src/git_helpers.rs")));
        assert!(!is_ignored_path(Path::new("/w/my.gitignore.txt")));
    }
}
