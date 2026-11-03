//! Browsing both sides of the SFTP pane: this computer's folders and a host's
//! (over its shared SFTP session), with the same entry shape for both.

use crate::error::{ErrorKind, Result, SshError};
use crate::session::{Interaction, Manager};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Which side of the pane: this computer, or a host (id, alias, machine…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote(String),
}

impl Side {
    /// `null` / "" / "local" is this computer.
    pub fn from_key(key: Option<&str>) -> Side {
        match key.map(str::trim) {
            None | Some("") | Some("local") => Side::Local,
            Some(k) => Side::Remote(k.to_owned()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// "dir", "file", "link" or "other".
    pub kind: &'static str,
    /// A folder, or a link to one: double-click opens it.
    pub dir: bool,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: Option<u64>,
    /// Unix permission bits.
    pub mode: Option<u32>,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
}

/// `dir/name` on a host (always `/`-separated).
pub fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder above a host path; None at the root.
pub fn remote_parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".into()),
        Some(i) => Some(trimmed[..i].to_owned()),
        None => None,
    }
}

/// The last part of a path on either side.
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).to_owned()
}

/// A new file or folder name: one path segment.
pub fn check_name(name: &str) -> Result<()> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) || (cfg!(windows) && name.contains(['\\', ':']));
    if bad {
        return Err(SshError::text(ErrorKind::Invalid, "bad-name", &[("name", name.to_owned())]));
    }
    Ok(())
}

fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
}

fn local_entry(path: &Path) -> Option<Entry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let meta = std::fs::symlink_metadata(path).ok()?;
    let link = meta.file_type().is_symlink();
    let target = if link { std::fs::metadata(path).ok() } else { Some(meta.clone()) };
    let dir = target.as_ref().is_some_and(|m| m.is_dir());
    let kind = if link {
        "link"
    } else if meta.is_dir() {
        "dir"
    } else if meta.is_file() {
        "file"
    } else {
        "other"
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let mode = None;
    Some(Entry {
        hidden: name.starts_with('.'),
        name,
        path: path.to_string_lossy().into_owned(),
        kind,
        dir,
        size: if dir { 0 } else { target.as_ref().map(|m| m.len()).unwrap_or(0) },
        mtime: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
        mode,
    })
}

fn list_local(path: &str, home: &Path) -> Result<Listing> {
    let dir = match path.trim() {
        "" | "~" => home.to_path_buf(),
        p => match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        },
    };
    let dir = std::fs::canonicalize(&dir).map_err(SshError::io)?;
    let mut entries: Vec<Entry> = std::fs::read_dir(&dir).map_err(SshError::io)?.filter_map(|e| e.ok()).filter_map(|e| local_entry(&e.path())).collect();
    sort(&mut entries);
    Ok(Listing { path: dir.to_string_lossy().into_owned(), parent: dir.parent().map(|p| p.to_string_lossy().into_owned()), entries })
}

async fn list_remote(sftp: &SftpSession, path: &str) -> Result<Listing> {
    let asked = match path.trim() {
        "" | "~" => ".".to_owned(),
        p => match p.strip_prefix("~/") {
            Some(rest) => format!("./{rest}"),
            None => p.to_owned(),
        },
    };
    let dir = sftp.canonicalize(asked).await.map_err(SshError::sftp)?;
    let mut entries = Vec::new();
    for e in sftp.read_dir(dir.clone()).await.map_err(SshError::sftp)? {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let path = remote_join(&dir, &name);
        let meta = e.metadata();
        let link = meta.is_symlink();
        let dir_like = if link { sftp.metadata(path.clone()).await.map(|m| m.is_dir()).unwrap_or(false) } else { meta.is_dir() };
        let kind = if link {
            "link"
        } else if meta.is_dir() {
            "dir"
        } else if meta.is_regular() {
            "file"
        } else {
            "other"
        };
        entries.push(Entry {
            hidden: name.starts_with('.'),
            name,
            path,
            kind,
            dir: dir_like,
            size: if dir_like { 0 } else { meta.size.unwrap_or(0) },
            mtime: meta.mtime.map(u64::from),
            mode: meta.permissions.map(|p| p & 0o7777),
        });
    }
    sort(&mut entries);
    Ok(Listing { parent: remote_parent(&dir), path: dir, entries })
}

/// Deletes a host path; folders with everything in them.
async fn remove_remote(sftp: &SftpSession, path: &str) -> Result<()> {
    let meta = sftp.symlink_metadata(path.to_owned()).await.map_err(SshError::sftp)?;
    if !meta.is_dir() {
        return sftp.remove_file(path.to_owned()).await.map_err(SshError::sftp);
    }
    let mut stack = vec![(path.to_owned(), false)];
    while let Some((dir, visited)) = stack.pop() {
        if visited {
            sftp.remove_dir(dir).await.map_err(SshError::sftp)?;
            continue;
        }
        stack.push((dir.clone(), true));
        for e in sftp.read_dir(dir.clone()).await.map_err(SshError::sftp)? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = remote_join(&dir, &name);
            if e.metadata().is_dir() {
                stack.push((child, false));
            } else {
                sftp.remove_file(child).await.map_err(SshError::sftp)?;
            }
        }
    }
    Ok(())
}

