use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES, HARD_CAP_BYTES,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileNodeKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub kind: FileNodeKind,
}

fn file_node_kind(file_type: std::fs::FileType) -> FileNodeKind {
    match crate::path_capability::node_kind_from_file_type(file_type) {
        crate::path_capability::NodeKind::File => FileNodeKind::File,
        crate::path_capability::NodeKind::Directory => FileNodeKind::Directory,
        crate::path_capability::NodeKind::Symlink => FileNodeKind::Symlink,
        crate::path_capability::NodeKind::Other => FileNodeKind::Other,
    }
}

fn sort_file_nodes(nodes: &mut [FileNode]) {
    nodes.sort_by_cached_key(|node| (std::cmp::Reverse(node.is_dir), node.name.to_lowercase()));
}

pub fn list_dir_entries(dir: &Path) -> Result<Vec<FileNode>, String> {
    let mut nodes: Vec<FileNode> = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir failed: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_str()?.to_string();
            let path = path.to_str()?.to_string();
            if name.contains('\0') || path.contains('\0') {
                return None;
            }
            let kind = file_node_kind(entry.file_type().ok()?);
            Some(FileNode {
                name,
                path,
                is_dir: kind == FileNodeKind::Directory,
                kind,
            })
        })
        .collect();
    sort_file_nodes(&mut nodes);
    Ok(nodes)
}

pub fn canonicalize_workspace(path: &str) -> Result<String, String> {
    let p = std::fs::canonicalize(path).map_err(|e| format!("invalid path: {e}"))?;
    if !p.is_dir() {
        return Err("workspace path is not a directory".into());
    }
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| "workspace path is not valid UTF-8".to_string())
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOpenResult {
    pub canonical_path: String,
    pub capability_id: String,
}

/// T2（#56）：Tauri 2 同步 command 在 main thread 執行，整檔讀寫／base64 編碼
/// 會凍住 UI event loop → command 一律 async ＋ 把 blocking 工作丟進
/// `tauri::async_runtime::spawn_blocking`（tokio 缺 `rt` feature，不可用
/// `tokio::task::spawn_blocking`）。`open_workspace`／`list_dir` 依 spec 維持
/// sync（µs 級 canonicalize／lazy 單層列目錄）。
pub(crate) async fn run_blocking<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| format!("fs blocking task failed: {e}"))?
}

#[tauri::command(async)]
pub fn workspace_canonical_path(path: String) -> Result<String, String> {
    canonicalize_workspace(&path)
}

#[tauri::command(async)]
pub fn open_workspace(
    path: String,
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
) -> Result<WorkspaceOpenResult, String> {
    let canonical_path = canonicalize_workspace(&path)?;
    let capability_id = state
        .0
        .activate(Path::new(&canonical_path))
        .map_err(String::from)?;
    Ok(WorkspaceOpenResult {
        canonical_path,
        capability_id,
    })
}

