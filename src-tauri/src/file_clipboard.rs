//! File copy / cut / paste commands for the local workspace, including the OS
//! clipboard file list (Finder / Explorer interop). Clipboard paths never come
//! from JS: the backend reads the list itself and imports it through the shared
//! no-follow engine in `yuzora_host::file_transfer`.
//!
//! The real OS clipboard is touched only by the thin `os_*` wrappers; the logic
//! around them takes injected path lists so tests never clobber the clipboard.

use crate::fs_service::run_blocking;
use crate::path_capability::{PinnedDir, SafeRelativePath, WorkspacePathState};
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", windows))]
fn clipboard_error(error: arboard::Error) -> String {
    match error {
        arboard::Error::ClipboardNotSupported => "clipboard-files-unsupported".into(),
        other => other.to_string(),
    }
}

/// Absolute file paths currently on the OS clipboard (empty when none).
#[cfg(any(target_os = "macos", windows))]
fn os_read_file_list() -> Result<Vec<PathBuf>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(clipboard_error)?;
    match clipboard.get().file_list() {
        Ok(paths) => Ok(paths),
        Err(arboard::Error::ContentNotAvailable) => Ok(Vec::new()),
        Err(error) => Err(clipboard_error(error)),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_read_file_list() -> Result<Vec<PathBuf>, String> {
    Err("clipboard-files-unsupported".into())
}

#[cfg(any(target_os = "macos", windows))]
fn os_write_file_list(paths: &[PathBuf]) -> Result<(), String> {
    arboard::Clipboard::new()
        .map_err(clipboard_error)?
        .set()
        .file_list(paths)
        .map_err(clipboard_error)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_write_file_list(_paths: &[PathBuf]) -> Result<(), String> {
    Err("clipboard-files-unsupported".into())
}

/// Map workspace-relative paths to absolute ones. Each must exist and, after
/// symlink resolution, stay inside `canonical`. The returned path keeps the
/// unresolved spelling so Finder shows the entry's own name.
fn resolve_workspace_paths(canonical: &Path, paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut resolved = Vec::with_capacity(paths.len());
    for path in paths {
        SafeRelativePath::parse(path)?;
        let absolute = canonical.join(path);
        let real = absolute.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(canonical) {
            return Err("path-escape".into());
        }
        resolved.push(absolute);
    }
    Ok(resolved)
}

fn utf8_paths(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .filter_map(|path| path.into_os_string().into_string().ok())
        .collect()
}

fn paste_paths(
    canonical: &Path,
    root: &PinnedDir,
    clipboard: Vec<PathBuf>,
    target_dir: &str,
) -> Result<Vec<String>, String> {
    if clipboard.is_empty() {
        return Ok(Vec::new());
    }
    // A folder that contains the workspace (e.g. the home folder pasted into a
    // project) would copy the copy into itself; refuse before walking it.
    for source in &clipboard {
        if std::fs::canonicalize(source).is_ok_and(|source| canonical.starts_with(&source)) {
            return Err("copy-into-itself".into());
        }
    }
    yuzora_host::file_transfer::import_into(root, &clipboard, target_dir)
}

#[tauri::command]
pub async fn fs_copy_paths(
    state: tauri::State<'_, WorkspacePathState>,
    workspace_capability_id: String,
    sources: Vec<String>,
    target_dir: String,
) -> Result<Vec<String>, String> {
    let (_, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || yuzora_host::file_transfer::copy_into(&root, &sources, &target_dir)).await
}

#[tauri::command]
pub async fn fs_move_paths(
    state: tauri::State<'_, WorkspacePathState>,
    workspace_capability_id: String,
    sources: Vec<String>,
    target_dir: String,
) -> Result<Vec<String>, String> {
    let (_, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || yuzora_host::file_transfer::move_into(&root, &sources, &target_dir)).await
}

#[tauri::command]
pub async fn fs_paste_clipboard_files(
    state: tauri::State<'_, WorkspacePathState>,
    workspace_capability_id: String,
    target_dir: String,
) -> Result<Vec<String>, String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || paste_paths(&canonical, &root, os_read_file_list()?, &target_dir)).await
}

