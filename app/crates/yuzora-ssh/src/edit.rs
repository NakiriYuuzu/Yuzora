//! Editing a host's file with a local program: the file is downloaded to a
//! temporary folder and watched, and every save goes back to the host. If the
//! host's copy changed since it was fetched (someone else saved it), uploads
//! stop and the edit shows a conflict until the person overwrites or stops.

use crate::error::{ErrorKind, Result, SshError};
use crate::session::{Interaction, Manager};
use crate::sftp::base_name;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

/// Files bigger than this are transferred, not edited.
pub const MAX_EDIT_BYTES: u64 = 64 << 20;
const POLL: Duration = Duration::from_millis(800);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInfo {
    pub id: u64,
    pub host: String,
    pub remote: String,
    pub local: String,
    /// "watching", "uploading", "conflict", "error" or "stopped".
    pub state: &'static str,
    pub uploads: u32,
    /// ms since the epoch.
    pub last_upload: Option<u64>,
    pub error: Option<String>,
}

/// The host's copy as last seen: (mtime, size).
type Stamp = (Option<u32>, u64);

struct Edit {
    info: Mutex<EditInfo>,
    stop: watch::Sender<bool>,
    base: Mutex<Stamp>,
    force: watch::Sender<u64>,
}

impl Edit {
    fn info(&self) -> EditInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, f: impl FnOnce(&mut EditInfo)) {
        f(&mut self.info.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

#[derive(Default)]
pub struct Registry {
    edits: Mutex<BTreeMap<u64, Arc<Edit>>>,
    next: AtomicU64,
}

impl Registry {
    pub fn list(&self) -> Vec<EditInfo> {
        self.edits.lock().unwrap_or_else(|e| e.into_inner()).values().map(|e| e.info()).collect()
    }

    fn get(&self, id: u64) -> Option<Arc<Edit>> {
        self.edits.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
    }

    /// Stops watching (the local copy stays where it is).
    pub fn stop(&self, id: u64) -> bool {
        match self.edits.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            Some(e) => {
                e.stop.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// After a conflict: upload the local copy over the host's anyway.
    pub fn overwrite(&self, id: u64) -> bool {
        match self.get(id) {
            Some(e) => {
                e.force.send_modify(|n| *n += 1);
                true
            }
            None => false,
        }
    }
}

fn local_stamp(path: &PathBuf) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

async fn remote_stamp(sftp: &SftpSession, path: &str) -> Result<Stamp> {
    let m = sftp.metadata(path.to_owned()).await.map_err(SshError::sftp)?;
    Ok((m.mtime, m.size.unwrap_or(0)))
}

/// Writes `local` over `remote` through `<remote>.part`, keeping its permissions.
async fn upload(sftp: &SftpSession, local: &PathBuf, remote: &str) -> Result<Stamp> {
    let data = tokio::fs::read(local).await.map_err(SshError::io)?;
    let mode = sftp.metadata(remote.to_owned()).await.ok().and_then(|m| m.permissions);
    let part = format!("{remote}{}", crate::transfer::PART);
    let mut f = sftp.open_with_flags(part.clone(), OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE).await.map_err(SshError::sftp)?;
    f.write_all(&data).await.map_err(SshError::io)?;
    f.shutdown().await.map_err(SshError::io)?;
    let _ = f.close().await;
    let _ = sftp.remove_file(remote.to_owned()).await;
    sftp.rename(part, remote.to_owned()).await.map_err(SshError::sftp)?;
    if let Some(mode) = mode {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        let _ = sftp.set_metadata(remote.to_owned(), attrs).await;
    }
    remote_stamp(sftp, remote).await
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Manager {
    /// Downloads a host's file for editing and starts watching it; the local copy's path is in the result.
    pub async fn open_remote(self: &Arc<Self>, host: &str, remote: &str) -> Result<EditInfo> {
        let session = self.session(host, Interaction::Ask).await?;
        if let Some(e) = self.edits.list().into_iter().find(|e| e.host == session.host_id && e.remote == remote && e.state != "stopped") {
            return Ok(e);
        }
        let sftp = session.sftp().await?;
        let meta = sftp.metadata(remote.to_owned()).await.map_err(SshError::sftp)?;
        if meta.is_dir() {
            return Err(SshError::text(ErrorKind::Invalid, "edit-folder", &[]));
        }
        if meta.size.unwrap_or(0) > MAX_EDIT_BYTES {
            return Err(SshError::text(ErrorKind::Invalid, "edit-too-big", &[("mb", (MAX_EDIT_BYTES >> 20).to_string())]));
        }
        let id = self.edits.next.fetch_add(1, Ordering::Relaxed) + 1;
        let dir = self.ctx.temp.join("edit").join(format!("{}-{id}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.map_err(SshError::io)?;
        let name = base_name(remote);
        let local = dir.join(if crate::sftp::check_name(&name).is_ok() { name } else { "file".into() });
        let mut data = Vec::new();
        let mut f = sftp.open_with_flags(remote.to_owned(), OpenFlags::READ).await.map_err(SshError::sftp)?;
        f.read_to_end(&mut data).await.map_err(SshError::io)?;
        let _ = f.close().await;
        tokio::fs::write(&local, &data).await.map_err(SshError::io)?;
        let (stop, mut stopped) = watch::channel(false);
        let (force, mut forced) = watch::channel(0u64);
        let edit = Arc::new(Edit {
            info: Mutex::new(EditInfo { id, host: session.host_id.clone(), remote: remote.to_owned(), local: local.to_string_lossy().into_owned(), state: "watching", uploads: 0, last_upload: None, error: None }),
            stop,
            base: Mutex::new((meta.mtime, meta.size.unwrap_or(0))),
            force,
        });
        self.edits.edits.lock().unwrap_or_else(|e| e.into_inner()).insert(id, edit.clone());
        let me = self.clone();
        let (host_id, remote) = (session.host_id.clone(), remote.to_owned());
        tokio::spawn(async move {
            let mut seen = local_stamp(&local);
            let mut pending: Option<(SystemTime, u64)> = None;
            loop {
                let overwrite = tokio::select! {
                    _ = stopped.wait_for(|s| *s) => break,
                    _ = forced.changed() => true,
                    _ = tokio::time::sleep(POLL) => false,
                };
                let now = local_stamp(&local);
                if !overwrite {
                    // Upload once a save has settled (the same stamp twice in a row).
                    if now == seen {
                        pending = None;
                        continue;
                    }
                    if pending != now {
                        pending = now;
                        continue;
                    }
                    if edit.info().state == "conflict" {
                        continue;
                    }
                }
                pending = None;
                seen = now;
                edit.set(|i| i.state = "uploading");
                let result = async {
                    let sftp = me.session(&host_id, Interaction::Ask).await?.sftp().await?;
                    let base = *edit.base.lock().unwrap_or_else(|e| e.into_inner());
                    if !overwrite && remote_stamp(&sftp, &remote).await? != base {
                        return Ok(None);
                    }
                    upload(&sftp, &local, &remote).await.map(Some)
                }
                .await;
                match result {
                    Ok(Some(stamp)) => {
                        *edit.base.lock().unwrap_or_else(|e| e.into_inner()) = stamp;
                        edit.set(|i| {
                            i.state = "watching";
                            i.uploads += 1;
                            i.last_upload = Some(now_ms());
                            i.error = None;
                        });
                    }
                    Ok(None) => edit.set(|i| {
                        i.state = "conflict";
                        i.error = Some(crate::strings::t("errors.edit-conflict", &[]));
                    }),
                    Err(e) => edit.set(|i| {
                        i.state = "error";
                        i.error = Some(e.message);
                    }),
                }
            }
            edit.set(|i| i.state = "stopped");
        });
        Ok(self.edits.get(id).map(|e| e.info()).ok_or_else(SshError::closed)?)
    }
}