#[tauri::command(async)]
pub fn list_dir(path: String) -> Result<Vec<FileNode>, String> {
    list_dir_entries(Path::new(&path))
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpenFileResult {
    #[serde(rename_all = "camelCase")]
    Full {
        content: String,
        size: u64,
        line_ending: LineEnding,
    },
    #[serde(rename_all = "camelCase")]
    Limited {
        content: String,
        size: u64,
        line_ending: LineEnding,
    },
    #[serde(rename_all = "camelCase")]
    TooLarge { size: u64 },
    #[serde(rename_all = "camelCase")]
    Binary { size: u64 },
    #[serde(rename_all = "camelCase")]
    NonUtf8Readonly {
        content: String,
        encoding: String,
        size: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineEnding {
    Lf,
    #[serde(rename = "crlf")]
    CrLf,
    Mixed,
}

fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut has_bare_cr = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                has_crlf = true;
                index += 2;
            }
            b'\r' => {
                has_bare_cr = true;
                index += 1;
            }
            b'\n' => {
                has_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    if has_bare_cr || (has_lf && has_crlf) {
        LineEnding::Mixed
    } else if has_crlf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

pub fn classify_and_read(path: &Path) -> Result<OpenFileResult, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    let size = meta.len();
    if size > HARD_CAP_BYTES {
        return Ok(OpenFileResult::TooLarge { size });
    }

    let mut file = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut prefix = vec![0u8; FILE_ANALYSIS_BYTES.min(size as usize)];
    file.read_exact(&mut prefix)
        .map_err(|e| format!("read failed: {e}"))?;

    match analyze_byte_content(&prefix) {
        ByteContent::Binary => Ok(OpenFileResult::Binary { size }),
        ByteContent::Utf16Le | ByteContent::Utf16Be => {
            let bytes = read_rest(&mut file, prefix, size)?;
            let codec =
                if analyze_byte_content(&bytes[..bytes.len().min(2)]) == ByteContent::Utf16Be {
                    encoding_rs::UTF_16BE
                } else {
                    encoding_rs::UTF_16LE
                };
            let (cow, used, _) = codec.decode(&bytes);
            Ok(OpenFileResult::NonUtf8Readonly {
                content: cow.into_owned(),
                encoding: used.name().to_string(),
                size,
            })
        }
        ByteContent::Text => {
            let bytes = read_rest(&mut file, prefix, size)?;
            match String::from_utf8(bytes) {
                Ok(content) => {
                    let line_ending = detect_line_ending(&content);
                    if size > FULL_FEATURE_MAX_BYTES {
                        Ok(OpenFileResult::Limited {
                            content,
                            size,
                            line_ending,
                        })
                    } else {
                        Ok(OpenFileResult::Full {
                            content,
                            size,
                            line_ending,
                        })
                    }
                }
                Err(err) => {
                    // 非 UTF-8 且無 BOM：以 WINDOWS_1252 lossy 解碼供唯讀檢視
                    // 用 decode_without_bom_handling：避免內容中恰好出現 UTF-8/UTF-16 BOM 位元組時
                    // 被 decode() 嗅探並覆蓋成該編碼解碼（decode() 的 BOM 嗅探是為一般用途設計，
                    // 這裡需要的是固定逐位元組 WINDOWS_1252 解碼）
                    let bytes = err.into_bytes();
                    let (cow, _had_errors) =
                        encoding_rs::WINDOWS_1252.decode_without_bom_handling(&bytes);
                    Ok(OpenFileResult::NonUtf8Readonly {
                        content: cow.into_owned(),
                        encoding: encoding_rs::WINDOWS_1252.name().to_string(),
                        size,
                    })
                }
            }
        }
    }
}

fn read_rest(file: &mut std::fs::File, mut prefix: Vec<u8>, size: u64) -> Result<Vec<u8>, String> {
    prefix.reserve(size as usize - prefix.len());
    file.read_to_end(&mut prefix)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(prefix)
}

pub fn write_file(path: &str, content: &str) -> Result<u64, String> {
    std::fs::write(path, content).map_err(|e| format!("write failed: {e}"))?;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    let mtime = meta
        .modified()
        .map_err(|e| format!("mtime failed: {e}"))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("time error: {e}"))?
        .as_millis() as u64;
    Ok(mtime)
}

pub fn path_is_openable_file(path: &Path) -> Result<bool, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("stat failed: {error}")),
    };
    Ok(metadata.is_file())
}

#[tauri::command]
pub async fn is_openable_file(path: String) -> Result<bool, String> {
    run_blocking(move || path_is_openable_file(Path::new(&path))).await
}

#[tauri::command]
pub async fn open_file(path: String) -> Result<OpenFileResult, String> {
    run_blocking(move || classify_and_read(Path::new(&path))).await
}

#[tauri::command]
pub async fn save_file(path: String, content: String) -> Result<u64, String> {
    run_blocking(move || write_file(&path, &content)).await
}

// Native mutations are anchored to the workspace handle, never to a caller's
// absolute path. Canonicalization is only used to preserve in-root directory
// symlink aliases; every subsequent operation walks the resulting path no-follow.
fn mutation_parent(
    canonical: &Path,
    root: &crate::path_capability::PinnedDir,
    relative: &str,
    create_parents: bool,
) -> Result<
    (
        crate::path_capability::PinnedDir,
        crate::path_capability::SafeLeafName,
    ),
    String,
> {
    use crate::path_capability::{SafeLeafName, SafeRelativePath};
    let safe = SafeRelativePath::parse(relative)?;
    let parent = Path::new(relative).parent().unwrap_or(Path::new(""));
    let absolute = canonical.join(parent);
    let mut existing = absolute.as_path();
    while existing.symlink_metadata().is_err() {
        existing = existing.parent().ok_or("invalid-parent")?;
    }
    let resolved = existing.canonicalize().map_err(|e| e.to_string())?;
    let suffix = absolute
        .strip_prefix(existing)
        .map_err(|_| "invalid-parent")?;
    let resolved = resolved.join(suffix);
    let relative_parent = resolved
        .strip_prefix(canonical)
        .map_err(|_| "path escapes the workspace via symlink")?;
    let mut pinned = root.open_subdir("")?;
    for component in relative_parent.components() {
        let Component::Normal(name) = component else {
            return Err("invalid-parent".into());
        };
        let name = SafeLeafName::parse(name.to_str().ok_or("path-not-utf8")?)?;
        if create_parents && pinned.existing_kind(&name)?.is_none() {
            // A concurrent creator may win; the no-follow open below still decides.
            let _ = pinned.mkdir(&name);
        }
        pinned = pinned.open_subdir(name.as_str())?;
    }
    Ok((pinned, safe.leaf().clone()))
}

