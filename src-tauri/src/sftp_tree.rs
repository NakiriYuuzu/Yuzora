//! Manual folder transfers use an owned staging tree and publish only on success.
//! Existing destination directories are never merged or overwritten.
use crate::path_capability::{NodeKind, PinnedDir, SafeLeafName, SafeRelativePath};
use crate::sftp_transfer::Transfer;
use crate::ssh_service::{SshManager, SshState};
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{OpenFlags, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 64;
const MAX_PATH_BYTES: usize = 4096;
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Upload,
    Download,
}
struct Grant {
    root: PinnedDir,
    direction: Direction,
    expires: Instant,
}
#[derive(Default)]
pub struct TreeState(Mutex<HashMap<String, Grant>>);
#[derive(Serialize)]
pub struct TreeSelection {
    id: String,
    leaf: String,
}
impl TreeState {
    pub fn clear(&self) {
        if let Ok(mut grants) = self.0.lock() {
            grants.clear();
        }
    }
    fn grant(
        &self,
        path: &Path,
        direction: Direction,
        leaf: &str,
    ) -> Result<TreeSelection, String> {
        let leaf = SafeLeafName::parse(leaf)?.as_str().to_owned();
        let root = PinnedDir::open_dir(path)?;
        let mut grants = self.0.lock().map_err(|_| "sftp-tree-lock")?;
        grants.retain(|_, value| value.expires > Instant::now());
        if grants.len() >= 32 {
            return Err("sftp-tree-selection-limit".into());
        }
        let id = format!("tree-{}", uuid::Uuid::new_v4());
        grants.insert(
            id.clone(),
            Grant {
                root,
                direction,
                expires: Instant::now() + Duration::from_secs(300),
            },
        );
        Ok(TreeSelection { id, leaf })
    }
    fn take(&self, id: &str, direction: Direction) -> Result<Grant, String> {
        let grant = self
            .0
            .lock()
            .map_err(|_| "sftp-tree-lock")?
            .remove(id)
            .ok_or("sftp-tree-selection-expired")?;
        if grant.direction != direction || grant.expires <= Instant::now() {
            return Err("sftp-tree-selection-expired".into());
        }
        Ok(grant)
    }
}

