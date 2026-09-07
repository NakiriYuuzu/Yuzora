pub use yuzora_host::workspace_trust::*;

#[tauri::command]
pub async fn workspace_trust_status(
    trust: tauri::State<'_, WorkspaceTrustState>,
    processes: tauri::State<'_, crate::process_service::ProcessState>,
    path: String,
) -> Result<WorkspaceTrustStatusDto, String> {
    let trust = trust.inner().clone();
    let manager = processes.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let status = trust.0.status(&path)?;
        if status.state == "invalid" && status.reason.as_deref() == Some("identityMismatch") {
            let _ = manager.stop_workspace(&path);
            if let Some(canonical) = &status.canonical_path {
                let _ = manager.stop_workspace(canonical);
            }
        }
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
pub async fn workspace_trust_challenge(
    trust: tauri::State<'_, WorkspaceTrustState>,
    path: String,
) -> Result<WorkspaceTrustChallengeDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.issue_workspace_challenge(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn workspace_trust_execution_challenge(
    trust: tauri::State<'_, WorkspaceTrustState>,
    path: String,
    command: String,
) -> Result<WorkspaceExecutionChallengeDto, String> {
    let trust = trust.inner().clone();
    tauri::async_runtime::spawn_blocking(move || trust.0.issue_execution_challenge(&path, &command))
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
    processes: tauri::State<'_, crate::process_service::ProcessState>,
    canonical_path: String,
) -> Result<Vec<TrustedWorkspaceDto>, String> {
    let trust = trust.inner().clone();
    let manager = processes.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let stopped = trust.0.revoke(&canonical_path)?;
        for path in stopped {
            let _ = manager.stop_workspace(&path);
        }
        trust.0.list()
    })
    .await
    .map_err(|error| error.to_string())?
}
