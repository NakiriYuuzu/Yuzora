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

#[derive(Default)]
pub struct WatcherState(pub std::sync::Mutex<WatcherSlot>);

/// 單一 active watcher 的槽位。async 化後 `start_watch` 的 `build_watcher`
/// 在 blocking pool 真並發（前端 fire-and-forget），完成順序不再等於呼叫
/// 順序——generation 讓「後呼叫者勝」而非「後完成者勝」：`begin()` 使所有
/// in-flight 舊請求過期，`install()` 只安裝仍為最新世代的 handle。
#[derive(Default)]
pub struct WatcherSlot {
    generation: u64,
    handle: Option<WatcherHandle>,
}

impl WatcherSlot {
    /// 登記一次新的 watch 請求：回傳其世代並**取走舊 handle**（呼叫端於鎖外
    /// drop——#57 T3「先停舊再掛新」，切換 gap 內不得再有舊 workspace 事件）；
    /// 先前尚未完成的請求即刻過期。
    pub fn begin(&mut self) -> (u64, Option<WatcherHandle>) {
        self.generation = self.generation.wrapping_add(1);
        (self.generation, self.handle.take())
    }

    /// 僅當 `generation` 仍為最新才安裝並回傳 true；過期的 handle 直接
    /// drop（棄置），不覆寫較新的 watcher。
    pub fn install(&mut self, generation: u64, handle: WatcherHandle) -> bool {
        if generation != self.generation {
            return false;
        }
        self.handle = Some(handle);
        true
    }
}

/// `fs:external-change` 事件 payload（#57 T3）：帶上 watcher 所屬的 workspace
/// 路徑，前端三個 listener（ExternalChangeBridge／GitBridge／
/// ExternalChangeResolver）比對 live workspacePath 後才處理，杜絕切換 gap 內
/// 舊 workspace 事件串場。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalChangeEvent {
    pub workspace_root: String,
    pub paths: Vec<String>,
}