#[tauri::command(async)]
pub async fn sftp_pick_tree(
    app: tauri::AppHandle,
    state: tauri::State<'_, TreeState>,
    direction: Direction,
    suggested_leaf: Option<String>,
) -> Result<Option<TreeSelection>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |file| {
        let _ = tx.send(file);
    });
    let Some(file) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let leaf = if direction == Direction::Upload {
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("sftp-unsafe-name")?
    } else {
        suggested_leaf.as_deref().ok_or("sftp-unsafe-name")?
    };
    state.grant(&path, direction, leaf).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TreeRequest {
    selection_id: String,
    transfer_id: String,
    direction: Direction,
    remote_path: String,
    name: String,
}
#[derive(Debug, Serialize)]
pub struct TreeResult {
    files: usize,
    bytes: u64,
}
#[derive(Clone)]
struct Entry {
    path: String,
    directory: bool,
    size: u64,
}
fn child(parent: &str, leaf: &str) -> String {
    format!("{}/{leaf}", parent.trim_end_matches('/'))
}
fn check_path(path: &str) -> Result<(), String> {
    if path.len() > MAX_PATH_BYTES || path.split('/').count() > MAX_DEPTH {
        return Err("sftp-tree-depth-limit".into());
    }
    SafeRelativePath::parse(path)?;
    Ok(())
}
fn count(entries: &[Entry]) -> Result<u64, String> {
    let bytes = entries
        .iter()
        .try_fold(0u64, |sum, item| sum.checked_add(item.size))
        .ok_or("sftp-tree-size-limit")?;
    if bytes > MAX_TOTAL_BYTES {
        return Err("sftp-tree-size-limit".into());
    }
    Ok(bytes)
}
fn scan_local(root: &PinnedDir, transfer: &Transfer) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(parent) = pending.pop() {
        transfer.check()?;
        let dir = root.open_subdir(&parent)?;
        for (name, kind) in dir.list_entries(MAX_ENTRIES - entries.len())? {
            let path = if parent.is_empty() {
                name
            } else {
                child(&parent, &name)
            };
            check_path(&path)?;
            let (directory, size) = match kind {
                NodeKind::Directory => {
                    pending.push(path.clone());
                    (true, 0)
                }
                NodeKind::File => (false, root.open_file(&SafeRelativePath::parse(&path)?)?.len),
                _ => return Err(format!("sftp-tree-unsupported-entry: {path}")),
            };
            entries.push(Entry {
                path,
                directory,
                size,
            });
        }
    }
    count(&entries)?;
    Ok(entries)
}
async fn require_missing(sftp: &SftpSession, path: &str) -> Result<(), String> {
    match sftp.symlink_metadata(path).await {
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == StatusCode::NoSuchFile =>
        {
            Ok(())
        }
        Ok(_) => Err("sftp-folder-conflict".into()),
        Err(error) => Err(error.to_string()),
    }
}
async fn canonical_directory(sftp: &SftpSession, path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.contains('\0')
        || sftp.canonicalize(path).await.map_err(|e| e.to_string())? != path
    {
        return Err("sftp-path-changed".into());
    }
    if !sftp
        .symlink_metadata(path)
        .await
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("sftp-not-directory".into());
    }
    Ok(())
}
async fn scan_remote(
    raw: &RawSftpSession,
    root: &str,
    transfer: &mut Transfer,
) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(parent) = pending.pop() {
        let path = if parent.is_empty() {
            root.to_owned()
        } else {
            child(root, &parent)
        };
        let handle = transfer
            .run(async {
                raw.opendir(path)
                    .await
                    .map(|result| result.handle)
                    .map_err(|e| e.to_string())
            })
            .await?;
        let scan = async {
            loop {
                let batch = transfer
                    .run(async {
                        match raw.readdir(&handle).await {
                            Ok(names) => Ok(Some(names.files)),
                            Err(russh_sftp::client::error::Error::Status(status))
                                if status.status_code == StatusCode::Eof =>
                            {
                                Ok(None)
                            }
                            Err(error) => Err(error.to_string()),
                        }
                    })
                    .await?;
                let Some(batch) = batch else {
                    break;
                };
                for file in batch {
                    if file.filename == "." || file.filename == ".." {
                        continue;
                    }
                    SafeLeafName::parse(&file.filename)?;
                    if entries.len() >= MAX_ENTRIES {
                        return Err("sftp-tree-entry-limit".into());
                    }
                    let path = if parent.is_empty() {
                        file.filename
                    } else {
                        child(&parent, &file.filename)
                    };
                    check_path(&path)?;
                    let kind = file.attrs.file_type();
                    let directory = kind.is_dir();
                    if !directory && !kind.is_file() {
                        return Err(format!("sftp-tree-unsupported-entry: {path}"));
                    }
                    if directory {
                        pending.push(path.clone());
                    }
                    entries.push(Entry {
                        path,
                        directory,
                        size: if directory {
                            0
                        } else {
                            file.attrs.size.ok_or("sftp-size-unavailable")?
                        },
                    });
                }
            }
            Ok::<_, String>(())
        }
        .await;
        let closed = raw.close(handle).await.map_err(|e| e.to_string());
        scan?;
        closed?;
    }
    count(&entries)?;
    Ok(entries)
}

