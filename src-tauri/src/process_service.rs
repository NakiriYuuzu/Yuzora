use std::sync::Arc;
pub use yuzora_host::process_service::{DevServerInfo, DevServerStatus, OnOutput, ProcessManager};
pub struct ProcessState(pub Arc<ProcessManager>);

pub fn create_manager(app: tauri::AppHandle) -> ProcessManager {
    ProcessManager::with_seams(
        Box::new(crate::logging::write_global),
        Arc::new(move |info| {
            use tauri::Emitter;
            let _ = app.emit("dev-server:status", info);
        }),
    )
}

#[tauri::command]
pub async fn dev_server_start(
    state: tauri::State<'_, ProcessState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    workspace: String,
    command: String,
    port: Option<u16>,
    challenge_id: String,
    on_output: tauri::ipc::Channel<String>,
) -> Result<DevServerInfo, String> {
    let manager = state.0.clone();
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_output;
        let on_output: OnOutput = Arc::new(move |line| {
            let _ = channel.send(line);
        });
        manager.start_authorized(
            &trust.0,
            &workspace,
            &command,
            &challenge_id,
            port,
            on_output,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dev_server_stop(
    state: tauri::State<'_, ProcessState>,
    workspace: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop(&workspace))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dev_server_stop_workspace(
    state: tauri::State<'_, ProcessState>,
    workspace: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop_workspace(&workspace))
        .await
        .map_err(|e| e.to_string())?
}
