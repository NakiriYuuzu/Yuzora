use std::path::Path;
pub use yuzora_host::watcher::{build_watcher, is_ignored_path, WatcherHandle};
#[derive(Default)]
pub struct WatcherState(pub std::sync::Mutex<WatcherSlot>);

#[tauri::command]
pub fn stop_watch(state: tauri::State<'_, WatcherState>) -> Result<(), String> {
    let (_, previous) = state.0.lock().map_err(|e| e.to_string())?.begin();
    drop(previous);
    Ok(())
}

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
pub async fn start_watch<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, WatcherState>,
    path: String,
) -> Result<(), String> {
    use tauri::Emitter;
    let (generation, stale) = state.0.lock().map_err(|e| e.to_string())?.begin();
    drop(stale);
    let workspace_root = path.clone();
    let event_root = workspace_root.clone();
    let event_app = app.clone();
    let handle = tauri::async_runtime::spawn_blocking(move || {
        build_watcher(Path::new(&path), move |paths| {
            let _ = event_app.emit(
                "fs:external-change",
                ExternalChangeEvent {
                    workspace_root: event_root.clone(),
                    paths,
                },
            );
        })
    })
    .await
    .map_err(|e| format!("watcher blocking task failed: {e}"))??;
    install_and_reconcile(&app, &state, generation, handle, workspace_root)
}

