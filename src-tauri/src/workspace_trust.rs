pub use yuzora_host::workspace_trust::*;

#[tauri::command]
pub async fn workspace_trust_status(
    trust: tauri::State<'_, WorkspaceTrustState>,
    path: String,
) -> Result<WorkspaceTrustStatusDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let status = trust.0.status(&path)?;
        Ok(status)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_list(
    trust: tauri::State<'_, WorkspaceTrustState>,
) -> Result<Vec<TrustedWorkspaceDto>, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.list())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_grant(
    trust: tauri::State<'_, WorkspaceTrustState>,
    challenge_id: String,
) -> Result<WorkspaceTrustStatusDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.grant(&challenge_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_revoke(
    trust: tauri::State<'_, WorkspaceTrustState>,
    canonical_path: String,
) -> Result<Vec<TrustedWorkspaceDto>, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        trust.0.revoke(&canonical_path)?;
        trust.0.list()
    })
    .await
    .map_err(|error| error.to_string())?
}