fn create_pinned(
    canonical: &Path,
    root: &crate::path_capability::PinnedDir,
    path: &str,
    directory: bool,
) -> Result<(), String> {
    let (parent, leaf) = mutation_parent(canonical, root, path, true)?;
    if directory {
        parent.mkdir(&leaf)
    } else {
        parent
            .create_exclusive(&leaf)?
            .sync_all()
            .map_err(|e| e.to_string())
    }
}

fn validate_mutation_leaf(
    canonical: &Path,
    relative: &str,
    parent: &crate::path_capability::PinnedDir,
    leaf: &crate::path_capability::SafeLeafName,
) -> Result<(), String> {
    if parent.existing_kind(leaf)? == Some(crate::path_capability::NodeKind::Symlink) {
        let target = canonical
            .join(relative)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !target.starts_with(canonical) {
            return Err("path escapes the workspace via symlink".into());
        }
    }
    Ok(())
}

fn rename_pinned(
    canonical: &Path,
    root: &crate::path_capability::PinnedDir,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let (source, source_leaf) = mutation_parent(canonical, root, from, false)?;
    let (destination, target_leaf) = mutation_parent(canonical, root, to, false)?;
    validate_mutation_leaf(canonical, from, &source, &source_leaf)?;
    source.rename_entry_new(&source_leaf, &destination, &target_leaf)
}

/// The user sees progress and can cancel, so a delete has no deadline; this
/// only stops a runaway walk.
const DELETE_ENTRY_LIMIT: usize = 2_000_000;
const DELETE_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum DeleteProgress {
    /// Walking the tree to learn its size.
    Counting {
        found: usize,
    },
    Deleting {
        removed: usize,
        total: usize,
    },
}

/// In-flight deletes by frontend operation id, so the user can cancel one.
#[derive(Default)]
pub struct DeleteOperations(Mutex<HashMap<String, Arc<AtomicBool>>>);

#[cfg(test)]
fn delete_pinned(
    canonical: &Path,
    root: &crate::path_capability::PinnedDir,
    path: &str,
) -> Result<(), String> {
    delete_pinned_reporting(canonical, root, path, &AtomicBool::new(false), &mut |_| {})
}

/// Counts the tree first so progress has a total, then deletes it. A cancel
/// during the count deletes nothing; later it leaves a partial delete.
fn delete_pinned_reporting(
    canonical: &Path,
    root: &crate::path_capability::PinnedDir,
    path: &str,
    cancelled: &AtomicBool,
    report: &mut dyn FnMut(DeleteProgress),
) -> Result<(), String> {
    let (parent, leaf) = mutation_parent(canonical, root, path, false)?;
    validate_mutation_leaf(canonical, path, &parent, &leaf)?;
    if parent.existing_kind(&leaf)?.is_none() {
        return Err("file-not-found".into());
    }
    let mut last = Instant::now();
    let total = parent.count_tree(&leaf, DELETE_ENTRY_LIMIT, &mut |found| {
        if last.elapsed() >= DELETE_PROGRESS_INTERVAL {
            last = Instant::now();
            report(DeleteProgress::Counting { found });
        }
        cancelled.load(Ordering::Relaxed)
    })?;
    let mut removed = 0;
    parent.remove_tree_with(&leaf, &mut || {
        if cancelled.load(Ordering::Relaxed) {
            return Err("delete-cancelled-partial".into());
        }
        if removed >= DELETE_ENTRY_LIMIT {
            return Err("delete-limit-reached-partial".into());
        }
        removed += 1;
        if last.elapsed() >= DELETE_PROGRESS_INTERVAL {
            last = Instant::now();
            report(DeleteProgress::Deleting { removed, total });
        }
        Ok(())
    })?;
    report(DeleteProgress::Deleting {
        removed: total,
        total,
    });
    Ok(())
}

#[tauri::command]
pub async fn fs_create_file(
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_capability_id: String,
    path: String,
) -> Result<(), String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || create_pinned(&canonical, &root, &path, false)).await
}

#[tauri::command]
pub async fn fs_create_dir(
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_capability_id: String,
    path: String,
) -> Result<(), String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || create_pinned(&canonical, &root, &path, true)).await
}

#[tauri::command]
pub async fn fs_rename(
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_capability_id: String,
    from: String,
    to: String,
) -> Result<(), String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    run_blocking(move || rename_pinned(&canonical, &root, &from, &to)).await
}

