//! Direct SFTP document editing. Never download a document into a local workspace.
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use yuzora_host::protocol::MAX_FILE_BYTES;

/// Stream a revision for transfer conflict checks without loading large files.
pub(crate) async fn remote_revision(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<String>, String> {
    validate_path(path)?;
    let metadata = match sftp.symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == StatusCode::NoSuchFile =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() {
        return Err("sftp-not-regular-file".into());
    }
    let mut file = sftp.open(path).await.map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0u8; 32 * 1024];
    loop {
        let read = file.read(&mut bytes).await.map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&bytes[..read]);
    }
    Ok(Some(
        hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
    ))
}

pub(crate) async fn atomic_replace(
    raw: &russh_sftp::client::RawSftpSession,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let mut data = Vec::new();
    for value in [from, to] {
        data.extend_from_slice(&(value.len() as u32).to_be_bytes());
        data.extend_from_slice(value.as_bytes());
    }
    match raw
        .extended("posix-rename@openssh.com", data)
        .await
        .map_err(|e| e.to_string())?
    {
        Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(()),
        _ => Err("sftp-atomic-replace-failed".into()),
    }
}

#[tauri::command]
pub async fn sftp_file_revision(
    state: tauri::State<'_, crate::ssh_service::SshState>,
    session_id: String,
    path: String,
    transfer_id: String,
) -> Result<Option<String>, String> {
    let mut cancelled = state.0.transfers.observer(&session_id, &transfer_id)?;
    crate::sftp_transfer::until_cancelled(&mut cancelled, async {
        let sftp = state.0.ensure_sftp(&session_id).await?;
        remote_revision(&sftp, &path).await
    })
    .await
}

fn revision(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn read_regular(sftp: &SftpSession, path: &str) -> Result<Vec<u8>, String> {
    let metadata = sftp
        .symlink_metadata(path)
        .await
        .map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("sftp-not-regular-file".into());
    }
    if metadata.size.is_some_and(|size| size > MAX_FILE_BYTES) {
        return Err("file-too-large".into());
    }
    let file = sftp.open(path).await.map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("file-too-large".into());
    }
    Ok(bytes)
}

fn validate_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|p| matches!(p, "." | ".."))
    {
        return Err("invalid-sftp-path".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn sftp_create_file(
    state: tauri::State<'_, crate::ssh_service::SshState>,
    session_id: String,
    path: String,
) -> Result<(), String> {
    validate_path(&path)?;
    let sftp = state.0.ensure_sftp(&session_id).await?;
    let _reservation = state.0.reserve_remote_write(&session_id, &path)?;
    let mut file = sftp
        .open_with_flags(
            &path,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        )
        .await
        .map_err(|e| e.to_string())?;
    use tokio::io::AsyncWriteExt;
    file.shutdown().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_read_file_base64(
    state: tauri::State<'_, crate::ssh_service::SshState>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> Result<Value, String> {
    use base64::Engine;
    validate_path(&path)?;
    let sftp = state.0.ensure_sftp(&session_id).await?;
    let bytes = read_regular(&sftp, &path).await?;
    if bytes.len() as u64 > max_bytes {
        return Err("file-too-large".into());
    }
    Ok(json!({"size":bytes.len(), "data":base64::engine::general_purpose::STANDARD.encode(&bytes)}))
}

#[tauri::command]
pub async fn sftp_open_file(
    state: tauri::State<'_, crate::ssh_service::SshState>,
    session_id: String,
    path: String,
) -> Result<Value, String> {
    validate_path(&path)?;
    let sftp = state.0.ensure_sftp(&session_id).await?;
    let bytes = read_regular(&sftp, &path).await?;
    Ok(json!({"file":yuzora_host::content::classify_bytes(&bytes),"revision":revision(&bytes)}))
}

pub(crate) async fn save_remote(
    manager: &crate::ssh_service::SshManager,
    session_id: &str,
    path: &str,
    content: &str,
    expected_revision: &str,
) -> Result<String, String> {
    validate_path(path)?;
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err("file-too-large".into());
    }
    let sftp = manager.ensure_sftp(session_id).await?;
    let canonical = sftp.canonicalize(path).await.map_err(|e| e.to_string())?;
    if canonical != path {
        return Err("sftp-path-changed".into());
    }
    let _reservation = manager.reserve_remote_write(session_id, &canonical)?;
    if revision(&read_regular(&sftp, path).await?) != expected_revision {
        return Err("file-conflict".into());
    }
    let raw = manager.open_sftp_raw(session_id).await?;
    let version = raw.init().await.map_err(|e| e.to_string())?;
    if version
        .extensions
        .get("posix-rename@openssh.com")
        .map(String::as_str)
        != Some("1")
    {
        let _ = raw.close_session();
        return Err("sftp-atomic-replace-unavailable".into());
    }
    let parent = path.rsplit_once('/').ok_or("invalid-sftp-path")?.0;
    let scratch = format!("{parent}/.yuzora-save-{}", uuid::Uuid::new_v4());
    let result = async {
        let metadata = sftp
            .symlink_metadata(path)
            .await
            .map_err(|e| e.to_string())?;
        let attrs = FileAttributes {
            permissions: metadata.permissions.map(|mode| mode & 0o777),
            ..Default::default()
        };
        let handle = raw
            .open(
                &scratch,
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                attrs,
            )
            .await
            .map_err(|e| e.to_string())?
            .handle;
        let write_result = async {
            for (index, chunk) in content.as_bytes().chunks(32 * 1024).enumerate() {
                raw.write(&handle, (index * 32 * 1024) as u64, chunk.to_vec())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if version
                .extensions
                .get("fsync@openssh.com")
                .map(String::as_str)
                == Some("1")
            {
                raw.fsync(&handle).await.map_err(|e| e.to_string())?;
            }
            Ok::<(), String>(())
        }
        .await;
        let close_result = raw.close(&handle).await.map_err(|e| e.to_string());
        write_result?;
        close_result?;
        if revision(&read_regular(&sftp, path).await?) != expected_revision {
            return Err("file-conflict".into());
        }
        if sftp.canonicalize(path).await.map_err(|e| e.to_string())? != canonical {
            return Err("sftp-path-changed".into());
        }
        atomic_replace(&raw, &scratch, path).await?;
        Ok(revision(content.as_bytes()))
    }
    .await;
    if result.is_err() {
        let _ = sftp.remove_file(&scratch).await;
    }
    let _ = raw.close_session();
    result
}

#[tauri::command]
pub async fn sftp_save_file(
    state: tauri::State<'_, crate::ssh_service::SshState>,
    session_id: String,
    path: String,
    content: String,
    expected_revision: String,
) -> Result<String, String> {
    save_remote(&state.0, &session_id, &path, &content, &expected_revision).await
}
