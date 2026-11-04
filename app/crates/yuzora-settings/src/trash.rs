//! Moving a file to the system's trash, where the person can put it back.
//!
//! - macOS: `/usr/bin/trash` (macOS 15 and later), else Finder through osascript.
//! - Windows: the Recycle Bin through PowerShell (`Microsoft.VisualBasic`).
//! - Linux and the rest: the freedesktop trash (`$XDG_DATA_HOME/Trash`), by hand.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn trash(path: &Path) -> std::io::Result<()> {
    platform(&absolute(path)?)
}

/// `path` with its folder resolved but not the file itself: a link goes to the
/// trash as the link (as `mv` would), never the file it points at.
fn absolute(path: &Path) -> std::io::Result<std::path::PathBuf> {
    if std::fs::symlink_metadata(path).is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} is gone", path.display())));
    }
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(folder)?.join(name))
}

fn ran(cmd: &mut Command) -> std::io::Result<()> {
    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned()))
    }
}

#[cfg(target_os = "macos")]
fn platform(path: &Path) -> std::io::Result<()> {
    if Path::new("/usr/bin/trash").exists() {
        return ran(Command::new("/usr/bin/trash").arg(path));
    }
    ran(Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv", "-e", "tell application \"Finder\" to delete (POSIX file (item 1 of argv) as alias)", "-e", "end run"])
        .arg(path))
}

#[cfg(windows)]
fn platform(path: &Path) -> std::io::Result<()> {
    // The path goes through the environment, never into the script text.
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg("Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($env:YUZORA_TRASH_PATH, 'OnlyErrorDialogs', 'SendToRecycleBin')")
        .env("YUZORA_TRASH_PATH", path);
    // No console on Windows: without this powershell opens a window.
    yuzora_core::spawn::no_window(&mut cmd);
    ran(&mut cmd)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform(path: &Path) -> std::io::Result<()> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| yuzora_core::paths::home_dir().join(".local/share"));
    freedesktop(path, &data.join("Trash"))
}

/// The freedesktop.org trash: the file into `files/`, where it came from and
/// when into `info/<name>.trashinfo` (written first, as the spec asks).
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
pub fn freedesktop(path: &Path, trash: &Path) -> std::io::Result<()> {
    let files = trash.join("files");
    let info = trash.join("info");
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let base = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| std::io::Error::other("no file name"))?;
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (base.to_owned(), String::new()),
    };
    let mut name = base.to_owned();
    let mut i = 1;
    let record = loop {
        let record = info.join(format!("{name}.trashinfo"));
        if std::fs::symlink_metadata(files.join(&name)).is_err() {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&record) {
                Ok(mut f) => {
                    use std::io::Write;
                    write!(f, "[Trash Info]\nPath={}\nDeletionDate={}\n", encode_path(path), deletion_date(SystemTime::now()))?;
                    break record;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        i += 1;
        name = format!("{stem}.{i}{ext}");
    };
    if let Err(e) = move_file(path, &files.join(&name)) {
        let _ = std::fs::remove_file(record);
        return Err(e);
    }
    Ok(())
}

/// A rename, or across filesystems (the config and data homes on different
/// mounts: EXDEV) a copy of the file or link, then removing the original.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => copy_then_remove(from, to),
        done => done,
    }
}

fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
        #[cfg(not(unix))]
        return Err(std::io::Error::other("a link on another filesystem"));
    } else if meta.is_file() {
        std::fs::copy(from, to)?;
    } else {
        return Err(std::io::Error::other("not a file"));
    }
    if let Err(e) = std::fs::remove_file(from) {
        let _ = std::fs::remove_file(to);
        return Err(e);
    }
    Ok(())
}

/// The path as a URL path: bytes outside the unreserved set and `/` escaped.
fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `YYYY-MM-DDThh:mm:ss` (UTC: the spec's local time needs a time zone table).
fn deletion_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dates_and_paths_are_written_as_the_spec_wants() {
        assert_eq!(deletion_date(UNIX_EPOCH), "1970-01-01T00:00:00");
        assert_eq!(deletion_date(UNIX_EPOCH + Duration::from_secs(1_793_719_425)), "2026-11-03T15:23:45");
        assert_eq!(deletion_date(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00");
        assert_eq!(encode_path(Path::new("/home/p/a b/字.md")), "/home/p/a%20b/%E5%AD%97.md");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_goes_to_the_trash_as_the_link() {
        let d = std::env::temp_dir().join(format!("yuzora-trash-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("t")).unwrap();
        std::fs::write(d.join("real.md"), "x").unwrap();
        std::os::unix::fs::symlink(d.join("real.md"), d.join("t/rev.md")).unwrap();
        let at = absolute(&d.join("t/rev.md")).unwrap();
        assert_eq!(at, std::fs::canonicalize(d.join("t")).unwrap().join("rev.md"), "the link's own path");
        freedesktop(&at, &d.join("Trash")).unwrap();
        assert!(std::fs::symlink_metadata(d.join("Trash/files/rev.md")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(d.join("real.md")).unwrap(), "x", "the file it pointed at stays");
        std::fs::remove_file(d.join("real.md")).unwrap();
        assert!(absolute(&d.join("Trash/files/rev.md")).is_ok(), "a broken link is still there to trash");
        assert_eq!(absolute(&d.join("none.md")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// What a move across filesystems does in place of the rename.
    #[cfg(unix)]
    #[test]
    fn a_copy_then_remove_moves_files_and_links() {
        let d = std::env::temp_dir().join(format!("yuzora-trash-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("to")).unwrap();
        std::fs::write(d.join("a.md"), "x").unwrap();
        copy_then_remove(&d.join("a.md"), &d.join("to/a.md")).unwrap();
        assert!(!d.join("a.md").exists());
        assert_eq!(std::fs::read_to_string(d.join("to/a.md")).unwrap(), "x");
        std::os::unix::fs::symlink(d.join("to/a.md"), d.join("l.md")).unwrap();
        copy_then_remove(&d.join("l.md"), &d.join("to/l.md")).unwrap();
        assert!(std::fs::symlink_metadata(d.join("l.md")).is_err());
        assert_eq!(std::fs::read_link(d.join("to/l.md")).unwrap(), d.join("to/a.md"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn freedesktop_trash_keeps_both_of_two_same_names() {
        let d = std::env::temp_dir().join(format!("yuzora-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::write(d.join("a/rev.md"), "one").unwrap();
        std::fs::write(d.join("b/rev.md"), "two").unwrap();
        let trash = d.join("Trash");
        freedesktop(&d.join("a/rev.md"), &trash).unwrap();
        freedesktop(&d.join("b/rev.md"), &trash).unwrap();
        assert!(!d.join("a/rev.md").exists() && !d.join("b/rev.md").exists());
        assert_eq!(std::fs::read_to_string(trash.join("files/rev.md")).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(trash.join("files/rev.2.md")).unwrap(), "two");
        let info = std::fs::read_to_string(trash.join("info/rev.2.md.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=") && info.contains("/b/rev.md\nDeletionDate="), "{info}");
        assert!(freedesktop(&d.join("a/missing.md"), &trash).is_err());
        assert!(!trash.join("info/missing.md.trashinfo").exists(), "no record for a failed move");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