impl SshManager {
    async fn upload_tree(
        &self,
        session: &str,
        source: PinnedDir,
        remote: &str,
        name: &str,
        transfer: &mut Transfer,
        progress: &(dyn Fn(u64, u64, bool) + Send + Sync),
    ) -> Result<TreeResult, String> {
        let id = transfer.id().to_owned();
        let entries = scan_local(&source, transfer)?;
        let total = count(&entries)?;
        let sftp = transfer.run(self.ensure_sftp(session)).await?;
        transfer.run(canonical_directory(&sftp, remote)).await?;
        let destination = child(remote, name);
        transfer.run(require_missing(&sftp, &destination)).await?;
        let staging = child(remote, &format!("{name}.yz-tmp-{id}"));
        sftp.create_dir(&staging).await.map_err(|e| e.to_string())?;
        let mut created: Vec<(String, bool)> = Vec::new();
        let mut bytes = 0;
        let mut files = 0;
        let mut last_emit = 0;
        let outcome = async {
            for entry in &entries {
                transfer.check()?;
                let target = child(&staging, &entry.path);
                let parent = target.rsplit_once('/').ok_or("sftp-path-changed")?.0;
                transfer.run(canonical_directory(&sftp, parent)).await?;
                if entry.directory {
                    sftp.create_dir(&target).await.map_err(|e| e.to_string())?;
                    created.push((target, true));
                    continue;
                }
                let opened = source.open_file(&SafeRelativePath::parse(&entry.path)?)?;
                let before = opened.file.metadata().map_err(|e| e.to_string())?;
                if opened.len != entry.size {
                    return Err("sftp-source-changed".into());
                }
                let mut local = tokio::fs::File::from_std(opened.file);
                let mut output = sftp
                    .open_with_flags(
                        &target,
                        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                created.push((target, false));
                transfer
                    .run(async {
                        let mut buffer = vec![0u8; 32768];
                        let mut copied = 0;
                        loop {
                            let n = local.read(&mut buffer).await.map_err(|e| e.to_string())?;
                            if n == 0 {
                                break;
                            }
                            copied += n as u64;
                            if copied > entry.size {
                                return Err("sftp-source-changed".into());
                            }
                            output
                                .write_all(&buffer[..n])
                                .await
                                .map_err(|e| e.to_string())?;
                            bytes += n as u64;
                            if bytes - last_emit >= 256 * 1024 {
                                last_emit = bytes;
                                progress(bytes, total, false);
                            }
                        }
                        let after = local.metadata().await.map_err(|e| e.to_string())?;
                        if copied != entry.size
                            || before.modified().ok() != after.modified().ok()
                            || after.len() != before.len()
                        {
                            return Err("sftp-source-changed".into());
                        }
                        output.shutdown().await.map_err(|e| e.to_string())
                    })
                    .await?;
                files += 1;
            }
            transfer.run(canonical_directory(&sftp, remote)).await?;
            transfer.run(require_missing(&sftp, &destination)).await?;
            transfer.check()?;
            sftp.rename(&staging, &destination)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = outcome {
            let mut clean = true;
            for (path, directory) in created.into_iter().rev() {
                clean &= if directory {
                    sftp.remove_dir(path).await.is_ok()
                } else {
                    sftp.remove_file(path).await.is_ok()
                };
            }
            clean &= sftp.remove_dir(&staging).await.is_ok();
            return Err(if clean {
                error
            } else {
                format!("{error}; sftp-partial-staging: {staging}")
            });
        }
        progress(bytes, total, true);
        Ok(TreeResult { files, bytes })
    }

    async fn download_tree(
        &self,
        session: &str,
        destination: PinnedDir,
        remote: &str,
        name: &str,
        transfer: &mut Transfer,
        progress: &(dyn Fn(u64, u64, bool) + Send + Sync),
    ) -> Result<TreeResult, String> {
        let id = transfer.id().to_owned();
        let sftp = transfer.run(self.ensure_sftp(session)).await?;
        transfer.run(canonical_directory(&sftp, remote)).await?;
        let raw = transfer.run(self.open_sftp_raw(session)).await?;
        transfer
            .run(async { raw.init().await.map_err(|e| e.to_string()) })
            .await?;
        let scan = scan_remote(&raw, remote, transfer).await;
        let _ = raw.close_session();
        let entries = scan?;
        let total = count(&entries)?;
        let leaf = SafeLeafName::parse(name)?;
        if destination
            .list_entries(MAX_ENTRIES)?
            .iter()
            .any(|(existing, _)| existing == name)
        {
            return Err("sftp-folder-conflict".into());
        }
        let stage_name = SafeLeafName::parse(&format!("{name}.yz-tmp-{id}"))?;
        destination.mkdir(&stage_name)?;
        let staging = destination.open_subdir(stage_name.as_str())?;
        let mut created: Vec<(String, bool)> = Vec::new();
        let mut bytes = 0;
        let mut files = 0;
        let mut last_emit = 0;
        let outcome = async {
            for entry in &entries {
                transfer.check()?;
                let (parent, name) = entry.path.rsplit_once('/').unwrap_or(("", &entry.path));
                let dir = staging.open_subdir(parent)?;
                let name = SafeLeafName::parse(name)?;
                if entry.directory {
                    dir.mkdir(&name)?;
                    created.push((entry.path.clone(), true));
                    continue;
                }
                let path = child(remote, &entry.path);
                // Every ancestor must still resolve inside the source tree.
                let remote_parent = if parent.is_empty() {
                    remote.to_owned()
                } else {
                    child(remote, parent)
                };
                transfer
                    .run(canonical_directory(&sftp, &remote_parent))
                    .await?;
                let revision = transfer
                    .run(crate::sftp_edit::remote_revision(&sftp, &path))
                    .await?
                    .ok_or("sftp-source-changed")?;
                let mut input = transfer
                    .run(async { sftp.open(&path).await.map_err(|e| e.to_string()) })
                    .await?;
                let mut output = tokio::fs::File::from_std(dir.create_exclusive(&name)?);
                created.push((entry.path.clone(), false));
                transfer
                    .run(async {
                        use sha2::{Digest, Sha256};
                        let mut hash = Sha256::new();
                        let mut buffer = vec![0u8; 32768];
                        let mut copied = 0;
                        loop {
                            let n = input.read(&mut buffer).await.map_err(|e| e.to_string())?;
                            if n == 0 {
                                break;
                            }
                            copied += n as u64;
                            if copied > entry.size {
                                return Err("sftp-source-changed".into());
                            }
                            output
                                .write_all(&buffer[..n])
                                .await
                                .map_err(|e| e.to_string())?;
                            hash.update(&buffer[..n]);
                            bytes += n as u64;
                            if bytes - last_emit >= 256 * 1024 {
                                last_emit = bytes;
                                progress(bytes, total, false);
                            }
                        }
                        let actual: String =
                            hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
                        if copied != entry.size || actual != revision {
                            return Err("sftp-source-changed".into());
                        }
                        output.sync_all().await.map_err(|e| e.to_string())
                    })
                    .await?;
                files += 1;
            }
            transfer.check()?;
            destination.rename_new(&stage_name, &destination, &leaf)?;
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = outcome {
            let mut clean = true;
            for (path, directory) in created.into_iter().rev() {
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
                let result = staging
                    .open_subdir(parent)
                    .map_err(String::from)
                    .and_then(|dir| {
                        let leaf = SafeLeafName::parse(name)?;
                        if directory {
                            dir.remove_empty_dir(&leaf)
                        } else {
                            dir.unlink(&leaf).map_err(String::from)
                        }
                    });
                clean &= result.is_ok();
            }
            drop(staging);
            clean &= destination.remove_empty_dir(&stage_name).is_ok();
            return Err(if clean {
                error
            } else {
                format!("{error}; sftp-partial-staging: {}", stage_name.as_str())
            });
        }
        progress(bytes, total, true);
        Ok(TreeResult { files, bytes })
    }
}

#[tauri::command]
pub async fn sftp_transfer_tree(
    app: tauri::AppHandle,
    ssh: tauri::State<'_, SshState>,
    state: tauri::State<'_, TreeState>,
    session_id: String,
    request: TreeRequest,
) -> Result<TreeResult, String> {
    let name = SafeLeafName::parse(&request.name)?;
    let grant = state.take(&request.selection_id, request.direction)?;
    let mut transfer = ssh.0.transfers.start(&session_id, &request.transfer_id)?;
    transfer.check()?;
    let manager: Arc<SshManager> = ssh.0.clone();
    let progress = |bytes, total, done| {
        manager.emit_progress(&app, &session_id, &request.transfer_id, bytes, total, done)
    };
    match request.direction {
        Direction::Upload => {
            manager
                .upload_tree(
                    &session_id,
                    grant.root,
                    &request.remote_path,
                    name.as_str(),
                    &mut transfer,
                    &progress,
                )
                .await
        }
        Direction::Download => {
            manager
                .download_tree(
                    &session_id,
                    grant.root,
                    &request.remote_path,
                    name.as_str(),
                    &mut transfer,
                    &progress,
                )
                .await
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) async fn verify_tree_transfers(manager: &SshManager, session: &str, fixture: &Path) {
    let source_path = fixture.join("tree source");
    std::fs::create_dir_all(source_path.join("中文 nested/empty")).unwrap();
    let data = vec![b't'; 1024 * 1024];
    std::fs::write(source_path.join("中文 nested/data.bin"), &data).unwrap();
    std::fs::write(source_path.join("root.txt"), b"hello").unwrap();
    let destination_path = fixture.join("tree destination");
    std::fs::create_dir_all(&destination_path).unwrap();
    let registry = TreeState::default();
    let selected = registry
        .grant(&source_path, Direction::Upload, "tree source")
        .unwrap();
    let source = registry.take(&selected.id, Direction::Upload).unwrap().root;
    assert!(registry.take(&selected.id, Direction::Upload).is_err());
    let id = manager.transfers.reserve(session).unwrap();
    let mut transfer = manager.transfers.start(session, &id).unwrap();
    let result = manager
        .upload_tree(
            session,
            source,
            fixture.to_str().unwrap(),
            "uploaded tree",
            &mut transfer,
            &|_, _, _| {},
        )
        .await
        .unwrap();
    assert_eq!(result.files, 2);
    assert_eq!(
        std::fs::read(fixture.join("uploaded tree/中文 nested/data.bin")).unwrap(),
        data
    );
    assert!(fixture.join("uploaded tree/中文 nested/empty").is_dir());
    drop(transfer);

    let id = manager.transfers.reserve(session).unwrap();
    let mut transfer = manager.transfers.start(session, &id).unwrap();
    let downloaded = manager
        .download_tree(
            session,
            PinnedDir::open_dir(&destination_path).unwrap(),
            fixture.join("uploaded tree").to_str().unwrap(),
            "downloaded tree",
            &mut transfer,
            &|_, _, _| {},
        )
        .await
        .unwrap();
    assert_eq!(downloaded.bytes, result.bytes);
    assert_eq!(
        std::fs::read(destination_path.join("downloaded tree/中文 nested/data.bin")).unwrap(),
        data
    );
    assert!(destination_path
        .join("downloaded tree/中文 nested/empty")
        .is_dir());
    drop(transfer);

    let id = manager.transfers.reserve(session).unwrap();
    let mut transfer = manager.transfers.start(session, &id).unwrap();
    let conflict = manager
        .upload_tree(
            session,
            PinnedDir::open_dir(&source_path).unwrap(),
            fixture.to_str().unwrap(),
            "uploaded tree",
            &mut transfer,
            &|_, _, _| {},
        )
        .await
        .unwrap_err();
    assert_eq!(conflict, "sftp-folder-conflict");
    assert_eq!(
        std::fs::read(fixture.join("uploaded tree/root.txt")).unwrap(),
        b"hello"
    );
    drop(transfer);

    let id = manager.transfers.reserve(session).unwrap();
    let mut transfer = manager.transfers.start(session, &id).unwrap();
    let cancel = |bytes, _, _| {
        if bytes > 0 {
            manager.transfers.cancel(session, &id).unwrap();
        }
    };
    let error = manager
        .upload_tree(
            session,
            PinnedDir::open_dir(&source_path).unwrap(),
            fixture.to_str().unwrap(),
            "cancelled tree",
            &mut transfer,
            &cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(error, "sftp-transfer-cancelled");
    assert!(!fixture.join("cancelled tree").exists());
    assert!(!fixture.join(format!("cancelled tree.yz-tmp-{id}")).exists());
    drop(transfer);

    let id = manager.transfers.reserve(session).unwrap();
    let mut transfer = manager.transfers.start(session, &id).unwrap();
    let cancel = |bytes, _, _| {
        if bytes > 0 {
            manager.transfers.cancel(session, &id).unwrap();
        }
    };
    let error = manager
        .download_tree(
            session,
            PinnedDir::open_dir(&destination_path).unwrap(),
            fixture.join("uploaded tree").to_str().unwrap(),
            "cancelled download",
            &mut transfer,
            &cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(error, "sftp-transfer-cancelled");
    assert!(!destination_path.join("cancelled download").exists());
    assert!(!destination_path
        .join(format!("cancelled download.yz-tmp-{id}"))
        .exists());
    drop(transfer);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", source_path.join("outside")).unwrap();
        let id = manager.transfers.reserve(session).unwrap();
        let mut transfer = manager.transfers.start(session, &id).unwrap();
        assert!(manager
            .upload_tree(
                session,
                PinnedDir::open_dir(&source_path).unwrap(),
                fixture.to_str().unwrap(),
                "unsafe tree",
                &mut transfer,
                &|_, _, _| {}
            )
            .await
            .unwrap_err()
            .contains("unsupported-entry"));
        assert!(!fixture.join("unsafe tree").exists());
    }
}
