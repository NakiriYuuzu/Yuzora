pub mod agent_process;
pub mod agent_terminal;
pub mod askpass;
pub mod asset_scope;
pub mod db_connection_actor;
pub mod db_credentials;
pub mod db_profiles;
pub mod db_result_session;
pub mod db_service;
pub mod dev_server_detect;
pub mod env_path;
pub mod file_content;
pub mod fs_service;
pub mod git_log;
pub mod git_service;
pub mod git_status;
pub mod git_watch;
pub mod logging;
pub mod lsp_adapters;
pub mod lsp_config;
pub mod lsp_download;
pub mod lsp_service;
pub mod perf_service;
pub mod preview_server;
pub mod preview_webview;
pub mod process_kill;
pub mod process_service;
pub mod pty_service;
pub mod search_service;
pub mod ssh_service;
pub mod watcher;
pub mod workspace_path_index;

const DATABASE_SHUTDOWN_THREAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

#[derive(Debug)]
enum DatabaseShutdownThreadError {
    SpawnFailed(String),
    RuntimeBuildFailed(String),
    TimedOut,
    WorkerDisconnected,
    WorkerPanicked,
}

impl std::fmt::Display for DatabaseShutdownThreadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(error) => write!(formatter, "worker spawn failed: {error}"),
            Self::RuntimeBuildFailed(error) => {
                write!(formatter, "shutdown runtime build failed: {error}")
            }
            Self::TimedOut => formatter.write_str("shutdown worker timed out"),
            Self::WorkerDisconnected => formatter.write_str("shutdown worker disconnected"),
            Self::WorkerPanicked => formatter.write_str("shutdown worker panicked"),
        }
    }
}

