use crate::{logging, lsp_adapters, lsp_config};
use std::sync::{Arc, Mutex};
pub use yuzora_host::lsp_service::*;
pub struct LspState(pub Arc<LspManager>);
pub fn new_manager(app: tauri::AppHandle) -> LspManager {
    let sink = Mutex::new(logging::LogSink::new(logging::default_log_dir()));
    let log: LogFn = Box::new(move |ev| {
        if let Ok(mut s) = sink.lock() {
            s.write(ev);
        }
    });
    let emit: EmitFn = Box::new(move |info| {
        use tauri::Emitter;
        let _ = app.emit("lsp:server-status", info);
    });
    LspManager::with_parts(logging::default_log_dir(), 500, log, emit)
}

/// T2（#56）：`lsp_detect_server`／`lsp_start`／`lsp_send` 原為同步 command，
/// 在 main thread 執行（Tauri 2）——全 PATH stat、spawn 子行程、寫 server stdin
/// （pipe buffer 滿時硬卡，且每次鍵入都走）都會凍住 UI event loop → async ＋
/// `tauri::async_runtime::spawn_blocking`（tokio 缺 `rt` feature，不可用
/// `tokio::task::spawn_blocking`）。State 是 `Arc<LspManager>`，clone 後 move
/// 進 closure，無跨 `.await` 持鎖問題。其餘 lsp commands（µs 級）維持 sync。
async fn run_blocking<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| format!("lsp blocking task failed: {e}"))?
}

#[tauri::command]
pub async fn lsp_detect_server(
    state: tauri::State<'_, LspState>,
    workspace: Option<String>,
    language: String,
) -> Result<LspServerInfo, String> {
    let manager = state.0.clone();
    run_blocking(move || {
        let active = workspace.as_deref().and_then(|workspace| {
            manager
                .status(workspace)
                .into_iter()
                .find(|info| info.language == language)
        });
        let cfg = lsp_config::load_from(&lsp_config::config_path());
        detect_server_info_from(&cfg, workspace.as_deref(), &language, active, which)
    })
    .await
}

#[tauri::command]
pub async fn lsp_start(
    state: tauri::State<'_, LspState>,
    workspace: String,
    language: String,
    on_message: tauri::ipc::Channel<String>,
) -> Result<LspServerInfo, String> {
    let manager = state.0.clone();
    run_blocking(move || {
        let cfg = lsp_config::load_from(&lsp_config::config_path());
        let ws_canonical =
            lsp_config::canonicalize(&workspace).unwrap_or_else(|| workspace.clone());
        let id = lsp_config::resolve_server(&cfg, &ws_canonical, &language)
            .or_else(|| lsp_adapters::adapters_for(&language).map(|a| a.default_id.to_string()))
            .ok_or_else(|| format!("no LSP adapter for language {language}"))?;
        let adapter = lsp_adapters::adapter(&language, &id)
            .ok_or_else(|| format!("unknown server {id} for {language}"))?;
        let resolved = ResolvedServer {
            server_id: id,
            command: adapter.command.to_string(),
            args: adapter.args.iter().map(|s| s.to_string()).collect(),
        };
        let channel = on_message;
        let on_message: OnMessage = Arc::new(move |body| {
            let _ = channel.send(body);
        });
        Ok(manager.start(&workspace, &language, resolved, on_message))
    })
    .await
}

#[tauri::command]
pub async fn lsp_send(
    state: tauri::State<'_, LspState>,
    workspace: String,
    language: String,
    message: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    run_blocking(move || manager.send(&workspace, &language, message)).await
}

#[tauri::command(async)]
pub fn lsp_stop_workspace(
    state: tauri::State<'_, LspState>,
    workspace: String,
) -> Result<(), String> {
    state.0.stop_workspace(&workspace);
    Ok(())
}

#[tauri::command(async)]
pub fn lsp_status(
    state: tauri::State<'_, LspState>,
    workspace: String,
) -> Result<Vec<LspServerInfo>, String> {
    Ok(state.0.status(&workspace))
}

#[tauri::command(async)]
pub fn lsp_config_get() -> Result<lsp_config::LspConfig, String> {
    Ok(lsp_config::load_from(&lsp_config::config_path()))
}

#[tauri::command(async)]
pub fn lsp_config_set_server(
    state: tauri::State<'_, LspState>,
    workspace: Option<String>,
    language: String,
    server_id: String,
) -> Result<lsp_config::LspConfig, String> {
    let path = lsp_config::config_path();
    let mut ws_canonical = None;
    let cfg = lsp_config::update(&path, |cfg| {
        lsp_adapters::adapter(&language, &server_id).ok_or("lsp-server-unsupported")?;
        ws_canonical = apply_set_server(cfg, workspace.as_deref(), &language, &server_id);
        Ok(())
    })?;
    // Running server for this (ws,lang) is now stale — stop it; frontend restarts.
    state.0.stop_matching(ws_canonical.as_deref(), &language);
    Ok(cfg)
}

#[tauri::command(async)]
pub fn lsp_config_stale() -> Result<Vec<String>, String> {
    Ok(lsp_config::stale_workspaces(&lsp_config::load_from(
        &lsp_config::config_path(),
    )))
}

#[tauri::command(async)]
pub fn lsp_config_clear_stale(workspace: String) -> Result<lsp_config::LspConfig, String> {
    let path = lsp_config::config_path();
    lsp_config::update(&path, |cfg| {
        lsp_config::clear_workspace(cfg, &workspace);
        Ok(())
    })
}

#[tauri::command(async)]
pub fn lsp_set_trace(state: tauri::State<'_, LspState>, enabled: bool) -> Result<(), String> {
    state.0.set_trace(enabled)
}
