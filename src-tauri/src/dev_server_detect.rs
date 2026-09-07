pub use yuzora_host::dev_server_detect::*;

#[tauri::command]
pub async fn dev_server_detect(
    workspace: String,
    extra_ports: Option<Vec<u16>>,
) -> Result<DevServerDetect, String> {
    tauri::async_runtime::spawn_blocking(move || {
        detect_workspace(&workspace, extra_ports.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}