#[tauri::command]
pub async fn clipboard_write_workspace_files(
    state: tauri::State<'_, WorkspacePathState>,
    workspace_capability_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let (canonical, _) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || os_write_file_list(&resolve_workspace_paths(&canonical, &paths)?)).await
}

#[tauri::command]
pub async fn clipboard_read_file_list() -> Result<Vec<String>, String> {
    run_blocking(|| os_read_file_list().map(utf8_paths)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn canonical(tmp: &tempfile::TempDir) -> PathBuf {
        fs::canonicalize(tmp.path()).unwrap()
    }

    #[test]
    fn resolves_relative_paths_to_absolute_and_requires_existence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(&tmp);
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/a.txt"), b"a").unwrap();
        let resolved = resolve_workspace_paths(&root, &["dir/a.txt".into(), "dir".into()]).unwrap();
        assert_eq!(resolved, vec![root.join("dir/a.txt"), root.join("dir")]);
        assert!(resolve_workspace_paths(&root, &["missing".into()]).is_err());
    }

    #[test]
    fn rejects_unsafe_and_escaping_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(&tmp);
        for bad in ["", "../x", "/etc/passwd", "a/../b", "a\\b"] {
            assert!(
                resolve_workspace_paths(&root, &[bad.to_string()]).is_err(),
                "{bad}"
            );
        }
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("s.txt"), b"s").unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("out")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("s.txt"), root.join("s-link")).unwrap();
            fs::write(root.join("in.txt"), b"in").unwrap();
            std::os::unix::fs::symlink(root.join("in.txt"), root.join("in-link")).unwrap();
            assert_eq!(
                resolve_workspace_paths(&root, &["out".into()]).unwrap_err(),
                "path-escape"
            );
            assert_eq!(
                resolve_workspace_paths(&root, &["out/s.txt".into()]).unwrap_err(),
                "path-escape"
            );
            assert_eq!(
                resolve_workspace_paths(&root, &["s-link".into()]).unwrap_err(),
                "path-escape"
            );
            // A link that stays inside the workspace keeps its own spelling.
            assert_eq!(
                resolve_workspace_paths(&root, &["in-link".into()]).unwrap(),
                vec![root.join("in-link")]
            );
        }
    }

    #[test]
    fn utf8_filter_keeps_valid_paths() {
        let paths = vec![PathBuf::from("/a/b"), PathBuf::from("/c")];
        assert_eq!(
            utf8_paths(paths),
            vec!["/a/b".to_string(), "/c".to_string()]
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bad = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff, 0xfe]));
            assert!(utf8_paths(vec![bad, PathBuf::from("/ok")]) == vec!["/ok".to_string()]);
        }
    }

    #[test]
    fn paste_imports_injected_clipboard_paths_and_ignores_an_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("note.txt"), b"note").unwrap();
        fs::create_dir(tmp.path().join("dest")).unwrap();
        let workspace = canonical(&tmp);
        let root = PinnedDir::open_dir(&workspace).unwrap();
        assert!(paste_paths(&workspace, &root, Vec::new(), "dest")
            .unwrap()
            .is_empty());
        assert!(fs::read_dir(tmp.path().join("dest"))
            .unwrap()
            .next()
            .is_none());
        let out = paste_paths(
            &workspace,
            &root,
            vec![external.path().join("note.txt")],
            "dest",
        )
        .unwrap();
        assert_eq!(out, vec!["dest/note.txt".to_string()]);
        assert_eq!(fs::read(tmp.path().join("dest/note.txt")).unwrap(), b"note");
    }

    #[test]
    fn paste_refuses_a_folder_that_contains_the_workspace() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(parent.path().join("big.txt"), b"x").unwrap();
        let workspace = project.canonicalize().unwrap();
        let root = PinnedDir::open_dir(&workspace).unwrap();
        assert_eq!(
            paste_paths(&workspace, &root, vec![parent.path().to_path_buf()], "").unwrap_err(),
            "copy-into-itself"
        );
        assert!(fs::read_dir(&project).unwrap().next().is_none());
    }
}
