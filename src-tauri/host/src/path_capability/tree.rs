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

    /// Search-only listing: bounded cancellation and an independent scan cursor.
    /// FilesList/SFTP retain the existing list_entries behavior.
    pub(crate) fn list_entries_until(
        &self,
        limit: usize,
        mut stop: impl FnMut() -> bool,
    ) -> Result<Vec<(String, NodeKind)>, String> {
        if stop() {
            return Err("directory-listing-stopped".into());
        }
        #[cfg(unix)]
        let mut entries = {
            let mut cursor = self.entries()?;
            let mut entries = Vec::new();
            while let Some(entry) = cursor.next_until(&mut stop) {
                entries.push(entry?);
                if entries.len() > limit {
                    return Err("sftp-tree-entry-limit".into());
                }
            }
            entries
        };
        #[cfg(windows)]
        let mut entries = self
            .independent_cursor()?
            .windows_entries_until(limit, &mut stop)?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    #[cfg(windows)]
    fn independent_cursor(&self) -> Result<Self, PathCapabilityError> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            ReOpenFile, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        // DuplicateHandle shares the directory cursor. ReOpenFile creates a new
        // file object from this handle, without resolving its former pathname.
        let handle = unsafe {
            ReOpenFile(
                self.handle.as_raw_handle(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(PathCapabilityError::Io);
        }
        let file = unsafe { File::from_raw_handle(handle) };
        reject_reparse_handle(&file)?;
        let id = file_id(&file)?;
        if id != self.id {
            return Err(PathCapabilityError::Io);
        }
        Ok(Self {
            handle: file.into(),
            id,
        })
    }

    /// Probe the canonical spelling relative to the pinned directory, so the
    /// filesystem (not a case-folded listing) determines ignore-file discovery.
    pub(crate) fn open_file_optional(
        &self,
        name: &SafeLeafName,
    ) -> Result<Option<OpenedFile>, PathCapabilityError> {
        match self.existing_kind(name)? {
            None => Ok(None),
            Some(NodeKind::File) => self
                .open_file(&SafeRelativePath::parse(name.as_str())?)
                .map(Some),
            Some(_) => Err(PathCapabilityError::NotARegularFile),
        }
    }

    pub(crate) fn open_subdir_optional(
        &self,
        name: &SafeLeafName,
    ) -> Result<Option<Self>, PathCapabilityError> {
        // A linked-worktree .git file or symlink is not an authorized directory.
        if self.existing_kind(name)? == Some(NodeKind::Directory) {
            self.open_subdir(name.as_str()).map(Some)
        } else {
            Ok(None)
        }
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

    /// Native file-tree rename preserves the selected final link itself. This
    /// never follows its reparse target; other consumers keep rename_new's gate.
    pub fn rename_entry_new(
        &self,
        from: &SafeLeafName,
        destination: &Self,
        to: &SafeLeafName,
    ) -> Result<(), String> {
        #[cfg(unix)]
        {
            self.rename_new(from, destination, to)
        }
        #[cfg(windows)]
        {
            let file = win_at::open_relative(
                &self.handle,
                from.as_str(),
                win_at::RelativeKind::Any,
                win_at::RelativeMode::OpenDelete,
            )?;
            win_at::rename_relative(&file, &destination.handle, to.as_str(), false)
                .map_err(String::from)
        }
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

    /// The shared budget bounds partial deletions and the deadline bounds how
    /// long one call may run; errors report that work may be partial.
    pub fn remove_tree(
        &self,
        name: &SafeLeafName,
        budget: &mut usize,
        deadline: Instant,
    ) -> Result<(), String> {
        self.remove_tree_with(name, &mut || {
            if *budget == 0 || Instant::now() >= deadline {
                return Err("delete-limit-reached-partial".into());
            }
            *budget -= 1;
            Ok(())
        })
    }

    /// Counts `name` and every entry below it without following links, so a
    /// delete can report progress. `stop` sees the running count and may end
    /// the walk; more than `limit` entries is an error.
    pub fn count_tree(
        &self,
        name: &SafeLeafName,
        limit: usize,
        stop: &mut dyn FnMut(usize) -> bool,
    ) -> Result<usize, String> {
        let mut count = 0;
        let kind = self.existing_kind(name)?;
        self.count_tree_depth(name, kind, limit, stop, &mut count, 0)?;
        Ok(count)
    }

    fn count_tree_depth(
        &self,
        name: &SafeLeafName,
        kind: Option<NodeKind>,
        limit: usize,
        stop: &mut dyn FnMut(usize) -> bool,
        count: &mut usize,
        depth: usize,
    ) -> Result<(), String> {
        if stop(*count) {
            return Err("delete-cancelled".into());
        }
        if depth >= 128 || *count >= limit {
            return Err("delete-limit-reached".into());
        }
        *count += 1;
        if kind == Some(NodeKind::Directory) {
            let child = self.open_subdir(name.as_str())?;
            let entries = child.list_entries(limit - *count).map_err(|error| {
                if error == "sftp-tree-entry-limit" {
                    "delete-limit-reached".to_string()
                } else {
                    error
                }
            })?;
            for (entry, entry_kind) in entries {
                child.count_tree_depth(
                    &SafeLeafName::parse(&entry)?,
                    Some(entry_kind),
                    limit,
                    stop,
                    count,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn remove_tree_with(
        &self,
        name: &SafeLeafName,
        tick: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        self.remove_tree_depth(name, tick, 0)
    }

    #[cfg(windows)]
    fn remove_tree_depth(
        &self,
        name: &SafeLeafName,
        tick: &mut dyn FnMut() -> Result<(), String>,
        depth: usize,
    ) -> Result<(), String> {
        // Listing is materialized on Windows; `tick` still bounds the delete.
        const LISTING_LIMIT: usize = 1_000_000;
        if depth >= 128 {
            return Err("delete-limit-reached-partial".into());
        }
        tick()?;
        let kind = self.existing_kind(name)?;
        if kind == Some(NodeKind::Directory) {
            let child = self.open_subdir(name.as_str())?;
            for (entry, _) in child.list_entries(LISTING_LIMIT)? {
                child.remove_tree_depth(&SafeLeafName::parse(&entry)?, tick, depth + 1)?;
            }
            if self.open_subdir(name.as_str())?.id_key() != child.id_key() {
                return Err("directory-changed-during-delete".into());
            }
            self.remove_empty_dir(name)
        } else if kind == Some(NodeKind::Symlink) {
            // FILE_OPEN_REPARSE_POINT opens the link or junction itself, so the
            // delete removes that entry and never reaches its target.
            let link = win_at::open_relative(
                &self.handle,
                name.as_str(),
                win_at::RelativeKind::Any,
                win_at::RelativeMode::OpenDelete,
            )?;
            if !is_reparse(&link.metadata().map_err(|_| PathCapabilityError::Io)?) {
                return Err("entry-changed-during-delete".into());
            }
            win_at::delete_on_close(&link).map_err(String::from)
        } else {
            self.unlink(name).map_err(String::from)
        }
    }

    #[cfg(windows)]
    fn windows_entries(&self, limit: usize) -> Result<Vec<(String, NodeKind)>, String> {
        self.windows_entries_until(limit, &mut || false)
    }

    #[cfg(windows)]
    fn windows_entries_until(
        &self,
        limit: usize,
        stop: &mut impl FnMut() -> bool,
    ) -> Result<Vec<(String, NodeKind)>, String> {
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
            if stop() {
                return Err("directory-listing-stopped".into());
            }
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
                if stop() {
                    return Err("directory-listing-stopped".into());
                }
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
    fn trees_are_counted_and_a_tick_can_stop_the_delete() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tree/a/b")).unwrap();
        for file in ["tree/1.txt", "tree/a/2.txt", "tree/a/b/3.txt"] {
            std::fs::write(tmp.path().join(file), b"x").unwrap();
        }
        let root = PinnedDir::open_dir(tmp.path()).unwrap();
        let tree = SafeLeafName::parse("tree").unwrap();
        // tree, a, b and three files.
        assert_eq!(root.count_tree(&tree, 100, &mut |_| false).unwrap(), 6);
        assert_eq!(
            root.count_tree(&tree, 3, &mut |_| false).unwrap_err(),
            "delete-limit-reached"
        );
        assert_eq!(
            root.count_tree(&tree, 100, &mut |seen| seen == 2)
                .unwrap_err(),
            "delete-cancelled"
        );

        let mut ticks = 0;
        let stopped = root.remove_tree_with(&tree, &mut || {
            ticks += 1;
            if ticks > 3 {
                return Err("stop".into());
            }
            Ok(())
        });
        assert_eq!(stopped.unwrap_err(), "stop");
        assert!(tmp.path().join("tree").exists());
        root.remove_tree_with(&tree, &mut || Ok(())).unwrap();
        assert!(!tmp.path().join("tree").exists());
    }

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
    fn tree_delete_removes_links_without_touching_their_targets() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target_dir = outside.path().join("dir");
        let target_file = outside.path().join("file.txt");
        std::fs::create_dir(&target_dir).unwrap();
        std::fs::write(target_dir.join("keep.txt"), b"keep").unwrap();
        std::fs::write(&target_file, b"keep").unwrap();
        let tree = workspace.path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        #[cfg(unix)]
        for (target, link) in [(&target_dir, "dir-link"), (&target_file, "file-link")] {
            std::os::unix::fs::symlink(target, tree.join(link)).unwrap();
        }
        #[cfg(windows)]
        {
            // A junction needs no symlink privilege, so unelevated users create them.
            let junction = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(tree.join("dir-link"))
                .arg(&target_dir)
                .output()
                .unwrap();
            assert!(junction.status.success(), "{junction:?}");
            std::os::windows::fs::symlink_file(&target_file, tree.join("file-link")).unwrap();
        }
        let root = PinnedDir::open_dir(workspace.path()).unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        root.open_subdir("tree")
            .unwrap()
            .remove_tree(&SafeLeafName::parse("dir-link").unwrap(), &mut 10, deadline)
            .unwrap();
        assert!(std::fs::symlink_metadata(tree.join("dir-link")).is_err());
        root.remove_tree(&SafeLeafName::parse("tree").unwrap(), &mut 10, deadline)
            .unwrap();
        assert!(!tree.exists());
        assert_eq!(std::fs::read(target_dir.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(&target_file).unwrap(), b"keep");
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
