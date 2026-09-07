use yuzora_host::workspace_path_index::{build_workspace_path_index, WORKSPACE_PATH_INDEX_CAP};
pub use yuzora_host::workspace_path_index::{WorkspacePathIndexEntry, WorkspacePathIndexResult};

#[tauri::command]
pub async fn workspace_path_index(workspace: String) -> Result<WorkspacePathIndexResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canonical_root = std::fs::canonicalize(&workspace)
            .map_err(|error| format!("invalid workspace path: {error}"))?;
        if !canonical_root.is_dir() {
            return Err("workspace path is not a directory".to_string());
        }
        build_workspace_path_index(canonical_root, WORKSPACE_PATH_INDEX_CAP)
    })
    .await
    .map_err(|error| format!("workspace path index worker failed: {error}"))?
}
