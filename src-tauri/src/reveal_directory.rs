use std::path::{Path, PathBuf};
use tauri_plugin_opener::OpenerExt;

use crate::path_capability::WorkspacePathRegistry;

fn workspace_directory(
    workspaces: &WorkspacePathRegistry,
    workspace_id: &str,
    path: &str,
) -> Result<PathBuf, String> {
    let root = workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    let directory = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !directory.starts_with(Path::new(&root)) || !directory.is_dir() {
        return Err("Only directories in the current workspace can be opened".into());
    }
    // Canonicalization may race a workspace switch. Never accept a revoked token.
    workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    Ok(directory)
}

/// Open a directory, never an arbitrary file, through the existing workspace grant.
/// The frontend's generic opener allowlist remains unchanged.
#[tauri::command(async)]
pub fn open_workspace_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_id: String,
    path: String,
) -> Result<(), String> {
    let directory = workspace_directory(&state.0, &workspace_id, &path)?;
    let directory = directory
        .to_str()
        .ok_or("Directory path is not valid UTF-8")?;
    app.opener()
        .open_path(directory, None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_current_workspace_directories_and_rejects_files_outside_and_revoked_tokens() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let child = root.path().join("中文 project");
        std::fs::create_dir(&child).unwrap();
        let file = root.path().join("script.sh");
        std::fs::write(&file, "echo test").unwrap();
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(root.path()).unwrap();
        assert!(workspace_directory(&registry, &id, root.path().to_str().unwrap()).is_ok());
        assert!(workspace_directory(&registry, &id, child.to_str().unwrap()).is_ok());
        assert!(workspace_directory(&registry, &id, file.to_str().unwrap()).is_err());
        assert!(workspace_directory(&registry, &id, outside.path().to_str().unwrap()).is_err());
        registry.activate(outside.path()).unwrap();
        assert!(workspace_directory(&registry, &id, child.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_directory_symlink_escaping_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(root.path()).unwrap();
        assert!(workspace_directory(&registry, &id, link.to_str().unwrap()).is_err());
    }
}
