pub mod agent_process;
pub mod agent_terminal;
pub mod askpass;
pub mod dev_server_detect;
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
pub mod process_kill;
pub mod process_service;
pub mod pty_service;
pub mod search_service;
pub mod watcher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let sink = logging::LogSink::new(logging::default_log_dir());
    sink.cleanup();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(std::sync::Mutex::new(sink))
        .manage(watcher::WatcherState(std::sync::Mutex::new(None)))
        .manage(git_service::GitServiceState(std::sync::Mutex::new(None)))
        .manage(git_watch::GitWatchState(std::sync::Mutex::new(None)))
        .manage(search_service::SearchState(std::sync::Arc::new(
            std::sync::atomic::AtomicU64::new(0),
        )))
        .setup(|app| {
            use tauri::{Emitter, Manager};
            let handle = app.handle().clone();
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fs_service::open_workspace,
            fs_service::list_dir,
            fs_service::open_file,
            fs_service::save_file,
            logging::log_event,
            logging::log_query,
            logging::log_sources,
            logging::log_export,
            watcher::start_watch,
            search_service::search_workspace,
            git_service::git_detect,
            git_service::git_status_cmd,
            git_service::git_stage,
            git_service::git_unstage,
            git_service::git_discard,
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
            agent_terminal::agent_terminal_create,
            agent_terminal::agent_terminal_output,
            agent_terminal::agent_terminal_wait_for_exit,
            agent_terminal::agent_terminal_kill,
            agent_terminal::agent_terminal_release
        ])
        .build(tauri::generate_context!())
        .expect("error while running yuzora application")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                use tauri::Manager;
                app.state::<pty_service::PtyState>().0.kill_all();
                app.state::<process_service::ProcessState>().0.kill_all();
                app.state::<agent_process::AgentProcessState>().0.kill_all();
                app.state::<agent_terminal::AgentTerminalState>()
                    .0
                    .kill_all();
            }
        })
}