fn refuse_root(path: &str) -> Result<()> {
    let p = path.trim();
    if p.is_empty() || p == "/" || p == "~" || p == "." || Path::new(p).parent().is_none() {
        return Err(SshError::text(ErrorKind::Invalid, "refuse-root", &[("path", p.to_owned())]));
    }
    Ok(())
}

impl Manager {
    async fn sftp_for(&self, host: &str) -> Result<std::sync::Arc<SftpSession>> {
        self.session(host, Interaction::Ask).await?.sftp().await
    }

    /// A folder's entries, folders first; `~` or "" is the home folder.
    pub async fn list(&self, side: &Side, path: &str) -> Result<Listing> {
        match side {
            Side::Local => {
                let (path, home) = (path.to_owned(), self.ctx.config.home.clone());
                tokio::task::spawn_blocking(move || list_local(&path, &home)).await.map_err(|e| SshError::new(ErrorKind::Io, e.to_string()))?
            }
            Side::Remote(host) => list_remote(&*self.sftp_for(host).await?, path).await,
        }
    }

    /// Makes `dir/name`; returns its path.
    pub async fn mkdir(&self, side: &Side, dir: &str, name: &str) -> Result<String> {
        check_name(name)?;
        match side {
            Side::Local => {
                let path = Path::new(dir).join(name);
                tokio::fs::create_dir(&path).await.map_err(SshError::io)?;
                Ok(path.to_string_lossy().into_owned())
            }
            Side::Remote(host) => {
                let path = remote_join(dir, name);
                self.sftp_for(host).await?.create_dir(path.clone()).await.map_err(SshError::sftp)?;
                Ok(path)
            }
        }
    }

    /// Renames within the same folder; returns the new path.
    pub async fn rename(&self, side: &Side, path: &str, new_name: &str) -> Result<String> {
        check_name(new_name)?;
        refuse_root(path)?;
        match side {
            Side::Local => {
                let from = PathBuf::from(path);
                let to = from.with_file_name(new_name);
                if tokio::fs::symlink_metadata(&to).await.is_ok() {
                    return Err(SshError::text(ErrorKind::Invalid, "exists", &[("name", new_name.to_owned())]));
                }
                tokio::fs::rename(&from, &to).await.map_err(SshError::io)?;
                Ok(to.to_string_lossy().into_owned())
            }
            Side::Remote(host) => {
                let to = remote_join(&remote_parent(path).unwrap_or_default(), new_name);
                let sftp = self.sftp_for(host).await?;
                if sftp.try_exists(to.clone()).await.unwrap_or(false) {
                    return Err(SshError::text(ErrorKind::Invalid, "exists", &[("name", new_name.to_owned())]));
                }
                sftp.rename(path.to_owned(), to.clone()).await.map_err(SshError::sftp)?;
                Ok(to)
            }
        }
    }

    /// Deletes a file, or a folder with everything in it.
    pub async fn remove(&self, side: &Side, path: &str) -> Result<()> {
        refuse_root(path)?;
        match side {
            Side::Local => {
                let p = PathBuf::from(path);
                let meta = tokio::fs::symlink_metadata(&p).await.map_err(SshError::io)?;
                if meta.is_dir() { tokio::fs::remove_dir_all(&p).await } else { tokio::fs::remove_file(&p).await }.map_err(SshError::io)
            }
            Side::Remote(host) => remove_remote(&*self.sftp_for(host).await?, path).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_join_and_climb() {
        assert_eq!(remote_join("/srv", "a"), "/srv/a");
        assert_eq!(remote_join("/", "a"), "/a");
        assert_eq!(remote_parent("/srv/yuzora/"), Some("/srv".into()));
        assert_eq!(remote_parent("/srv"), Some("/".into()));
        assert_eq!(remote_parent("/"), None);
        assert_eq!(base_name("/srv/releases/"), "releases");
        assert_eq!(base_name("C:\\Users\\a.txt"), "a.txt");
    }

    #[test]
    fn names_are_one_segment_and_roots_are_never_removed() {
        assert!(check_name("ok.txt").is_ok());
        for bad in ["", ".", "..", "a/b", "a\0"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        for root in ["/", "", "~", "."] {
            assert!(refuse_root(root).is_err(), "{root:?}");
        }
        assert!(refuse_root("/srv/x").is_ok());
    }

    #[test]
    fn local_listing_sorts_folders_first_and_marks_hidden() {
        let dir = std::env::temp_dir().join(format!("yuzora-ssh-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::write(dir.join("b.txt"), "hello").unwrap();
        std::fs::write(dir.join(".env"), "").unwrap();
        std::fs::write(dir.join("A.md"), "").unwrap();
        let l = list_local(dir.to_str().unwrap(), &dir).unwrap();
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Zeta", ".env", "A.md", "b.txt"]);
        assert!(l.entries[1].hidden);
        assert_eq!(l.entries[3].size, 5);
        assert_eq!(l.entries[0].kind, "dir");
        assert_eq!(list_local("~", &dir).unwrap().path, l.path);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