fn shutdown_database_runtime_on_dedicated_thread(
    state: db_profiles::DatabaseProfileState,
) -> Result<db_profiles::DatabaseRuntimeShutdownReport, DatabaseShutdownThreadError> {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("database-shutdown".to_string())
        .spawn(move || {
            let result =
                tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .map_err(|error| error.to_string())
                    .map(|runtime| {
                        runtime.block_on(state.shutdown_database_runtime(
                            db_service::DatabaseShutdownTimeouts::default(),
                        ))
                    });
            let _ = result_tx.send(result);
        })
        .map_err(|error| DatabaseShutdownThreadError::SpawnFailed(error.to_string()))?;

    match result_rx.recv_timeout(DATABASE_SHUTDOWN_THREAD_TIMEOUT) {
        Ok(Ok(report)) => {
            worker
                .join()
                .map_err(|_| DatabaseShutdownThreadError::WorkerPanicked)?;
            Ok(report)
        }
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(DatabaseShutdownThreadError::RuntimeBuildFailed(error))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Dropping the JoinHandle detaches the bounded database cleanup
            // attempt so a pathological worker cannot hold process exit open.
            drop(worker);
            Err(DatabaseShutdownThreadError::TimedOut)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let panicked = worker.join().is_err();
            Err(if panicked {
                DatabaseShutdownThreadError::WorkerPanicked
            } else {
                DatabaseShutdownThreadError::WorkerDisconnected
            })
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 先套用持久化的 log level 門檻，確保啟動最早期的寫入（含 env_path）也受門檻約束
    logging::apply_persisted_log_level();
    // GUI（Finder/Dock）啟動的 .app 只拿到 launchd 預設 PATH，撈不到 homebrew/nvm/
    // bun。必須在任何 tauri::Builder／執行緒 spawn 之前跑，set_var("PATH") 才安全。
    env_path::fix_gui_path();
    // 啟動期清理一次；其後由共享 sink 在每日首筆寫入時觸發
    logging::cleanup_global();
    let database_shutdown_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // Remember window size/position/maximized across launches. VISIBLE is
        // excluded on purpose: the window starts hidden (anti-FOUC) and the
        // frontend shows it on the first themed frame — restoring a persisted
        // visible=true here would defeat that.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .manage(watcher::WatcherState(std::sync::Mutex::new(None)))
        .manage(git_service::GitServiceState(std::sync::Mutex::new(None)))
        .manage(git_watch::GitWatchState(std::sync::Mutex::new(None)))
        .manage(search_service::SearchState(std::sync::Arc::new(
            std::sync::atomic::AtomicU64::new(0),
        )))
        .manage(db_service::DbState::default())
        .manage(db_result_session::ResultSessionState::default())
        .manage(perf_service::PerfState(std::sync::Mutex::new(
            sysinfo::System::new(),
        )))
        .manage(preview_server::PreviewServerState(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )))
        .manage(preview_webview::PreviewWebviewState(std::sync::Mutex::new(
            None,
        )))
        .setup(|app| {
            use tauri::{Emitter, Manager};
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
            let handle = app.handle().clone();
            let profile_repository_path =
                app.path().app_data_dir()?.join("database-profiles-v1.json");
            let database_state = app.state::<db_service::DbState>().inner().clone();
            let result_sessions = app
                .state::<db_result_session::ResultSessionState>()
                .inner()
                .clone();
            app.manage(db_profiles::DatabaseProfileState::production(
                profile_repository_path,
                database_state,
                result_sessions,
            ));
            // Windows stub / bind 失敗回 Err → 降級：manage AskpassState(None)，四個 remote
            // command 經 env_for 取空 env（git 仍可用系統 credential helper）。務必 manage，
            // 否則 State<AskpassState> 取用時 panic。
            match askpass::AskpassServer::start(move |req| {
                let _ = handle.emit("git:askpass-request", req);
            }) {
                Ok(server) => {
                    app.manage(askpass::AskpassState(Some(server)));
                }
                Err(e) => {
                    eprintln!("askpass disabled: {e}");
                    app.manage(askpass::AskpassState(None));
                }
            }
            app.manage(lsp_service::LspState(std::sync::Arc::new(
                lsp_service::LspManager::new(app.handle().clone()),
            )));
            app.manage(pty_service::PtyState(std::sync::Arc::new(
                pty_service::PtyManager::new(app.handle().clone()),
            )));
            app.manage(process_service::ProcessState(std::sync::Arc::new(
                process_service::ProcessManager::new(app.handle().clone()),
            )));
            app.manage(agent_process::AgentProcessState(std::sync::Arc::new(
                agent_process::AgentManager::new(),
            )));
            app.manage(agent_terminal::AgentTerminalState(std::sync::Arc::new(
                agent_terminal::AgentTerminalManager::new(),
            )));
            app.manage(ssh_service::SshState(std::sync::Arc::new(
                ssh_service::SshManager::new(app.handle().clone()),
            )));
            // The main window starts hidden (tauri.conf `visible: false`) so the
            // native chrome never flashes the OS theme before the persisted
            // preference applies; the frontend shows it on its first themed
            // frame (AppShell). Failsafe: if the frontend wedges before that,
            // force-show after 3s so the app can never boot into an invisible
            // window.
            let show_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Some(window) = show_handle.get_webview_window("main") {
                    // Err defaults to "not visible": show() is idempotent, so
                    // when the state is unreadable, erring toward a redundant
                    // show can never leave the window invisible.
                    if !window.is_visible().unwrap_or(false) {
                        let _ = window.show();
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            asset_scope::allow_workspace_asset_scope,
            fs_service::open_workspace,
            fs_service::list_dir,
            fs_service::open_file,
            fs_service::save_file,
            fs_service::fs_create_file,
            fs_service::fs_create_dir,
            fs_service::fs_rename,
            fs_service::fs_delete,
            fs_service::read_file_base64,
            logging::log_event,
            logging::log_query,
            logging::log_sources,
            logging::log_export,
            logging::get_log_level,
            logging::set_log_level,
            watcher::start_watch,
            workspace_path_index::workspace_path_index,
            search_service::search_workspace,
            db_service::db_list_tables,
            db_service::db_table_columns,
            db_service::db_query_run,
            db_service::db_query_cancel,
            db_service::db_result_page,
            db_service::db_result_session_release,
            db_profiles::db_profile_list,
            db_profiles::db_profile_import_legacy,
            db_profiles::db_profile_create,
            db_profiles::db_profile_update,
            db_profiles::db_profile_remove_credential,
            db_profiles::db_profile_forget,
            db_profiles::db_profile_recover,
            db_profiles::db_profile_open,
            db_profiles::db_profile_disconnect,
            db_profiles::db_test_connection,
            perf_service::perf_snapshot,
            preview_server::preview_serve,
            preview_server::preview_stop_all,
            preview_webview::preview_open_url,
            preview_webview::preview_set_bounds,
            preview_webview::preview_set_visible,
            preview_webview::preview_close,
            preview_webview::preview_back,
            preview_webview::preview_forward,
            preview_webview::preview_reload,
            git_service::git_detect,
            git_service::git_status_cmd,
            git_service::git_stage,
            git_service::git_unstage,
            git_service::git_discard,
            git_service::git_rollback_paths,
            git_service::git_commit_cmd,
            git_service::git_branches,
            git_service::git_create_branch,
            git_service::git_checkout,
            git_service::git_cherry_pick,
            git_service::git_fetch_cmd,
            git_service::git_pull_cmd,
            git_service::git_push_cmd,
            git_service::git_remote_probe,
            git_service::git_diff_content,
            git_service::git_conflict_abort,
            git_service::git_conflict_continue,
            git_log::git_log_page,
            git_log::git_commit_detail,
            git_log::git_log_authors,
            git_log::git_file_at_rev,
            askpass::askpass_respond,
            lsp_service::lsp_start,
            lsp_service::lsp_send,
            lsp_service::lsp_stop_workspace,
            lsp_service::lsp_status,
            lsp_service::lsp_detect_server,
            lsp_service::lsp_config_get,
            lsp_service::lsp_config_set_server,
            lsp_service::lsp_config_stale,
            lsp_service::lsp_config_clear_stale,
            lsp_service::lsp_set_trace,
            lsp_download::lsp_install_server,
            pty_service::pty_open,
            pty_service::pty_write,
            pty_service::pty_resize,
            pty_service::pty_close,
            pty_service::pty_close_workspace,
            dev_server_detect::dev_server_detect,
            process_service::dev_server_start,
            process_service::dev_server_stop,
            process_service::dev_server_stop_workspace,
            agent_process::agent_spawn,
            agent_process::agent_write,
            agent_process::agent_kill,
            agent_process::agent_list,
            agent_process::agent_set_trace,
            agent_process::agent_stderr_tail,
            agent_terminal::agent_terminal_create,
            agent_terminal::agent_terminal_output,
            agent_terminal::agent_terminal_wait_for_exit,
            agent_terminal::agent_terminal_kill,
            agent_terminal::agent_terminal_release,
            ssh_service::ssh_connect,
            ssh_service::ssh_open_shell,
            ssh_service::ssh_write,
            ssh_service::ssh_resize,
            ssh_service::ssh_disconnect,
            ssh_service::sftp_list_dir,
            ssh_service::sftp_mkdir,
            ssh_service::sftp_rename,
            ssh_service::sftp_remove,
            ssh_service::sftp_upload,
            ssh_service::sftp_download
        ])
        .build(tauri::generate_context!())
        .expect("error while running yuzora application")
        .run(move |app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                use tauri::Manager;
                if !database_shutdown_started.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let database_profiles = app
                        .state::<db_profiles::DatabaseProfileState>()
                        .inner()
                        .clone();
                    match shutdown_database_runtime_on_dedicated_thread(database_profiles) {
                        Ok(report) if report.has_failures() => {
                            eprintln!("database shutdown completed with failures: {report:?}");
                        }
                        Ok(report) => {
                            eprintln!("database shutdown completed: {report:?}");
                        }
                        Err(error) => {
                            eprintln!("database shutdown did not complete: {error}");
                        }
                    }
                }
                app.state::<pty_service::PtyState>().0.kill_all();
                app.state::<process_service::ProcessState>().0.kill_all();
                app.state::<agent_process::AgentProcessState>().0.kill_all();
                app.state::<agent_terminal::AgentTerminalState>()
                    .0
                    .kill_all();
                app.state::<ssh_service::SshState>().0.kill_all();
                app.state::<preview_server::PreviewServerState>().stop_all();
            }
        })
}