#[tauri::command]
pub async fn fs_delete(
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    operations: tauri::State<'_, DeleteOperations>,
    workspace_capability_id: String,
    path: String,
    operation_id: String,
    on_progress: tauri::ipc::Channel<DeleteProgress>,
) -> Result<(), String> {
    let (canonical, root) = state.0.mutation_root(&workspace_capability_id)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    operations
        .0
        .lock()
        .unwrap()
        .insert(operation_id.clone(), cancelled.clone());
    let result = run_blocking(move || {
        delete_pinned_reporting(&canonical, &root, &path, &cancelled, &mut |progress| {
            let _ = on_progress.send(progress);
        })
    })
    .await;
    operations.0.lock().unwrap().remove(&operation_id);
    result
}

#[tauri::command]
pub fn fs_delete_cancel(operations: tauri::State<'_, DeleteOperations>, operation_id: String) {
    if let Some(cancelled) = operations.0.lock().unwrap().get(&operation_id) {
        cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Serialize, Debug)]
pub struct FileBase64 {
    pub data: String,
    pub size: u64,
}

/// Reads a user-picked file (AgentZone image attachments) as base64. The
/// caller enforces the mime whitelist by extension; this side enforces only
/// the size ceiling so an oversized pick fails with a structured error
/// instead of ballooning the IPC payload.
pub fn read_base64_file(path: &str, max_bytes: u64) -> Result<FileBase64, String> {
    use base64::Engine;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a regular file: {path}"));
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "file too large: {} bytes (max {max_bytes})",
            meta.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    Ok(FileBase64 {
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        size: bytes.len() as u64,
    })
}

