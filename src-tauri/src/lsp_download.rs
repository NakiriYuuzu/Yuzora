pub use yuzora_host::lsp_download::*;

#[tauri::command]
pub async fn lsp_install_server(
    app: tauri::AppHandle,
    workspace: Option<String>,
    language: String,
) -> Result<crate::lsp_service::LspServerInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter;
        let info =
            yuzora_host::lsp_download::install(workspace.as_deref(), &language, &|progress| {
                let _ = app.emit("lsp:install-progress", progress);
            })?;
        let _ = app.emit("lsp:server-status", info.clone());
        Ok(info)
    })
    .await
    .map_err(|e| format!("LSP install task failed: {e}"))?
}
