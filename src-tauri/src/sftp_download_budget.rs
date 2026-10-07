//! Per-volume reservations for app-owned downloads. The kernel still arbitrates
//! external writers and quotas; every I/O error leaves the destination untouched.
use std::collections::HashMap;
use std::fs::File;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RESERVE: u64 = 256 * 1024 * 1024;
const RECHECK_BYTES: u64 = 8 * 1024 * 1024;
static VOLUMES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

pub(crate) struct DownloadBudget {
    file: File,
    volume: String,
    remaining: u64,
    since_check: u64,
    checked_at: Instant,
}

fn reserve_bytes(current: u64, additional: u64, available: u64) -> Result<u64, String> {
    current
        .checked_add(additional)
        .filter(|total| {
            total
                .checked_add(RESERVE)
                .is_some_and(|needed| needed <= available)
        })
        .ok_or_else(|| "sftp-insufficient-space".into())
}

impl DownloadBudget {
    pub(crate) fn acquire(file: &File, expected: u64) -> Result<Self, String> {
        let file = file.try_clone().map_err(|_| "sftp-space-unavailable")?;
        let mut volumes = VOLUMES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "sftp-space-unavailable")?;
        let (volume, available) = space(&file)?;
        let total = reserve_bytes(*volumes.get(&volume).unwrap_or(&0), expected, available)?;
        volumes.insert(volume.clone(), total);
        Ok(Self {
            file,
            volume,
            remaining: expected,
            since_check: 0,
            checked_at: Instant::now(),
        })
    }

    pub(crate) fn before_write(&mut self, bytes: usize) -> Result<(), String> {
        if bytes as u64 > self.remaining {
            return Err("sftp-size-mismatch".into());
        }
        if self.since_check >= RECHECK_BYTES || self.checked_at.elapsed() >= Duration::from_secs(1)
        {
            let volumes = VOLUMES
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| "sftp-space-unavailable")?;
            let (volume, available) = space(&self.file)?;
            if volume != self.volume {
                return Err("sftp-space-unavailable".into());
            }
            reserve_bytes(*volumes.get(&volume).unwrap_or(&0), 0, available)?;
            self.checked_at = Instant::now();
            self.since_check = 0;
        }
        Ok(())
    }

    pub(crate) fn written(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        self.remaining -= bytes;
        self.since_check += bytes;
        if let Ok(mut volumes) = VOLUMES.get_or_init(Default::default).lock() {
            if let Some(total) = volumes.get_mut(&self.volume) {
                *total -= bytes;
            }
        }
    }

    pub(crate) fn finish(&self) -> Result<(), String> {
        if self.remaining == 0 {
            Ok(())
        } else {
            Err("sftp-size-mismatch".into())
        }
    }
}

impl Drop for DownloadBudget {
    fn drop(&mut self) {
        if let Ok(mut volumes) = VOLUMES.get_or_init(Default::default).lock() {
            if let Some(total) = volumes.get_mut(&self.volume) {
                *total -= self.remaining;
                if *total == 0 {
                    volumes.remove(&self.volume);
                }
            }
        }
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // statvfs integer widths vary across Unix targets.
fn space(file: &File) -> Result<(String, u64), String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err("sftp-space-unavailable".into());
    }
    let stats = unsafe { stats.assume_init() };
    let available = (stats.f_bavail as u64)
        .checked_mul(stats.f_frsize as u64)
        .ok_or("sftp-space-unavailable")?;
    let device = file.metadata().map_err(|_| "sftp-space-unavailable")?.dev();
    Ok((device.to_string(), available))
}

#[cfg(any(windows, test))]
fn windows_volume_root(path: &str) -> Option<String> {
    if path.starts_with(r"\\?\Volume{") {
        return path.find(r"}\").map(|end| path[..end + 2].to_owned());
    }
    let unc = path
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| path.strip_prefix(r"\\"))?;
    let mut parts = unc.split('\\');
    let server = parts
        .next()
        .filter(|part| !part.is_empty() && *part != "?")?;
    let share = parts.next().filter(|part| !part.is_empty())?;
    Some(format!(r"\\{server}\{share}\"))
}

#[cfg(windows)]
fn space(file: &File) -> Result<(String, u64), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetFinalPathNameByHandleW, VOLUME_NAME_GUID,
    };
    let mut path = vec![0u16; 32768];
    let mut len = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            path.as_mut_ptr(),
            path.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    // SMB shares have no local volume GUID. Resolve their actual UNC root from
    // the opened handle; never query a frontend-supplied or stale pathname.
    if len == 0 {
        len = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                0,
            )
        } as usize;
    }
    if len == 0 || len >= path.len() {
        return Err("sftp-space-unavailable".into());
    }
    let path = String::from_utf16(&path[..len]).map_err(|_| "sftp-space-unavailable")?;
    let volume = windows_volume_root(&path).ok_or("sftp-space-unavailable")?;
    let wide: Vec<u16> = volume.encode_utf16().chain(Some(0)).collect();
    let mut available = 0u64;
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err("sftp-space-unavailable".into());
    }
    let (serial, _, _) = crate::path_capability::windows_file_identity(file)
        .map_err(|_| "sftp-space-unavailable")?;
    // Handles on aliases of one volume share its reservation. Serial collisions
    // only make admission more conservative, never increase the write budget.
    Ok((format!("windows-volume-{serial}"), available))
}

#[cfg(not(any(unix, windows)))]
fn space(_: &File) -> Result<(String, u64), String> {
    Err("sftp-space-unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_roots_support_guid_and_network_share_aliases() {
        assert_eq!(
            windows_volume_root(r"\\?\Volume{abc}\folder\temp"),
            Some(r"\\?\Volume{abc}\".into())
        );
        assert_eq!(
            windows_volume_root(r"\\?\UNC\server\share\folder\temp"),
            Some(r"\\server\share\".into())
        );
        assert_eq!(
            windows_volume_root(r"\\server\share\temp"),
            Some(r"\\server\share\".into())
        );
        assert_eq!(windows_volume_root(r"\\server"), None);
    }
    #[test]
    fn concurrent_reservations_preserve_headroom_and_reject_overflow() {
        let first = reserve_bytes(0, 100, RESERVE + 150).unwrap();
        assert!(reserve_bytes(first, 51, RESERVE + 150).is_err());
        assert_eq!(reserve_bytes(first, 50, RESERVE + 150).unwrap(), 150);
        assert!(reserve_bytes(u64::MAX, 1, u64::MAX).is_err());
    }
    #[test]
    fn exact_size_is_checked_before_writes_and_at_eof() {
        let file = tempfile::tempfile().unwrap();
        let mut budget = DownloadBudget::acquire(&file, 3).unwrap();
        assert!(budget.before_write(4).is_err());
        assert!(budget.finish().is_err());
        budget.before_write(3).unwrap();
        budget.written(3);
        budget.finish().unwrap();
        assert!(budget.before_write(1).is_err());
    }
}
