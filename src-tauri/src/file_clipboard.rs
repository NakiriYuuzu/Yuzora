//! File copy / cut / paste commands for the local workspace, including the OS
//! clipboard file list (Finder / Explorer interop). Clipboard paths never come
//! from JS: the backend reads the list itself and imports it through the shared
//! no-follow engine in `yuzora_host::file_transfer`.
//!
//! Finder / Explorer drag-and-drop follows the same rule: the renderer may only
//! name paths that the native `Drop` event of the main webview just delivered.
//!
//! The real OS clipboard is touched only by the thin `os_*` wrappers; the logic
//! around them takes injected path lists so tests never clobber the clipboard.

use crate::fs_service::run_blocking;
use crate::path_capability::{PinnedDir, SafeRelativePath, WorkspacePathState};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Webview, WebviewEvent};

#[cfg(any(target_os = "macos", windows))]
fn clipboard_error(error: arboard::Error) -> String {
    match error {
        arboard::Error::ClipboardNotSupported => "clipboard-files-unsupported".into(),
        other => other.to_string(),
    }
}

/// Absolute file paths currently on the OS clipboard (empty when none).
#[cfg(any(target_os = "macos", windows))]
pub(crate) fn os_read_file_list() -> Result<Vec<PathBuf>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(clipboard_error)?;
    match clipboard.get().file_list() {
        Ok(paths) => Ok(paths),
        Err(arboard::Error::ContentNotAvailable) => Ok(Vec::new()),
        Err(error) => Err(clipboard_error(error)),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn os_read_file_list() -> Result<Vec<PathBuf>, String> {
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
    yuzora_host::file_transfer::import_into(root, canonical, &clipboard, target_dir)
}

/// A drop older than this no longer authorises an import.
const DROP_TTL: Duration = Duration::from_secs(120);
/// Two quick drops must not invalidate each other's in-flight import.
const KEPT_DROPS: usize = 3;

/// Path sets of the latest native file drops onto the main webview, oldest first.
pub(crate) struct RecentDrop(Mutex<Vec<(Instant, Vec<PathBuf>)>>);

impl RecentDrop {
    fn record(&self, paths: Vec<PathBuf>, now: Instant) {
        if let Ok(mut drops) = self.0.lock() {
            drops.push((now, paths));
            let excess = drops.len().saturating_sub(KEPT_DROPS);
            drops.drain(..excess);
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(drops: Vec<Vec<PathBuf>>, now: Instant) -> Self {
        Self(Mutex::new(
            drops.into_iter().map(|paths| (now, paths)).collect(),
        ))
    }

    /// Requested paths, accepted only when one fresh drop delivered every one
    /// of them. A successful take consumes that drop, so it cannot be replayed.
    pub(crate) fn take(&self, requested: &[String], now: Instant) -> Result<Vec<PathBuf>, String> {
        let mut drops = self.0.lock().map_err(|_| "dropped-paths-lock")?;
        let mut stale = false;
        for index in (0..drops.len()).rev() {
            let (at, dropped) = &drops[index];
            let found: Option<Vec<PathBuf>> = requested
                .iter()
                .map(|path| {
                    dropped
                        .iter()
                        .find(|known| known.as_os_str() == path.as_str())
                        .cloned()
                })
                .collect();
            let Some(found) = found.filter(|found| !found.is_empty()) else {
                continue;
            };
            if now.saturating_duration_since(*at) > DROP_TTL {
                stale = true;
                continue;
            }
            drops.remove(index);
            return Ok(found);
        }
        Err(if stale {
            "dropped-paths-stale"
        } else {
            "dropped-paths-unknown"
        }
        .into())
    }
}

pub(crate) static RECENT_DROP: RecentDrop = RecentDrop(Mutex::new(Vec::new()));

/// Tauri emits the drop to JS before running native listeners, so the renderer's
/// request can briefly beat `record_native_drop`; retry only that "unknown" case.
pub(crate) async fn retry_until_recorded<T>(
    mut attempt: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    for _ in 0..4 {
        match attempt() {
            Err(error) if error == "dropped-paths-unknown" => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            other => return other,
        }
    }
    attempt()
}

/// Only the app's own window may authorise imports.
fn should_record(label: &str) -> bool {
    label == "main"
}

/// Webview event hook: remember what the user just dropped on the main window.
/// Other webviews (the Preview page) never authorise workspace imports.
pub fn record_native_drop(webview: &Webview, event: &WebviewEvent) {
    if !should_record(webview.label()) {
        return;
    }
    if let WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
        RECENT_DROP.record(paths.clone(), Instant::now());
    }
}

#[tauri::command]
pub async fn fs_import_dropped_paths(
    state: tauri::State<'_, WorkspacePathState>,
    workspace_capability_id: String,
    paths: Vec<String>,
    target_dir: String,
) -> Result<Vec<String>, String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    let dropped = retry_until_recorded(|| RECENT_DROP.take(&paths, Instant::now())).await?;
    run_blocking(move || paste_paths(&canonical, &root, dropped, &target_dir)).await
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

    fn drop_of(paths: &[&Path]) -> (RecentDrop, Instant) {
        let now = Instant::now();
        let recent = RecentDrop(Mutex::new(Vec::new()));
        recent.record(paths.iter().map(PathBuf::from).collect(), now);
        (recent, now)
    }

    #[test]
    fn dropped_import_accepts_only_paths_from_a_fresh_drop() {
        let external = tempfile::tempdir().unwrap();
        let note = external.path().join("note.txt");
        fs::write(&note, b"note").unwrap();
        let (recent, now) = drop_of(&[&note]);
        let note_str = note.to_str().unwrap().to_string();
        assert_eq!(
            recent.take(&["/etc/passwd".into()], now).unwrap_err(),
            "dropped-paths-unknown"
        );
        // One unknown path poisons the whole request.
        assert_eq!(
            recent
                .take(&[note_str.clone(), "/etc/hosts".into()], now)
                .unwrap_err(),
            "dropped-paths-unknown"
        );
        // A path that merely resolves to a dropped one was not itself dropped.
        assert_eq!(
            recent.take(&[format!("{note_str}/../x")], now).unwrap_err(),
            "dropped-paths-unknown"
        );
        assert_eq!(recent.take(&[], now).unwrap_err(), "dropped-paths-unknown");
        assert_eq!(
            recent
                .take(
                    std::slice::from_ref(&note_str),
                    now + DROP_TTL + Duration::from_secs(1)
                )
                .unwrap_err(),
            "dropped-paths-stale"
        );
        // Failed requests consume nothing.
        assert_eq!(
            recent
                .take(std::slice::from_ref(&note_str), now + DROP_TTL)
                .unwrap(),
            vec![note.clone()]
        );
    }

    #[test]
    fn a_successful_take_consumes_the_drop() {
        let (recent, now) = drop_of(&[Path::new("/d/a.txt"), Path::new("/d/b.txt")]);
        assert_eq!(
            recent.take(&["/d/b.txt".into()], now).unwrap(),
            vec![PathBuf::from("/d/b.txt")]
        );
        // No replay or double-invoke of the same drop, not even for its other paths.
        for path in ["/d/a.txt", "/d/b.txt"] {
            assert_eq!(
                recent.take(&[path.into()], now).unwrap_err(),
                "dropped-paths-unknown"
            );
        }
    }

    #[test]
    fn the_last_few_drops_stay_independently_authorised() {
        let now = Instant::now();
        let recent = RecentDrop(Mutex::new(Vec::new()));
        for name in ["/d/1", "/d/2", "/d/3", "/d/4"] {
            recent.record(vec![PathBuf::from(name)], now);
        }
        // The oldest of four fell off; the other three can be taken in any order.
        assert_eq!(
            recent.take(&["/d/1".into()], now).unwrap_err(),
            "dropped-paths-unknown"
        );
        for name in ["/d/3", "/d/2", "/d/4"] {
            assert!(recent.take(&[name.into()], now).is_ok(), "{name}");
        }
    }

    #[test]
    fn only_the_main_webview_records_drops() {
        assert!(should_record("main"));
        for label in ["preview", "preview-1", "Main", "", "main "] {
            assert!(!should_record(label), "{label:?}");
        }
    }

    #[test]
    fn retry_waits_out_a_drop_that_is_recorded_late() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let calls = std::cell::Cell::new(0);
        let late = rt.block_on(retry_until_recorded(|| {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err("dropped-paths-unknown".to_string())
            } else {
                Ok(calls.get())
            }
        }));
        assert_eq!(late, Ok(3));
        // Other errors return immediately and are not retried.
        let calls = std::cell::Cell::new(0);
        let stale: Result<(), String> = rt.block_on(retry_until_recorded(|| {
            calls.set(calls.get() + 1);
            Err("dropped-paths-stale".into())
        }));
        assert_eq!(stale.unwrap_err(), "dropped-paths-stale");
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn dropped_import_without_any_drop_is_unknown() {
        let recent = RecentDrop(Mutex::new(Vec::new()));
        assert_eq!(
            recent.take(&["/a".into()], Instant::now()).unwrap_err(),
            "dropped-paths-unknown"
        );
    }

    #[test]
    fn dropped_import_renames_conflicts_and_refuses_the_workspace_parent() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        fs::create_dir_all(project.join("dest")).unwrap();
        fs::write(project.join("dest/note.txt"), b"old").unwrap();
        let external = tempfile::tempdir().unwrap();
        let note = external.path().join("note.txt");
        fs::write(&note, b"new").unwrap();
        let workspace = project.canonicalize().unwrap();
        let root = PinnedDir::open_dir(&workspace).unwrap();
        let (recent, now) = drop_of(&[&note, parent.path()]);

        let mut both = recent
            .take(
                &[
                    note.to_str().unwrap().into(),
                    parent.path().to_str().unwrap().into(),
                ],
                now,
            )
            .unwrap();
        let dropped = vec![both.remove(0)];
        let out = paste_paths(&workspace, &root, dropped, "dest").unwrap();
        assert_eq!(out.len(), 1);
        assert_ne!(out[0], "dest/note.txt");
        assert_eq!(fs::read(project.join("dest/note.txt")).unwrap(), b"old");
        assert_eq!(fs::read(workspace.join(&out[0])).unwrap(), b"new");

        assert_eq!(
            paste_paths(&workspace, &root, both, "").unwrap_err(),
            "copy-into-itself"
        );
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
