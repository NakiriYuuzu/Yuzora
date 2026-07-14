use tauri::Manager;

/// Adds the opened workspace directory (recursively) to the asset-protocol
/// scope so `convertFileSrc` URLs inside it resolve. The static scope in
/// tauri.conf.json stays empty: nothing on disk is reachable through the
/// `asset:` scheme until the user actually opens a workspace, and grants are
/// only ever whole opened-workspace trees (plan constraint C4). Grants
/// accumulate for the app session; there is no revoke path (plan Q1).
#[tauri::command]
pub fn allow_workspace_asset_scope(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let canonical =
        std::fs::canonicalize(&path).map_err(|e| format!("canonicalize {path}: {e}"))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }
    app.asset_protocol_scope()
        .allow_directory(&canonical, true)
        .map_err(|e| e.to_string())
}