#[cfg(test)]
mod command_inventory_tests {
    #[test]
    fn app_exit_inventory_runs_bounded_database_shutdown_once() {
        let source = include_str!("lib.rs");
        for required in [
            "tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit",
            "database_shutdown_started.swap(true, std::sync::atomic::Ordering::AcqRel)",
            "shutdown_database_runtime_on_dedicated_thread(database_profiles)",
            "recv_timeout(DATABASE_SHUTDOWN_THREAD_TIMEOUT)",
        ] {
            assert!(
                source.contains(required),
                "missing bounded app-exit database shutdown seam: {required}"
            );
        }
    }

    #[test]
    fn p6_query_commands_replace_the_legacy_unsplit_query_command() {
        let inventory_source = include_str!("lib.rs");
        let service_source = include_str!("db_service.rs");
        let legacy = ["db_service::db_", "query,"].concat();
        assert!(
            !inventory_source.lines().any(|line| line.trim() == legacy),
            "legacy db_query must not be reachable through Tauri invoke"
        );
        let legacy_wrapper = ["fn db_", "query("].concat();
        assert!(
            !service_source
                .lines()
                .any(|line| line.contains(&legacy_wrapper)),
            "legacy db_query Tauri wrapper must not exist in production source"
        );
        for suffix in [
            "query_run,",
            "query_cancel,",
            "result_page,",
            "result_session_release,",
        ] {
            let required = format!("db_service::db_{suffix}");
            assert!(
                inventory_source.lines().any(|line| line.trim() == required),
                "missing P6 command registration: {required}"
            );
        }
    }
}