#[tauri::command]
pub async fn read_file_base64(path: String, max_bytes: u64) -> Result<FileBase64, String> {
    run_blocking(move || read_base64_file(&path, max_bytes)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn capability_mutations_preserve_aliases_and_resist_parent_substitution() {
        use crate::path_capability::WorkspacePathRegistry;
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("dir")).unwrap();
        symlink("dir", workspace.path().join("alias")).unwrap();
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(workspace.path()).unwrap();
        let (canonical, root) = registry.mutation_root(&id).unwrap();
        create_pinned(&canonical, &root, "alias/nested/ok", false).unwrap();
        assert!(workspace.path().join("dir/nested/ok").is_file());
        let (parent, leaf) = mutation_parent(&canonical, &root, "dir/created", false).unwrap();
        fs::rename(workspace.path().join("dir"), workspace.path().join("moved")).unwrap();
        symlink(outside.path(), workspace.path().join("dir")).unwrap();
        parent.create_exclusive(&leaf).unwrap();
        assert!(!outside.path().join("created").exists());
        assert!(workspace.path().join("moved/created").exists());
        assert!(create_pinned(&canonical, &root, "dir/escaped", false).is_err());
        // Preserve native policy: only in-root final links can be renamed/deleted.
        symlink(outside.path(), workspace.path().join("external-link")).unwrap();
        assert!(delete_pinned(&canonical, &root, "external-link").is_err());
        symlink("moved", workspace.path().join("final-link")).unwrap();
        rename_pinned(&canonical, &root, "final-link", "renamed-link").unwrap();
        delete_pinned(&canonical, &root, "renamed-link").unwrap();
        assert!(workspace.path().join("moved").is_dir());
        assert!(outside.path().is_dir());
        registry.clear();
        assert!(registry.mutation_root(&id).is_err());
    }

    #[test]
    #[ignore = "manual: creates 60k files to show deletes outgrow the old 50k budget"]
    fn delete_handles_more_entries_than_the_old_budget() {
        use crate::path_capability::WorkspacePathRegistry;
        let workspace = tempfile::tempdir().unwrap();
        for dir in 0..60 {
            let dir = workspace.path().join(format!("big/{dir}"));
            fs::create_dir_all(&dir).unwrap();
            for file in 0..1_000 {
                fs::write(dir.join(file.to_string()), "").unwrap();
            }
        }
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(workspace.path()).unwrap();
        let (canonical, root) = registry.mutation_root(&id).unwrap();
        let started = Instant::now();
        let mut reports = 0;
        delete_pinned_reporting(
            &canonical,
            &root,
            "big",
            &AtomicBool::new(false),
            &mut |_| reports += 1,
        )
        .unwrap();
        assert!(!workspace.path().join("big").exists());
        eprintln!(
            "deleted 60,061 entries in {:?} with {reports} progress reports",
            started.elapsed()
        );
        registry.clear();
    }

    #[test]
    fn delete_reports_its_total_and_a_cancel_before_deleting_keeps_everything() {
        use crate::path_capability::WorkspacePathRegistry;
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("big/nested")).unwrap();
        for i in 0..5 {
            fs::write(workspace.path().join(format!("big/nested/{i}.txt")), "x").unwrap();
        }
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(workspace.path()).unwrap();
        let (canonical, root) = registry.mutation_root(&id).unwrap();

        let error = delete_pinned_reporting(
            &canonical,
            &root,
            "big",
            &AtomicBool::new(true),
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(error, "delete-cancelled");
        assert_eq!(
            fs::read_dir(workspace.path().join("big/nested"))
                .unwrap()
                .count(),
            5
        );

        let mut reports = Vec::new();
        delete_pinned_reporting(
            &canonical,
            &root,
            "big",
            &AtomicBool::new(false),
            &mut |progress| reports.push(progress),
        )
        .unwrap();
        assert!(!workspace.path().join("big").exists());
        // big, nested and five files.
        assert_eq!(
            reports.last(),
            Some(&DeleteProgress::Deleting {
                removed: 7,
                total: 7
            })
        );
        registry.clear();
    }

    #[test]
    fn list_dir_sorts_dirs_first_then_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("zeta")).unwrap();
        fs::write(tmp.path().join("alpha.txt"), "a").unwrap();
        fs::write(tmp.path().join("Beta.txt"), "b").unwrap();
        let nodes = list_dir_entries(tmp.path()).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha.txt", "Beta.txt"]);
        assert!(nodes[0].is_dir);
        assert_eq!(nodes[0].kind, FileNodeKind::Directory);
        assert_eq!(nodes[1].kind, FileNodeKind::File);
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_marks_symlinks_distinctly() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("real.txt"), "ok").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real.txt"), tmp.path().join("link.txt"))
            .unwrap();
        let nodes = list_dir_entries(tmp.path()).unwrap();
        let link = nodes.iter().find(|node| node.name == "link.txt").unwrap();
        assert!(!link.is_dir);
        assert_eq!(link.kind, FileNodeKind::Symlink);
    }

    #[test]
    fn canonicalize_workspace_rejects_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        fs::write(&f, "x").unwrap();
        assert!(canonicalize_workspace(f.to_str().unwrap()).is_err());
        assert!(canonicalize_workspace(tmp.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn openable_file_accepts_files_and_rejects_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("target.txt");
        std::fs::write(&file, "ok").unwrap();

        assert_eq!(path_is_openable_file(&file).unwrap(), true);
        assert_eq!(path_is_openable_file(dir.path()).unwrap(), false);
        assert_eq!(
            path_is_openable_file(&dir.path().join("missing")).unwrap(),
            false
        );
    }

    #[test]
    fn open_file_classifies_by_size_and_content() {
        let tmp = tempfile::tempdir().unwrap();

        let small = tmp.path().join("small.ts");
        fs::write(&small, "export const a = 1\n").unwrap();
        match classify_and_read(&small).unwrap() {
            OpenFileResult::Full { content, .. } => assert!(content.contains("a = 1")),
            other => panic!("expected Full, got {other:?}"),
        }

        let png = tmp.path().join("img.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert!(matches!(
            classify_and_read(&png).unwrap(),
            OpenFileResult::Binary { .. }
        ));

        let big = tmp.path().join("big.txt");
        let f = fs::File::create(&big).unwrap();
        f.set_len(crate::file_content::HARD_CAP_BYTES + 1).unwrap();
        assert!(matches!(
            classify_and_read(&big).unwrap(),
            OpenFileResult::TooLarge { .. }
        ));
    }

    #[test]
    fn detects_editable_line_endings_without_changing_content() {
        for (name, bytes, expected) in [
            ("lf.txt", b"one\ntwo\n".as_slice(), LineEnding::Lf),
            ("crlf.txt", b"one\r\ntwo\r\n".as_slice(), LineEnding::CrLf),
            ("mixed.txt", b"one\r\ntwo\n".as_slice(), LineEnding::Mixed),
            ("bare-cr.txt", b"one\rtwo".as_slice(), LineEnding::Mixed),
            ("empty.txt", b"".as_slice(), LineEnding::Lf),
            ("no-newline.txt", b"one".as_slice(), LineEnding::Lf),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join(name);
            fs::write(&path, bytes).unwrap();
            match classify_and_read(&path).unwrap() {
                OpenFileResult::Full {
                    content,
                    line_ending,
                    ..
                } => {
                    assert_eq!(content.as_bytes(), bytes);
                    assert_eq!(line_ending, expected);
                }
                other => panic!("expected Full, got {other:?}"),
            }
        }
    }

    #[test]
    fn editable_line_ending_contract_serializes_as_typescript_shape() {
        let value = serde_json::to_value(OpenFileResult::Full {
            content: "one\r\ntwo\r\n".into(),
            size: 10,
            line_ending: LineEnding::CrLf,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "full",
                "content": "one\r\ntwo\r\n",
                "size": 10,
                "lineEnding": "crlf"
            })
        );
    }

    #[test]
    fn utf8_bom_content_and_line_ending_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("utf8-bom.txt");
        let bytes = b"\xef\xbb\xbfone\r\ntwo\r\n";
        fs::write(&path, bytes).unwrap();
        match classify_and_read(&path).unwrap() {
            OpenFileResult::Full {
                content,
                line_ending,
                ..
            } => {
                assert_eq!(content.as_bytes(), bytes);
                assert_eq!(line_ending, LineEnding::CrLf);
            }
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn open_file_limited_between_thresholds() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let mid = tmp.path().join("mid.txt");
        // 實寫文字內容（稀疏檔的 NUL 會被 sniff 判成 Binary，不可用 set_len）
        let block = "abcdefghij\r\n".repeat(86);
        let mut f = std::io::BufWriter::new(fs::File::create(&mid).unwrap());
        let times = crate::file_content::FULL_FEATURE_MAX_BYTES / 1024 + 2;
        for _ in 0..times {
            f.write_all(block.as_bytes()).unwrap();
        }
        drop(f);
        match classify_and_read(&mid).unwrap() {
            OpenFileResult::Limited { line_ending, .. } => {
                assert_eq!(line_ending, LineEnding::CrLf)
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn save_file_writes_and_returns_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.txt");
        let mtime = write_file(p.to_str().unwrap(), "hello").unwrap();
        assert!(mtime > 0);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn open_file_utf16_bom_is_readonly() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("utf16.txt");
        fs::write(&p, b"\xff\xfeh\x00i\x00").unwrap();
        match classify_and_read(&p).unwrap() {
            OpenFileResult::NonUtf8Readonly {
                content, encoding, ..
            } => {
                assert_eq!(content, "hi");
                assert!(encoding.contains("UTF-16"));
            }
            other => panic!("expected NonUtf8Readonly, got {other:?}"),
        }
    }

    // 生產環境 workspace 一律先過 open_workspace → canonicalize（macOS 上 tempdir
    // 的 /var 是 /private/var 的 symlink），測試也照此把 root canonicalize 後當
    // workspace 傳，才與實際邊界檢查一致。
    fn ws(tmp: &tempfile::TempDir) -> String {
        fs::canonicalize(tmp.path())
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn with_mutation_root<T>(
        workspace: &str,
        f: impl FnOnce(&Path, &crate::path_capability::PinnedDir) -> Result<T, String>,
    ) -> Result<T, String> {
        let registry = crate::path_capability::WorkspacePathRegistry::new();
        let id = registry.activate(Path::new(workspace))?;
        let (canonical, root) = registry.mutation_root(&id)?;
        f(&canonical, &root)
    }
    fn relative_test_path<'a>(workspace: &str, path: &'a str) -> Result<&'a str, String> {
        Path::new(path)
            .strip_prefix(workspace)
            .ok()
            .and_then(Path::to_str)
            .ok_or("outside-workspace".into())
    }
    fn create_file_in_workspace(workspace: &str, path: &str) -> Result<(), String> {
        with_mutation_root(workspace, |canonical, root| {
            create_pinned(canonical, root, relative_test_path(workspace, path)?, false)
        })
    }
    fn create_dir_in_workspace(workspace: &str, path: &str) -> Result<(), String> {
        with_mutation_root(workspace, |canonical, root| {
            create_pinned(canonical, root, relative_test_path(workspace, path)?, true)
        })
    }
    fn rename_in_workspace(workspace: &str, from: &str, to: &str) -> Result<(), String> {
        with_mutation_root(workspace, |canonical, root| {
            rename_pinned(
                canonical,
                root,
                relative_test_path(workspace, from)?,
                relative_test_path(workspace, to)?,
            )
        })
    }
    fn delete_in_workspace(workspace: &str, path: &str) -> Result<(), String> {
        with_mutation_root(workspace, |canonical, root| {
            delete_pinned(canonical, root, relative_test_path(workspace, path)?)
        })
    }

    fn under(workspace: &str, rel: &str) -> String {
        format!("{workspace}/{rel}")
    }

    #[test]
    fn fs_create_file_creates_with_parent_dirs_and_rejects_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let target = under(&w, "nested/deep/a.txt");
        create_file_in_workspace(&w, &target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "");
        // 已存在 → 錯誤，且內容不被覆蓋。
        fs::write(&target, "keep").unwrap();
        assert!(create_file_in_workspace(&w, &target).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn fs_create_dir_creates_and_rejects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let target = under(&w, "newdir");
        create_dir_in_workspace(&w, &target).unwrap();
        assert!(Path::new(&target).is_dir());
        assert!(create_dir_in_workspace(&w, &target).is_err());
    }

    #[test]
    fn fs_rename_moves_and_rejects_existing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let from = under(&w, "old.txt");
        let to = under(&w, "new.txt");
        fs::write(&from, "body").unwrap();
        rename_in_workspace(&w, &from, &to).unwrap();
        assert!(!Path::new(&from).exists());
        assert_eq!(fs::read_to_string(&to).unwrap(), "body");
        // to 已存在 → 錯誤。
        let from2 = under(&w, "other.txt");
        fs::write(&from2, "x").unwrap();
        assert!(rename_in_workspace(&w, &from2, &to).is_err());
        assert!(Path::new(&from2).exists());
    }

    #[test]
    fn fs_delete_removes_file_and_dir_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let file = under(&w, "f.txt");
        fs::write(&file, "x").unwrap();
        delete_in_workspace(&w, &file).unwrap();
        assert!(!Path::new(&file).exists());

        let dir = under(&w, "d");
        fs::create_dir_all(format!("{dir}/sub")).unwrap();
        fs::write(format!("{dir}/sub/inner.txt"), "y").unwrap();
        delete_in_workspace(&w, &dir).unwrap();
        assert!(!Path::new(&dir).exists());
    }

    #[test]
    fn fs_commands_reject_paths_escaping_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let escape = under(&w, "../escaped.txt");
        assert!(create_file_in_workspace(&w, &escape).is_err());
        assert!(create_dir_in_workspace(&w, &escape).is_err());
        assert!(delete_in_workspace(&w, &escape).is_err());
        assert!(rename_in_workspace(&w, &under(&w, "a.txt"), &escape).is_err());
        // 目標檔案未被建立在 workspace 之外。
        assert!(!Path::new(&fs::canonicalize(&tmp).unwrap())
            .parent()
            .unwrap()
            .join("escaped.txt")
            .exists());
    }

    #[test]
    fn fs_commands_reject_workspace_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        assert!(delete_in_workspace(&w, &w).is_err());
        assert!(Path::new(&w).is_dir());
    }

    // workspace 內放一個指向外部目錄的 symlink，斷言透過它（穿越 symlink component）
    // 的 create/rename/delete 全部 fail-closed，且外部目錄不被寫入/破壞。
    #[cfg(unix)]
    #[test]
    fn fs_commands_reject_paths_through_external_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_root = fs::canonicalize(outside.path()).unwrap();
        let w = ws(&tmp);

        // <workspace>/link -> <外部目錄>
        let link = under(&w, "link");
        symlink(&outside_root, &link).unwrap();

        // 透過 symlink 建立檔案 / 目錄：擋，且外部不出現該項目。
        let through_file = under(&w, "link/evil.txt");
        assert!(create_file_in_workspace(&w, &through_file).is_err());
        assert!(create_dir_in_workspace(&w, &under(&w, "link/evildir")).is_err());
        assert!(!outside_root.join("evil.txt").exists());
        assert!(!outside_root.join("evildir").exists());

        // 外部先放一個真實檔案，確認 delete / rename-from 穿越 symlink 都擋，
        // 且不是因為來源不存在才失敗——檔案仍在外部原地。
        let outside_file = outside_root.join("real.txt");
        fs::write(&outside_file, "keep").unwrap();
        assert!(delete_in_workspace(&w, &under(&w, "link/real.txt")).is_err());
        assert!(
            rename_in_workspace(&w, &under(&w, "link/real.txt"), &under(&w, "moved.txt")).is_err()
        );
        assert!(outside_file.exists());
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "keep");

        // rename-to 穿越 symlink 也擋，來源留在 workspace 內。
        let src = under(&w, "src.txt");
        fs::write(&src, "body").unwrap();
        assert!(rename_in_workspace(&w, &src, &through_file).is_err());
        assert!(Path::new(&src).exists());
        assert!(!outside_root.join("evil.txt").exists());
    }

    // T2（#56）：read_file_base64 async 化後的可測核心——行為與 async 化前一致。
    #[test]
    fn read_base64_file_encodes_within_limit() {
        use base64::Engine;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("img.bin");
        fs::write(&p, b"hello").unwrap();
        let out = read_base64_file(p.to_str().unwrap(), 10).unwrap();
        assert_eq!(out.size, 5);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&out.data)
                .unwrap(),
            b"hello"
        );
    }

    #[test]
    fn read_base64_file_rejects_oversize_and_non_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.bin");
        fs::write(&p, b"123456").unwrap();
        let err = read_base64_file(p.to_str().unwrap(), 5).unwrap_err();
        assert!(err.contains("file too large"), "got: {err}");
        // 目錄不是 regular file → 結構化錯誤而非讀取失敗。
        let err = read_base64_file(tmp.path().to_str().unwrap(), 100).unwrap_err();
        assert!(err.contains("not a regular file"), "got: {err}");
    }

    #[test]
    fn open_file_non_utf8_falls_back_windows_1252() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("mixed.txt");
        // UTF-8 BOM (EF BB BF) + 非法 UTF-8 序列（C3 後接 0x28，不是合法 continuation byte）+ "AB"
        fs::write(&p, b"\xef\xbb\xbf\xc3\x28AB").unwrap();
        match classify_and_read(&p).unwrap() {
            OpenFileResult::NonUtf8Readonly {
                content, encoding, ..
            } => {
                assert_eq!(encoding, "windows-1252");
                // 逐位元組 Windows-1252（byte >= 0xA0 時與 Latin-1 相同，碼位＝位元組值）解碼：
                // 0xEF -> ï(U+00EF)  0xBB -> »(U+00BB)  0xBF -> ¿(U+00BF)  0xC3 -> Ã(U+00C3)
                // 0x28/0x41/0x42 為 ASCII，直接對應 "(AB"。
                // 修復後 UTF-8 BOM 不再被 decode() 特殊嗅探並覆蓋 codec，逐 byte 走 windows-1252，
                // 故 BOM 三 bytes 也被解成 "ï»¿" 而非被剝除——此為預期行為（唯讀 fallback 用途）。
                assert_eq!(content, "ï»¿Ã(AB");
            }
            other => panic!("expected NonUtf8Readonly, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod directory_sort_tests {
    use super::*;

    fn node(name: &str, kind: FileNodeKind, position: usize) -> FileNode {
        FileNode {
            name: name.to_owned(),
            path: format!("/owned/{position}/{name}"),
            is_dir: kind == FileNodeKind::Directory,
            kind,
        }
    }

    #[test]
    fn lowercase_ties_preserve_order_and_metadata() {
        let mut nodes = vec![
            node("a", FileNodeKind::File, 0),
            node("A", FileNodeKind::Symlink, 1),
            node("K", FileNodeKind::Other, 2),
            node("K", FileNodeKind::File, 3),
            node("k", FileNodeKind::Symlink, 4),
            node("İ", FileNodeKind::File, 5),
            node("i\u{307}", FileNodeKind::Other, 6),
            node("Σ", FileNodeKind::File, 7),
            node("σ", FileNodeKind::Symlink, 8),
            node("A", FileNodeKind::Directory, 9),
            node("a", FileNodeKind::Directory, 10),
        ];
        let expected: Vec<_> = [9, 10, 0, 1, 5, 6, 2, 3, 4, 7, 8]
            .map(|index| serde_json::to_value(&nodes[index]).unwrap())
            .into();
        sort_file_nodes(&mut nodes);
        assert_eq!(
            serde_json::to_value(nodes).unwrap(),
            serde_json::json!(expected)
        );
    }

    #[test]
    fn cached_sort_matches_stable_comparator_across_sizes() {
        let names = [
            "Z", "a", "A", "Ä", "ä", "İ", "i\u{307}", "資料", "Σ", "σ", "ς", "K", "K", "k", "é",
            "e\u{301}",
        ];
        let kinds = [
            FileNodeKind::File,
            FileNodeKind::Directory,
            FileNodeKind::Symlink,
            FileNodeKind::Other,
        ];
        for count in [0, 1, 2, 7, 32, 257, 4096] {
            let make = || {
                (0..count)
                    .rev()
                    .map(|i| {
                        node(
                            names[(i * 7) % names.len()],
                            kinds[(i / 3) % kinds.len()],
                            i,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let mut actual = make();
            let mut expected = make();
            expected.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            sort_file_nodes(&mut actual);
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "count {count}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_listing_preserves_symlinks_and_read_errors() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("Zdir")).unwrap();
        std::fs::write(root.path().join("Alpha"), "owned").unwrap();
        std::os::unix::fs::symlink(root.path().join("Zdir"), root.path().join("DirLink")).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), root.path().join("Broken"))
            .unwrap();
        let nodes = list_dir_entries(root.path()).unwrap();
        assert_eq!(
            nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["Zdir", "Alpha", "Broken", "DirLink"]
        );
        for node in &nodes[2..] {
            assert_eq!(node.kind, FileNodeKind::Symlink);
            assert!(!node.is_dir);
        }
        assert!(list_dir_entries(&root.path().join("missing"))
            .unwrap_err()
            .starts_with("read_dir failed:"));
    }
}
