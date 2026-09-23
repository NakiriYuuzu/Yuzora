//! Bounded, descriptor-relative directory operations for manual tree transfers.
use super::*;
use std::io;

impl PinnedDir {
    pub fn list_entries(&self, limit: usize) -> Result<Vec<(String, NodeKind)>, String> {
        #[cfg(unix)]
        let mut entries: Vec<_> = self.entries()?.take(limit + 1).collect::<Result<_, _>>()?;
        #[cfg(windows)]
        let mut entries = self.windows_entries(limit)?;
        if entries.len() > limit {
            return Err("sftp-tree-entry-limit".into());
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    #[cfg(windows)]
    pub fn open_subdir(&self, path: &str) -> Result<Self, PathCapabilityError> {
        let mut handle = self
            .handle
            .try_clone()
            .map_err(|_| PathCapabilityError::Io)?;
        if !path.is_empty() {
            for name in SafeRelativePath::parse(path)?.components() {
                let file = win_at::open_relative(
                    &handle,
                    name.as_str(),
                    win_at::RelativeKind::Directory,
                    win_at::RelativeMode::Open,
                )?;
                reject_reparse_handle(&file)?;
                handle = file.into();
            }
        }
        let file = File::from(handle.try_clone().map_err(|_| PathCapabilityError::Io)?);
        let id = file_id(&file)?;
        Ok(Self { handle, id })
    }

    #[cfg(windows)]
    pub fn mkdir(&self, name: &SafeLeafName) -> Result<(), String> {
        let file = win_at::open_relative(
            &self.handle,
            name.as_str(),
            win_at::RelativeKind::Directory,
            win_at::RelativeMode::CreateNew,
        )?;
        reject_reparse_handle(&file)?;
        Ok(())
    }

    #[cfg(windows)]
    pub fn rename_new(
        &self,
        from: &SafeLeafName,
        destination: &Self,
        to: &SafeLeafName,
    ) -> Result<(), String> {
        let file = win_at::open_relative(
            &self.handle,
            from.as_str(),
            win_at::RelativeKind::Any,
            win_at::RelativeMode::OpenDelete,
        )?;
        reject_reparse_handle(&file)?;
        win_at::rename_relative(&file, &destination.handle, to.as_str(), false)
            .map_err(String::from)
    }

    pub fn remove_empty_dir(&self, name: &SafeLeafName) -> Result<(), String> {
        #[cfg(unix)]
        {
            let name = to_cstring(Path::new(name.as_str()))?;
            if unsafe { libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                != 0
            {
                return Err(io::Error::last_os_error().to_string());
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let file = win_at::open_relative(
                &self.handle,
                name.as_str(),
                win_at::RelativeKind::Directory,
                win_at::RelativeMode::OpenDelete,
            )?;
            reject_reparse_handle(&file)?;
            win_at::delete_on_close(&file).map_err(String::from)
        }
    }

    #[cfg(windows)]
    pub fn remove_tree(
        &self,
        name: &SafeLeafName,
        budget: &mut usize,
        deadline: Instant,
    ) -> Result<(), String> {
        self.remove_tree_depth(name, budget, deadline, 0)
    }

    #[cfg(windows)]
    fn remove_tree_depth(
        &self,
        name: &SafeLeafName,
        budget: &mut usize,
        deadline: Instant,
        depth: usize,
    ) -> Result<(), String> {
        if *budget == 0 || depth >= 128 || Instant::now() >= deadline {
            return Err("delete-limit-reached-partial".into());
        }
        *budget -= 1;
        if self.existing_kind(name)? == Some(NodeKind::Directory) {
            let child = self.open_subdir(name.as_str())?;
            for (entry, _) in child.list_entries(*budget)? {
                child.remove_tree_depth(
                    &SafeLeafName::parse(&entry)?,
                    budget,
                    deadline,
                    depth + 1,
                )?;
            }
            if self.open_subdir(name.as_str())?.id_key() != child.id_key() {
                return Err("directory-changed-during-delete".into());
            }
            self.remove_empty_dir(name)
        } else {
            // unlink opens the child with FILE_OPEN_REPARSE_POINT and refuses
            // reparse handles; a junction can never lead traversal outside root.
            self.unlink(name).map_err(String::from)
        }
    }

    #[cfg(windows)]
    fn windows_entries(&self, limit: usize) -> Result<Vec<(String, NodeKind)>, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
            FILE_ID_BOTH_DIR_INFO,
        };
        let mut entries = Vec::new();
        // u64 storage guarantees the native record's alignment.
        let mut buffer = vec![0u64; 8192];
        let bytes = buffer.len() * 8;
        let mut class = FileIdBothDirectoryRestartInfo;
        loop {
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.handle.as_raw_handle(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    bytes as u32,
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(18) {
                    break;
                } // ERROR_NO_MORE_FILES
                return Err(error.to_string());
            }
            class = FileIdBothDirectoryInfo;
            let mut offset = 0;
            loop {
                if offset + std::mem::size_of::<FILE_ID_BOTH_DIR_INFO>() > bytes {
                    return Err("directory-record-invalid".into());
                }
                let pointer = unsafe { buffer.as_ptr().cast::<u8>().add(offset) };
                let info =
                    unsafe { std::ptr::read_unaligned(pointer.cast::<FILE_ID_BOTH_DIR_INFO>()) };
                let start = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let len = info.FileNameLength as usize;
                if !len.is_multiple_of(2) || offset + start + len > bytes {
                    return Err("directory-record-invalid".into());
                }
                let units = unsafe {
                    std::slice::from_raw_parts(pointer.add(start).cast::<u16>(), len / 2)
                };
                let name = String::from_utf16(units).map_err(|_| "path-not-utf8")?;
                if name != "." && name != ".." {
                    let leaf = SafeLeafName::parse(&name)?;
                    if let Some(kind) = self.existing_kind(&leaf)? {
                        entries.push((name, kind));
                    }
                    if entries.len() > limit {
                        return Err("sftp-tree-entry-limit".into());
                    }
                }
                let next = info.NextEntryOffset as usize;
                if next == 0 {
                    break;
                }
                if next < start + len || !next.is_multiple_of(8) {
                    return Err("directory-record-invalid".into());
                }
                offset = offset.checked_add(next).ok_or("directory-record-invalid")?;
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn files_can_be_renamed_and_nested_trees_deleted_with_a_budget() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tree/sub")).unwrap();
        std::fs::write(tmp.path().join("tree/sub/file.txt"), b"content").unwrap();
        let root = PinnedDir::open_dir(tmp.path()).unwrap();
        let sub = root.open_subdir("tree/sub").unwrap();
        sub.rename_new(
            &SafeLeafName::parse("file.txt").unwrap(),
            &sub,
            &SafeLeafName::parse("renamed.txt").unwrap(),
        )
        .unwrap();
        assert_eq!(
            sub.list_entries(10).unwrap(),
            vec![("renamed.txt".into(), NodeKind::File)]
        );
        drop(sub);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        assert!(root
            .remove_tree(&SafeLeafName::parse("tree").unwrap(), &mut 0, deadline)
            .is_err());
        assert!(tmp.path().join("tree/sub/renamed.txt").exists());
        root.remove_tree(&SafeLeafName::parse("tree").unwrap(), &mut 10, deadline)
            .unwrap();
        assert!(!tmp.path().join("tree").exists());
    }
    #[test]
    fn tree_operations_are_bounded_and_never_replace_a_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PinnedDir::open_dir(tmp.path()).unwrap();
        let staging = SafeLeafName::parse("staging").unwrap();
        root.mkdir(&staging).unwrap();
        root.mkdir(&SafeLeafName::parse("existing").unwrap())
            .unwrap();
        assert!(root.list_entries(1).is_err());
        assert!(root
            .rename_new(&staging, &root, &SafeLeafName::parse("existing").unwrap())
            .is_err());
        root.rename_new(&staging, &root, &SafeLeafName::parse("new").unwrap())
            .unwrap();
        root.remove_empty_dir(&SafeLeafName::parse("new").unwrap())
            .unwrap();
        assert_eq!(root.list_entries(5).unwrap().len(), 1);
    }
}