/// T2（#56）：同步 command 在 main thread 執行（Tauri 2），notify 遞迴掛載在
/// Linux inotify 會走訪整棵樹 → async ＋ `tauri::async_runtime::spawn_blocking`
/// （tokio 缺 `rt` feature，不可用 `tokio::task::spawn_blocking`）。handle 建好
/// `.await` 回來後才 lock State 存回——不跨 `.await` 持 `MutexGuard`。
/// review fix（#56）：async 化讓兩次快速切換的 build 並發競速，慢的舊
/// workspace 可能後到覆寫新的——await 前先 `begin()` 取世代、await 後
/// `install()` 比對，過期 handle 直接丟棄（比照 search_service 的
/// generation 取消樣板）。
/// T3（#57）：`begin()` 同時取走舊 handle、於鎖外先 drop——先停舊再掛新，
/// 切換 gap 內不再產生舊 workspace 的 fs 事件（listener 端另有 workspaceRoot
/// 過濾雙保險）。
#[tauri::command]
pub async fn start_watch(
    app: tauri::AppHandle,
    state: tauri::State<'_, WatcherState>,
    path: String,
) -> Result<(), String> {
    use tauri::Emitter;
    let (generation, stale) = state.0.lock().map_err(|e| e.to_string())?.begin();
    drop(stale);
    let workspace_root = path.clone();
    let handle = tauri::async_runtime::spawn_blocking(move || {
        build_watcher(Path::new(&path), move |paths| {
            let _ = app.emit(
                "fs:external-change",
                ExternalChangeEvent {
                    workspace_root: workspace_root.clone(),
                    paths,
                },
            );
        })
    })
    .await
    .map_err(|e| format!("watcher blocking task failed: {e}"))??;
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .install(generation, handle);
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

    /// A→B 快速切換、A 的 build 較慢完成（後到）：過期的 A 不得覆寫 B，
    /// B 的 watcher 必須仍然有效（後呼叫者勝，而非後完成者勝）。
    #[test]
    fn stale_watch_build_finishing_late_does_not_override_newer_watcher() {
        let mut slot = WatcherSlot::default();
        let (gen_a, _) = slot.begin();
        let (gen_b, _) = slot.begin();

        let tmp_b = tempfile::tempdir().unwrap();
        let (tx_b, rx_b) = std::sync::mpsc::channel::<Vec<String>>();
        let root_event_tx = tx_b.clone();
        let handle_b = build_watcher(tmp_b.path(), move |paths| {
            let _ = tx_b.send(paths);
        })
        .unwrap();
        assert!(slot.install(gen_b, handle_b), "latest generation installs");

        let tmp_a = tempfile::tempdir().unwrap();
        let (tx_a, rx_a) = std::sync::mpsc::channel::<Vec<String>>();
        let handle_a = build_watcher(tmp_a.path(), move |paths| {
            let _ = tx_a.send(paths);
        })
        .unwrap();
        assert!(
            !slot.install(gen_a, handle_a),
            "stale generation must be rejected"
        );

        // macOS FSEvents 可能先把 watch root 當成獨立 batch 送達；注入同樣的
        // 合法 prelude，確保測試驗證「b.txt 終會送達」而不是依賴第一批內容。
        root_event_tx
            .send(vec![tmp_b.path().to_string_lossy().into_owned()])
            .unwrap();
        std::fs::write(tmp_b.path().join("b.txt"), "x").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let paths = rx_b
                .recv_timeout(remaining)
                .expect("b.txt change was never delivered");
            if paths.iter().any(|p| p.ends_with("b.txt")) {
                break;
            }
        }

        // A 的 handle 已在 install 拒絕時 drop——它的事件永遠不會送達。
        std::fs::write(tmp_a.path().join("a.txt"), "x").unwrap();
        assert!(rx_a.recv_timeout(Duration::from_millis(600)).is_err());
    }

    /// A→B 快速切換、完成順序恰與呼叫順序相同：A 先完成也不得安裝——
    /// `begin(B)` 已使其過期；最終仍以 B 為準。
    #[test]
    fn earlier_generation_never_installs_even_if_it_completes_first() {
        let mut slot = WatcherSlot::default();
        let (gen_a, _) = slot.begin();
        let (gen_b, _) = slot.begin();

        let tmp_a = tempfile::tempdir().unwrap();
        let handle_a = build_watcher(tmp_a.path(), |_| {}).unwrap();
        assert!(!slot.install(gen_a, handle_a));

        let tmp_b = tempfile::tempdir().unwrap();
        let handle_b = build_watcher(tmp_b.path(), |_| {}).unwrap();
        assert!(slot.install(gen_b, handle_b));
    }

    /// #57 T3 AC4：`begin()` 必須取走舊 handle（先停舊再掛新）——drop 後舊
    /// watcher 的目錄變更不得再送達；空槽位 begin 則取不到東西。
    #[test]
    fn begin_takes_the_old_handle_so_switching_stops_it_before_the_new_watch() {
        let mut slot = WatcherSlot::default();
        let (gen_old, none) = slot.begin();
        assert!(none.is_none(), "empty slot has no handle to take");

        let tmp_old = tempfile::tempdir().unwrap();
        let (tx_old, rx_old) = std::sync::mpsc::channel::<Vec<String>>();
        let handle_old = build_watcher(tmp_old.path(), move |paths| {
            let _ = tx_old.send(paths);
        })
        .unwrap();
        assert!(slot.install(gen_old, handle_old));

        let (_gen_new, taken) = slot.begin();
        assert!(
            taken.is_some(),
            "begin must hand back the previous handle for the caller to drop"
        );
        drop(taken);

        std::fs::write(tmp_old.path().join("old.txt"), "x").unwrap();
        assert!(
            rx_old.recv_timeout(Duration::from_millis(600)).is_err(),
            "old watcher must be stopped before the new one is built"
        );
    }

    /// #57 T3：fs:external-change 事件 payload 帶 workspaceRoot＋paths
    /// （前端 listener 過濾契約）。
    #[test]
    fn external_change_event_serializes_workspace_root_and_paths() {
        let v = serde_json::to_value(ExternalChangeEvent {
            workspace_root: "/w".to_string(),
            paths: vec!["/w/a.txt".to_string()],
        })
        .unwrap();
        assert_eq!(v["workspaceRoot"], "/w");
        assert_eq!(v["paths"][0], "/w/a.txt");
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