fn install_and_reconcile<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &WatcherState,
    generation: u64,
    handle: WatcherHandle,
    workspace_root: String,
) -> Result<(), String> {
    use tauri::Emitter;
    let installed = state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .install(generation, handle);
    if installed {
        // Listing and open buffers may predate registration. Reconcile once
        // after admission; synchronous listeners must run outside the slot lock.
        app.emit(
            "fs:external-change",
            ExternalChangeEvent {
                workspace_root: workspace_root.clone(),
                paths: vec![workspace_root],
            },
        )
        .map_err(|error| format!("watcher reconciliation failed: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(unix)]
    #[test]
    fn canonical_alias_watch_survives_write_replace_write_replace() {
        use std::io::Write;
        use tauri::{Listener, Manager};
        let fixture = tempfile::tempdir().unwrap();
        let real = fixture.path().join("real");
        let alias = fixture.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let root = crate::fs_service::canonicalize_workspace(alias.to_str().unwrap()).unwrap();
        let file = Path::new(&root).join("document.txt");
        let app = tauri::test::mock_app();
        app.manage(WatcherState::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let listener = app.listen("fs:external-change", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            let _ = sender.send(payload);
        });
        tauri::async_runtime::block_on(start_watch(
            app.handle().clone(),
            app.state::<WatcherState>(),
            root.clone(),
        ))
        .unwrap();
        for (index, content) in ["write one", "replace two", "write three", "replace four"]
            .into_iter()
            .enumerate()
        {
            // Bounded OS smoke: drain currently available batches, but OS delay
            // and debounce mean a later event cannot be attributed to one stage.
            let quiet_deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                assert!(
                    std::time::Instant::now() < quiet_deadline,
                    "owned watcher did not settle"
                );
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok(_) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(error) => panic!("watch listener closed: {error}"),
                }
            }
            if index % 2 == 0 {
                std::fs::write(&file, content).unwrap();
            } else {
                let mut temporary = tempfile::NamedTempFile::new_in(&root).unwrap();
                temporary.write_all(content.as_bytes()).unwrap();
                temporary.persist(&file).unwrap();
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let event = receiver
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .unwrap();
                assert_eq!(event["workspaceRoot"], root);
                if event["paths"].as_array().unwrap().iter().any(|path| {
                    path == &serde_json::json!(root)
                        || path == &serde_json::json!(file.to_str().unwrap())
                }) {
                    break;
                }
            }
            assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
        }
        stop_watch(app.state::<WatcherState>()).unwrap();
        app.unlisten(listener);
        assert!(std::fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn rejected_and_stopped_completions_do_not_reconcile_even_after_returning_to_same_root() {
        use tauri::{Listener, Manager};
        let fixture = tempfile::tempdir().unwrap();
        let root =
            crate::fs_service::canonicalize_workspace(fixture.path().to_str().unwrap()).unwrap();
        let app = tauri::test::mock_app();
        app.manage(WatcherState::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let listener_app = app.handle().clone();
        let listener = app.listen("fs:external-change", move |event| {
            let unlocked = listener_app.state::<WatcherState>().0.try_lock().is_ok();
            let _ = sender.send((event.payload().to_string(), unlocked));
        });
        let state = app.state::<WatcherState>();
        let (old_a, _) = state.0.lock().unwrap().begin();
        let (_b, _) = state.0.lock().unwrap().begin();
        let (current_a, _) = state.0.lock().unwrap().begin();
        // Real handles, quiet callbacks: only the completion path under test
        // publishes to this app, independent of OS prelude timing.
        let old = build_watcher(Path::new(&root), |_| {}).unwrap();
        install_and_reconcile(app.handle(), &state, old_a, old, root.clone()).unwrap();
        assert!(receiver.try_recv().is_err());
        let current = build_watcher(Path::new(&root), |_| {}).unwrap();
        install_and_reconcile(app.handle(), &state, current_a, current, root.clone()).unwrap();
        let (payload, unlocked) = receiver.try_recv().unwrap();
        assert!(unlocked);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).unwrap()["paths"],
            serde_json::json!([root])
        );
        assert!(receiver.try_recv().is_err());
        let (pending, previous) = state.0.lock().unwrap().begin();
        drop(previous);
        stop_watch(app.state::<WatcherState>()).unwrap();
        let stopped = build_watcher(Path::new(&root), |_| {}).unwrap();
        install_and_reconcile(app.handle(), &state, pending, stopped, root.clone()).unwrap();
        assert!(receiver.try_recv().is_err());
        assert!(tauri::async_runtime::block_on(start_watch(
            app.handle().clone(),
            app.state::<WatcherState>(),
            format!("{root}/absent")
        ))
        .is_err());
        assert!(receiver.try_recv().is_err());
        stop_watch(app.state::<WatcherState>()).unwrap();
        app.unlisten(listener);
    }

    #[test]
    fn native_watch_completion_reconciles_changes_since_initial_snapshot() {
        use tauri::{Listener, Manager};
        let fixture = tempfile::tempdir().unwrap();
        let root =
            crate::fs_service::canonicalize_workspace(fixture.path().to_str().unwrap()).unwrap();
        let file = Path::new(&root).join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let initial_listing = crate::fs_service::list_dir_entries(Path::new(&root)).unwrap();
        let initial_buffer = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, "v2").unwrap();
        std::fs::write(Path::new(&root).join("new.txt"), "new").unwrap();
        let app = tauri::test::mock_app();
        app.manage(WatcherState::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let listener_app = app.handle().clone();
        let listener = app.listen("fs:external-change", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            let unlocked = listener_app.state::<WatcherState>().0.try_lock().is_ok();
            let _ = sender.send((payload, unlocked));
        });
        tauri::async_runtime::block_on(start_watch(
            app.handle().clone(),
            app.state::<WatcherState>(),
            root.clone(),
        ))
        .unwrap();
        // Observe at completion; later OS batches are not the required initial
        // reconciliation and cannot repair the registration gap reliably.
        let events: Vec<_> = receiver.try_iter().collect();
        stop_watch(app.state::<WatcherState>()).unwrap();
        app.unlisten(listener);
        eprintln!(
            "completion events={}, old listing={}, old buffer={initial_buffer}",
            events.len(),
            initial_listing.len()
        );
        assert_eq!(initial_buffer, "v1");
        assert_eq!(initial_listing.len(), 1);
        let (_, unlocked) = events
            .iter()
            .find(|(payload, _)| {
                payload["workspaceRoot"] == root && payload["paths"] == serde_json::json!([root])
            })
            .expect("successful registration must invalidate the initial root snapshot");
        assert!(
            *unlocked,
            "synchronous listeners must run outside the watcher lock"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");
        assert_eq!(
            crate::fs_service::list_dir_entries(Path::new(&root))
                .unwrap()
                .len(),
            2
        );
    }

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
        assert_eq!(v.as_object().unwrap().len(), 2);
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
