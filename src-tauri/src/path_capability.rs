//! Descriptor/handle-relative path operations for F4/F5 and the F2 preview
//! allowlist-to-read gap.
//!
//! Unix walks every component with `openat(..., O_NOFOLLOW)` from a pinned
//! directory descriptor. Windows keeps a directory handle and opens each
//! component with `NtCreateFile(..., RootDirectory)` plus
//! `FILE_OPEN_REPARSE_POINT`, then rejects reparse handles. Non-UTF-8
//! operational paths fail closed.

use std::collections::HashMap;
use std::fs::{File, FileType};
#[cfg(unix)]
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

pub const SELECTED_PATH_TTL: Duration = Duration::from_secs(60);
pub const DOWNLOAD_DEST_TTL: Duration = Duration::from_secs(60);

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCapabilityError {
    NotUtf8,
    UnsafeLeaf,
    UnsafeRelativePath,
    NotADirectory,
    NotARegularFile,
    SymlinkRejected,
    ReparseRejected,
    Escape,
    DestinationBusy,
    CapabilityExpired,
    CapabilityMissing,
    WorkspaceCapabilityMissing,
    DownloadCapabilityExpired,
    DownloadCapabilityMissing,
    Io,
}

impl PathCapabilityError {
    pub fn as_code(self) -> &'static str {
        match self {
            Self::NotUtf8 => "path-not-utf8",
            Self::UnsafeLeaf => "unsafe-leaf-name",
            Self::UnsafeRelativePath => "unsafe-relative-path",
            Self::NotADirectory => "not-a-directory",
            Self::NotARegularFile => "not-a-regular-file",
            Self::SymlinkRejected => "symlink-rejected",
            Self::ReparseRejected => "reparse-rejected",
            Self::Escape => "path-escape",
            Self::DestinationBusy => "destination-busy",
            Self::CapabilityExpired => "selected-path-expired",
            Self::CapabilityMissing => "selected-path-missing",
            Self::WorkspaceCapabilityMissing => "workspace-capability-missing",
            Self::DownloadCapabilityExpired => "download-destination-capability-expired",
            Self::DownloadCapabilityMissing => "download-destination-capability-missing",
            Self::Io => "path-io-failed",
        }
    }
}

impl From<PathCapabilityError> for String {
    fn from(error: PathCapabilityError) -> Self {
        error.as_code().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeLeafName(String);

impl SafeLeafName {
    pub fn parse(name: &str) -> Result<Self, PathCapabilityError> {
        if !is_safe_leaf_name(name) {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn is_safe_leaf_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.contains('\0') || name.contains('/') || name.contains('\\') {
        return false;
    }
    if name.chars().next().is_some_and(|ch| ch == ':') {
        return false;
    }
    if looks_like_drive_prefix(name) || looks_like_unc_or_verbatim(name) {
        return false;
    }
    if std::path::MAIN_SEPARATOR != '/'
        && std::path::MAIN_SEPARATOR != '\\'
        && name.contains(std::path::MAIN_SEPARATOR)
    {
        return false;
    }
    if cfg!(windows) && !windows_ordinary_leaf(name) {
        return false;
    }
    true
}

fn windows_ordinary_leaf(name: &str) -> bool {
    if name.contains(':') {
        return false;
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return false;
    }
    let stem = name.split_once('.').map(|(head, _)| head).unwrap_or(name);
    if stem.is_empty() {
        return false;
    }
    let stem = stem.to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) == false
}

fn looks_like_drive_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

fn looks_like_unc_or_verbatim(name: &str) -> bool {
    name.starts_with("//")
        || name.starts_with("\\\\")
        || name.starts_with("//?/")
        || name.starts_with("\\\\?\\")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeRelativePath(Vec<SafeLeafName>);

impl SafeRelativePath {
    pub fn parse(path: &str) -> Result<Self, PathCapabilityError> {
        if path.is_empty() || path.contains('\0') {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        if path.starts_with('/')
            || path.starts_with('\\')
            || looks_like_drive_prefix(path)
            || looks_like_unc_or_verbatim(path)
        {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        if path.contains('\\') {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        let mut parts = Vec::new();
        for part in path.split('/') {
            if part.is_empty() {
                return Err(PathCapabilityError::UnsafeRelativePath);
            }
            parts.push(
                SafeLeafName::parse(part).map_err(|_| PathCapabilityError::UnsafeRelativePath)?,
            );
        }
        if parts.is_empty() {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        Ok(Self(parts))
    }

    pub fn components(&self) -> &[SafeLeafName] {
        &self.0
    }

    pub fn leaf(&self) -> &SafeLeafName {
        self.0.last().expect("SafeRelativePath is never empty")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
    Other,
}

pub fn node_kind_from_file_type(file_type: FileType) -> NodeKind {
    if file_type.is_symlink() {
        NodeKind::Symlink
    } else if file_type.is_dir() {
        NodeKind::Directory
    } else if file_type.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileId {
    #[cfg(unix)]
    Unix { dev: u64, ino: u64 },
    #[cfg(windows)]
    Windows { volume: u32, index: u64 },
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

impl FileId {
    fn as_key(self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix { dev, ino } => format!("unix:{dev}:{ino}"),
            #[cfg(windows)]
            Self::Windows { volume, index } => format!("win:{volume}:{index}"),
            #[cfg(not(any(unix, windows)))]
            Self::Unsupported => "unsupported".into(),
        }
    }
}

pub struct OpenedFile {
    pub file: File,
    pub len: u64,
    pub leaf: String,
}

pub struct PinnedDir {
    #[cfg(unix)]
    fd: OwnedFd,
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
    #[cfg(not(any(unix, windows)))]
    _unsupported: (),
    id: FileId,
}

impl PinnedDir {
    pub fn open_dir(path: &Path) -> Result<Self, PathCapabilityError> {
        require_operational_utf8(path)?;
        #[cfg(unix)]
        {
            let c_path = to_cstring(path)?;
            let fd = unsafe {
                libc::open(
                    c_path.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(map_open_error(
                    io::Error::last_os_error(),
                    PathCapabilityError::NotADirectory,
                ));
            }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let file = File::from(fd.try_clone().map_err(|_| PathCapabilityError::Io)?);
            let id = file_id(&file)?;
            drop(file);
            Ok(Self { fd, id })
        }
        #[cfg(windows)]
        {
            let file = open_windows_dir(path)?;
            reject_reparse_handle(&file)?;
            let id = file_id(&file)?;
            Ok(Self {
                handle: std::os::windows::io::OwnedHandle::from(file),
                id,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(PathCapabilityError::Io)
        }
    }

    pub fn id_key(&self) -> String {
        self.id.as_key()
    }

    pub fn open_file(
        &self,
        relative: &SafeRelativePath,
    ) -> Result<OpenedFile, PathCapabilityError> {
        self.open_file_components(relative.components())
    }

    pub fn open_file_names(&self, names: &[String]) -> Result<OpenedFile, PathCapabilityError> {
        let mut parts = Vec::with_capacity(names.len());
        for name in names {
            parts.push(SafeLeafName::parse(name)?);
        }
        if parts.is_empty() {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        self.open_file_components(&parts)
    }

    fn open_file_components(
        &self,
        components: &[SafeLeafName],
    ) -> Result<OpenedFile, PathCapabilityError> {
        if components.is_empty() {
            return Err(PathCapabilityError::UnsafeRelativePath);
        }
        #[cfg(unix)]
        {
            let file = unix_open_components(&self.fd, components)?;
            let meta = file.metadata().map_err(|_| PathCapabilityError::Io)?;
            if !meta.is_file() {
                return Err(PathCapabilityError::NotARegularFile);
            }
            Ok(OpenedFile {
                file,
                len: meta.len(),
                leaf: components.last().unwrap().as_str().to_string(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::OwnedHandle;
            let mut current = self
                .handle
                .try_clone()
                .map_err(|_| PathCapabilityError::Io)?;
            for (index, name) in components.iter().enumerate() {
                let last = index + 1 == components.len();
                let kind = if last {
                    win_at::RelativeKind::File
                } else {
                    win_at::RelativeKind::Directory
                };
                let opened = win_at::open_relative(
                    &current,
                    name.as_str(),
                    kind,
                    win_at::RelativeMode::Open,
                )?;
                reject_reparse_handle(&opened)?;
                if last {
                    let opened_meta = opened.metadata().map_err(|_| PathCapabilityError::Io)?;
                    if !opened_meta.is_file() {
                        return Err(PathCapabilityError::NotARegularFile);
                    }
                    return Ok(OpenedFile {
                        file: opened,
                        len: opened_meta.len(),
                        leaf: name.as_str().to_string(),
                    });
                }
                current = OwnedHandle::from(opened);
            }
            Err(PathCapabilityError::UnsafeRelativePath)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = components;
            Err(PathCapabilityError::Io)
        }
    }

    pub fn create_exclusive(&self, name: &SafeLeafName) -> Result<File, PathCapabilityError> {
        #[cfg(unix)]
        {
            unix_openat(
                self.fd.as_raw_fd(),
                name.as_str(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        }
        #[cfg(windows)]
        {
            let file = win_at::open_relative(
                &self.handle,
                name.as_str(),
                win_at::RelativeKind::File,
                win_at::RelativeMode::CreateNew,
            )?;
            reject_reparse_handle(&file)?;
            Ok(file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = name;
            Err(PathCapabilityError::Io)
        }
    }

    pub fn promote(
        &self,
        from: &SafeLeafName,
        to: &SafeLeafName,
    ) -> Result<(), PathCapabilityError> {
        match self.existing_kind(to)? {
            Some(NodeKind::Directory) => return Err(PathCapabilityError::NotARegularFile),
            Some(NodeKind::Other) => return Err(PathCapabilityError::NotARegularFile),
            Some(NodeKind::Symlink) | Some(NodeKind::File) | None => {}
        }
        #[cfg(unix)]
        {
            let from_c = to_cstring(Path::new(from.as_str()))?;
            let to_c = to_cstring(Path::new(to.as_str()))?;
            let rc = unsafe {
                libc::renameat(
                    self.fd.as_raw_fd(),
                    from_c.as_ptr(),
                    self.fd.as_raw_fd(),
                    to_c.as_ptr(),
                )
            };
            if rc != 0 {
                return Err(PathCapabilityError::Io);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let src = win_at::open_relative(
                &self.handle,
                from.as_str(),
                win_at::RelativeKind::File,
                win_at::RelativeMode::OpenDelete,
            )?;
            reject_reparse_handle(&src)?;
            win_at::rename_relative(&src, &self.handle, to.as_str(), true)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (from, to);
            Err(PathCapabilityError::Io)
        }
    }

    pub fn unlink(&self, name: &SafeLeafName) -> Result<(), PathCapabilityError> {
        #[cfg(unix)]
        {
            let c_name = to_cstring(Path::new(name.as_str()))?;
            let rc = unsafe { libc::unlinkat(self.fd.as_raw_fd(), c_name.as_ptr(), 0) };
            if rc != 0 {
                return Err(PathCapabilityError::Io);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let file = win_at::open_relative(
                &self.handle,
                name.as_str(),
                win_at::RelativeKind::File,
                win_at::RelativeMode::OpenDelete,
            )?;
            reject_reparse_handle(&file)?;
            win_at::delete_on_close(&file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = name;
            Err(PathCapabilityError::Io)
        }
    }

    fn existing_kind(&self, name: &SafeLeafName) -> Result<Option<NodeKind>, PathCapabilityError> {
        #[cfg(unix)]
        {
            let c_name = to_cstring(Path::new(name.as_str()))?;
            let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
            let rc = unsafe {
                libc::fstatat(
                    self.fd.as_raw_fd(),
                    c_name.as_ptr(),
                    &mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if rc != 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::NotFound {
                    return Ok(None);
                }
                return Err(PathCapabilityError::Io);
            }
            let mode = stat.st_mode & libc::S_IFMT;
            Ok(Some(if mode == libc::S_IFDIR {
                NodeKind::Directory
            } else if mode == libc::S_IFLNK {
                NodeKind::Symlink
            } else if mode == libc::S_IFREG {
                NodeKind::File
            } else {
                NodeKind::Other
            }))
        }
        #[cfg(windows)]
        {
            match win_at::open_relative_optional(
                &self.handle,
                name.as_str(),
                win_at::RelativeKind::Any,
                win_at::RelativeMode::OpenAttrs,
            )? {
                None => Ok(None),
                Some(file) => {
                    let meta = file.metadata().map_err(|_| PathCapabilityError::Io)?;
                    if is_reparse(&meta) {
                        Ok(Some(NodeKind::Symlink))
                    } else {
                        Ok(Some(node_kind_from_file_type(meta.file_type())))
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = name;
            Err(PathCapabilityError::Io)
        }
    }
}

pub struct DestScratch {
    pinned: PinnedDir,
    temp_name: SafeLeafName,
    final_name: SafeLeafName,
    file: Option<File>,
    armed: bool,
}

impl DestScratch {
    pub fn create(
        dest_dir: &Path,
        leaf: &str,
        transfer_id: &str,
    ) -> Result<Self, PathCapabilityError> {
        let pinned = PinnedDir::open_dir(dest_dir)?;
        Self::create_pinned(pinned, SafeLeafName::parse(leaf)?, transfer_id)
    }

    fn create_pinned(
        pinned: PinnedDir,
        final_name: SafeLeafName,
        transfer_id: &str,
    ) -> Result<Self, PathCapabilityError> {
        if !is_safe_transfer_id(transfer_id) {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let temp_name =
            SafeLeafName::parse(&format!("{}.yz-tmp-{}", final_name.as_str(), transfer_id))?;
        let file = pinned.create_exclusive(&temp_name)?;
        Ok(Self {
            pinned,
            temp_name,
            final_name,
            file: Some(file),
            armed: true,
        })
    }

    pub fn dest_key(&self) -> String {
        format!(
            "local:{}:{}",
            self.pinned.id_key(),
            self.final_name.as_str()
        )
    }

    pub fn take_file(&mut self) -> File {
        self.file.take().expect("dest scratch file already taken")
    }

    pub fn promote(mut self) -> Result<(), PathCapabilityError> {
        if let Some(file) = self.file.take() {
            drop(file);
        }
        self.pinned.promote(&self.temp_name, &self.final_name)?;
        self.armed = false;
        Ok(())
    }

    pub fn discard(self) {}
}

impl Drop for DestScratch {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if let Some(file) = self.file.take() {
            drop(file);
        }
        let _ = self.pinned.unlink(&self.temp_name);
    }
}

pub fn is_safe_transfer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub fn open_workspace_file(
    workspace_root: &str,
    relative_path: &str,
) -> Result<OpenedFile, PathCapabilityError> {
    let root = Path::new(workspace_root);
    require_operational_utf8(root)?;
    let relative = SafeRelativePath::parse(relative_path)?;
    let pinned = PinnedDir::open_dir(root)?;
    pinned.open_file(&relative)
}

struct ActiveWorkspacePath {
    id: String,
    canonical_root: String,
    pinned: PinnedDir,
}

pub struct WorkspacePathRegistry {
    active: Mutex<Option<ActiveWorkspacePath>>,
}

impl WorkspacePathRegistry {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub fn activate(&self, root: &Path) -> Result<String, PathCapabilityError> {
        require_operational_utf8(root)?;
        let canonical = std::fs::canonicalize(root).map_err(|_| PathCapabilityError::Io)?;
        require_operational_utf8(&canonical)?;
        let canonical_root = canonical
            .to_str()
            .ok_or(PathCapabilityError::NotUtf8)?
            .to_string();
        let pinned = PinnedDir::open_dir(&canonical)?;
        let id = format!("ws-{}", random_token());
        let mut active = self.active.lock().map_err(|_| PathCapabilityError::Io)?;
        *active = Some(ActiveWorkspacePath {
            id: id.clone(),
            canonical_root,
            pinned,
        });
        Ok(id)
    }

    pub fn canonical_root(&self, workspace_id: &str) -> Result<String, PathCapabilityError> {
        let active = self.active.lock().map_err(|_| PathCapabilityError::Io)?;
        active
            .as_ref()
            .filter(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.canonical_root.clone())
            .ok_or(PathCapabilityError::WorkspaceCapabilityMissing)
    }

    pub fn open_file(
        &self,
        workspace_id: &str,
        relative_path: &str,
    ) -> Result<OpenedFile, PathCapabilityError> {
        let relative = SafeRelativePath::parse(relative_path)?;
        let active = self.active.lock().map_err(|_| PathCapabilityError::Io)?;
        let workspace = active
            .as_ref()
            .filter(|workspace| workspace.id == workspace_id)
            .ok_or(PathCapabilityError::WorkspaceCapabilityMissing)?;
        workspace.pinned.open_file(&relative)
    }

    pub fn clear(&self) {
        if let Ok(mut active) = self.active.lock() {
            *active = None;
        }
    }
}

pub struct WorkspacePathState(pub WorkspacePathRegistry);

impl WorkspacePathState {
    pub fn new() -> Self {
        Self(WorkspacePathRegistry::new())
    }

    pub fn clear(&self) {
        self.0.clear();
    }
}

pub fn open_absolute_file_nofollow(path: &Path) -> Result<OpenedFile, PathCapabilityError> {
    require_operational_utf8(path)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PathCapabilityError::UnsafeLeaf)?;
    let leaf = SafeLeafName::parse(leaf)?;
    let parent = path.parent().ok_or(PathCapabilityError::NotADirectory)?;
    let pinned = PinnedDir::open_dir(parent)?;
    pinned.open_file_components(std::slice::from_ref(&leaf))
}

struct SelectedPath {
    file: File,
    leaf: String,
    len: u64,
    expires_at: Instant,
}

pub struct SelectedPathRegistry {
    items: Mutex<HashMap<String, SelectedPath>>,
}

impl SelectedPathRegistry {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
        }
    }

    pub fn grant(&self, path: &str) -> Result<String, PathCapabilityError> {
        self.purge_expired();
        let opened = open_absolute_file_nofollow(Path::new(path))?;
        let id = format!("sel-{}", random_token());
        let mut items = self.items.lock().map_err(|_| PathCapabilityError::Io)?;
        items.insert(
            id.clone(),
            SelectedPath {
                file: opened.file,
                leaf: opened.leaf,
                len: opened.len,
                expires_at: Instant::now() + SELECTED_PATH_TTL,
            },
        );
        Ok(id)
    }

    pub fn peek_leaf(&self, id: &str) -> Result<String, PathCapabilityError> {
        let items = self.items.lock().map_err(|_| PathCapabilityError::Io)?;
        let item = items
            .get(id)
            .ok_or(PathCapabilityError::CapabilityMissing)?;
        if item.expires_at <= Instant::now() {
            return Err(PathCapabilityError::CapabilityExpired);
        }
        Ok(item.leaf.clone())
    }

    pub fn take(&self, id: &str) -> Result<OpenedFile, PathCapabilityError> {
        let mut items = self.items.lock().map_err(|_| PathCapabilityError::Io)?;
        let item = items
            .remove(id)
            .ok_or(PathCapabilityError::CapabilityMissing)?;
        if item.expires_at <= Instant::now() {
            return Err(PathCapabilityError::CapabilityExpired);
        }
        Ok(OpenedFile {
            file: item.file,
            len: item.len,
            leaf: item.leaf,
        })
    }

    pub fn clear(&self) {
        if let Ok(mut items) = self.items.lock() {
            items.clear();
        }
    }

    fn purge_expired(&self) {
        if let Ok(mut items) = self.items.lock() {
            let now = Instant::now();
            items.retain(|_, item| item.expires_at > now);
        }
    }
}

struct DownloadDestination {
    pinned: PinnedDir,
    leaf: SafeLeafName,
    expires_at: Instant,
}

pub struct DownloadDestinationRegistry {
    items: Mutex<HashMap<String, DownloadDestination>>,
}

impl DownloadDestinationRegistry {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
        }
    }

    pub fn grant(&self, path: &Path) -> Result<DownloadDestinationGrant, PathCapabilityError> {
        self.purge_expired();
        require_operational_utf8(path)?;
        let leaf = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(PathCapabilityError::UnsafeLeaf)?;
        let leaf = SafeLeafName::parse(leaf)?;
        let parent = path.parent().ok_or(PathCapabilityError::NotADirectory)?;
        let pinned = PinnedDir::open_dir(parent)?;
        let id = format!("download-{}", random_token());
        let grant = DownloadDestinationGrant {
            id: id.clone(),
            leaf: leaf.as_str().to_string(),
        };
        let mut items = self.items.lock().map_err(|_| PathCapabilityError::Io)?;
        items.insert(
            id,
            DownloadDestination {
                pinned,
                leaf,
                expires_at: Instant::now() + DOWNLOAD_DEST_TTL,
            },
        );
        Ok(grant)
    }

    pub fn take_scratch(
        &self,
        id: &str,
        transfer_id: &str,
    ) -> Result<DestScratch, PathCapabilityError> {
        let mut items = self.items.lock().map_err(|_| PathCapabilityError::Io)?;
        let item = items
            .remove(id)
            .ok_or(PathCapabilityError::DownloadCapabilityMissing)?;
        if item.expires_at <= Instant::now() {
            return Err(PathCapabilityError::DownloadCapabilityExpired);
        }
        DestScratch::create_pinned(item.pinned, item.leaf, transfer_id)
    }

    pub fn clear(&self) {
        if let Ok(mut items) = self.items.lock() {
            items.clear();
        }
    }

    fn purge_expired(&self) {
        if let Ok(mut items) = self.items.lock() {
            let now = Instant::now();
            items.retain(|_, item| item.expires_at > now);
        }
    }
}

pub struct SelectedPathState(pub SelectedPathRegistry);

impl SelectedPathState {
    pub fn new() -> Self {
        Self(SelectedPathRegistry::new())
    }

    pub fn clear(&self) {
        self.0.clear();
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedPathGrant {
    pub id: String,
    pub leaf: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadDestinationGrant {
    pub id: String,
    pub leaf: String,
}

pub struct DownloadDestinationState(pub DownloadDestinationRegistry);

impl DownloadDestinationState {
    pub fn new() -> Self {
        Self(DownloadDestinationRegistry::new())
    }

    pub fn clear(&self) {
        self.0.clear();
    }
}

fn grant_native_selection(
    registry: &SelectedPathRegistry,
    path: &Path,
) -> Result<SelectedPathGrant, PathCapabilityError> {
    let text = require_operational_utf8(path)?;
    let id = registry.grant(text)?;
    let leaf = registry.peek_leaf(&id)?;
    Ok(SelectedPathGrant { id, leaf })
}

#[tauri::command(async)]
pub async fn sftp_pick_download_destination(
    app: tauri::AppHandle,
    state: tauri::State<'_, DownloadDestinationState>,
    suggested_leaf: String,
) -> Result<Option<DownloadDestinationGrant>, String> {
    use tauri_plugin_dialog::DialogExt;
    let leaf = SafeLeafName::parse(&suggested_leaf).map_err(String::from)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_can_create_directories(false)
        .set_file_name(leaf.as_str())
        .save_file(move |file| {
            let _ = tx.send(file);
        });
    let Some(file) = rx
        .await
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?
    else {
        return Ok(None);
    };
    let path = file
        .into_path()
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?;
    state.0.grant(&path).map(Some).map_err(String::from)
}

#[tauri::command(async)]
pub async fn sftp_pick_selected_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, SelectedPathState>,
) -> Result<Vec<SelectedPathGrant>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_can_create_directories(false)
        .pick_files(move |files| {
            let _ = tx.send(files);
        });
    let Some(files) = rx
        .await
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?
    else {
        return Ok(Vec::new());
    };
    let mut grants = Vec::with_capacity(files.len());
    for file in files {
        let path = file
            .into_path()
            .map_err(|_| PathCapabilityError::Io.as_code().to_string())?;
        grants.push(grant_native_selection(&state.0, &path).map_err(String::from)?);
    }
    Ok(grants)
}

pub struct TransferDestSet {
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl TransferDestSet {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    pub fn acquire(&self, key: String) -> Result<TransferDestGuard, PathCapabilityError> {
        let mut guard = self.inner.lock().map_err(|_| PathCapabilityError::Io)?;
        if !guard.insert(key.clone()) {
            return Err(PathCapabilityError::DestinationBusy);
        }
        Ok(TransferDestGuard {
            inner: Arc::clone(&self.inner),
            key,
        })
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }
}

pub struct TransferDestGuard {
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
    key: String,
}

impl Drop for TransferDestGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&self.key);
        }
    }
}

pub fn remote_dest_key(session_id: &str, remote_path: &str) -> String {
    format!("remote:{session_id}:{remote_path}")
}

fn require_operational_utf8(path: &Path) -> Result<&str, PathCapabilityError> {
    let Some(text) = path.to_str() else {
        return Err(PathCapabilityError::NotUtf8);
    };
    if text.contains('\0') {
        return Err(PathCapabilityError::NotUtf8);
    }
    Ok(text)
}

fn file_id(file: &File) -> Result<FileId, PathCapabilityError> {
    #[cfg(unix)]
    {
        let meta = file.metadata().map_err(|_| PathCapabilityError::Io)?;
        Ok(FileId::Unix {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(windows)]
    {
        let (volume, index, _) = windows_file_identity(file)?;
        Ok(FileId::Windows { volume, index })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(PathCapabilityError::Io)
    }
}

#[cfg(windows)]
pub(crate) fn windows_file_identity(file: &File) -> Result<(u32, u64, bool), PathCapabilityError> {
    win_at::file_identity(file)
}

#[cfg(unix)]
fn unix_open_components(
    root: &OwnedFd,
    components: &[SafeLeafName],
) -> Result<File, PathCapabilityError> {
    let mut dirfd = root.as_raw_fd();
    let mut held: Option<OwnedFd> = None;
    for (index, name) in components.iter().enumerate() {
        let last = index + 1 == components.len();
        let flags = if last {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW
        } else {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW
        };
        let next = unix_openat_fd(dirfd, name.as_str(), flags, 0)?;
        if last {
            return Ok(File::from(next));
        }
        dirfd = next.as_raw_fd();
        held = Some(next);
    }
    drop(held);
    Err(PathCapabilityError::UnsafeRelativePath)
}

#[cfg(unix)]
fn unix_openat(dirfd: i32, name: &str, flags: i32, mode: u32) -> Result<File, PathCapabilityError> {
    Ok(File::from(unix_openat_fd(dirfd, name, flags, mode)?))
}

#[cfg(unix)]
fn unix_openat_fd(
    dirfd: i32,
    name: &str,
    flags: i32,
    mode: u32,
) -> Result<OwnedFd, PathCapabilityError> {
    let c_name = to_cstring(Path::new(name))?;
    let fd = unsafe { libc::openat(dirfd, c_name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(map_openat_error(dirfd, c_name.as_ptr()));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(unix)]
fn map_openat_error(dirfd: i32, name: *const libc::c_char) -> PathCapabilityError {
    let err = io::Error::last_os_error();
    let code = err.raw_os_error();
    if code == Some(libc::ELOOP) || unix_is_symlink_at(dirfd, name) {
        return PathCapabilityError::SymlinkRejected;
    }
    if err.kind() == io::ErrorKind::NotFound {
        return PathCapabilityError::Io;
    }
    if code == Some(libc::ENOTDIR) {
        return PathCapabilityError::NotADirectory;
    }
    PathCapabilityError::Io
}

#[cfg(unix)]
fn unix_is_symlink_at(dirfd: i32, name: *const libc::c_char) -> bool {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let rc = unsafe { libc::fstatat(dirfd, name, stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    if rc != 0 {
        return false;
    }
    unsafe { (*stat.as_ptr()).st_mode & libc::S_IFMT == libc::S_IFLNK }
}

#[cfg(unix)]
fn to_cstring(path: &Path) -> Result<std::ffi::CString, PathCapabilityError> {
    let text = require_operational_utf8(path)?;
    std::ffi::CString::new(text).map_err(|_| PathCapabilityError::NotUtf8)
}

#[cfg(unix)]
fn map_open_error(err: io::Error, fallback: PathCapabilityError) -> PathCapabilityError {
    if err.raw_os_error() == Some(libc::ELOOP) {
        PathCapabilityError::SymlinkRejected
    } else if err.kind() == io::ErrorKind::NotFound {
        PathCapabilityError::Io
    } else if err.raw_os_error() == Some(libc::ENOTDIR) {
        PathCapabilityError::NotADirectory
    } else {
        fallback
    }
}

#[cfg(windows)]
mod win_at {
    use super::PathCapabilityError;
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const DELETE: u32 = 0x0001_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_LIST_DIRECTORY: u32 = 0x0001;
    const FILE_TRAVERSE: u32 = 0x0020;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const FILE_OPEN: u32 = 1;
    const FILE_CREATE: u32 = 2;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
    const FILE_RENAME_INFO: u32 = 3;
    const FILE_DISPOSITION_INFO: u32 = 4;
    const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034_u32 as i32;
    const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xC000_003A_u32 as i32;
    const STATUS_OBJECT_NAME_COLLISION: i32 = 0xC000_0035_u32 as i32;
    const STATUS_NOT_A_DIRECTORY: i32 = 0xC000_0103_u32 as i32;
    const STATUS_FILE_IS_A_DIRECTORY: i32 = 0xC000_00BA_u32 as i32;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: RawHandle,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status: isize,
        information: usize,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            file_handle: *mut RawHandle,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *mut i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut c_void,
            ea_length: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SetFileInformationByHandle(
            file: RawHandle,
            class: u32,
            info: *const c_void,
            size: u32,
        ) -> i32;
    }

    pub(super) fn file_identity(file: &File) -> Result<(u32, u64, bool), PathCapabilityError> {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        if ok == 0 {
            return Err(PathCapabilityError::Io);
        }
        let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        Ok((
            info.dwVolumeSerialNumber,
            index,
            info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        ))
    }

    #[derive(Clone, Copy)]
    pub enum RelativeKind {
        File,
        Directory,
        Any,
    }

    #[derive(Clone, Copy)]
    pub enum RelativeMode {
        Open,
        CreateNew,
        OpenDelete,
        OpenAttrs,
    }

    pub fn open_relative(
        dir: &OwnedHandle,
        name: &str,
        kind: RelativeKind,
        mode: RelativeMode,
    ) -> Result<File, PathCapabilityError> {
        match open_relative_optional(dir, name, kind, mode)? {
            Some(file) => Ok(file),
            None => Err(PathCapabilityError::Io),
        }
    }

    pub fn open_relative_optional(
        dir: &OwnedHandle,
        name: &str,
        kind: RelativeKind,
        mode: RelativeMode,
    ) -> Result<Option<File>, PathCapabilityError> {
        if name.contains('\0') || name.contains('/') || name.contains('\\') {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let mut utf16: Vec<u16> = name.encode_utf16().collect();
        if utf16.iter().any(|&unit| unit == 0) {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let byte_len = utf16.len().saturating_mul(2);
        if byte_len == 0 || byte_len > u16::MAX as usize {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let mut unicode = UnicodeString {
            length: byte_len as u16,
            maximum_length: byte_len as u16,
            buffer: utf16.as_mut_ptr(),
        };
        let mut attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: dir.as_raw_handle(),
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut iosb = IoStatusBlock {
            status: 0,
            information: 0,
        };
        let mut handle: RawHandle = std::ptr::null_mut();
        let (access, disposition, options, file_attrs) = match (kind, mode) {
            (RelativeKind::Directory, _) => (
                FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                0,
            ),
            (RelativeKind::File, RelativeMode::CreateNew) => (
                GENERIC_WRITE | SYNCHRONIZE,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                FILE_ATTRIBUTE_NORMAL,
            ),
            (RelativeKind::File, RelativeMode::OpenDelete)
            | (RelativeKind::Any, RelativeMode::OpenDelete) => (
                DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                0,
            ),
            (RelativeKind::Any, RelativeMode::OpenAttrs) => (
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                0,
            ),
            (RelativeKind::File, _) => (
                GENERIC_READ | SYNCHRONIZE,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                0,
            ),
            (RelativeKind::Any, _) => (
                GENERIC_READ | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                0,
            ),
        };
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                access,
                &mut attrs,
                &mut iosb,
                std::ptr::null_mut(),
                file_attrs,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                disposition,
                options,
                std::ptr::null_mut(),
                0,
            )
        };
        if status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_OBJECT_PATH_NOT_FOUND {
            return Ok(None);
        }
        if status < 0 {
            return Err(map_ntstatus(status));
        }
        Ok(Some(unsafe { File::from_raw_handle(handle) }))
    }

    fn map_ntstatus(status: i32) -> PathCapabilityError {
        if status == STATUS_OBJECT_NAME_COLLISION {
            return PathCapabilityError::Io;
        }
        if status == STATUS_NOT_A_DIRECTORY {
            return PathCapabilityError::NotADirectory;
        }
        if status == STATUS_FILE_IS_A_DIRECTORY {
            return PathCapabilityError::NotARegularFile;
        }
        PathCapabilityError::Io
    }

    pub fn rename_relative(
        file: &File,
        dest_dir: &OwnedHandle,
        new_name: &str,
        replace: bool,
    ) -> Result<(), PathCapabilityError> {
        if new_name.contains('\0') || new_name.contains('/') || new_name.contains('\\') {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let utf16: Vec<u16> = new_name.encode_utf16().collect();
        let name_bytes = utf16.len().saturating_mul(2);
        if name_bytes == 0 {
            return Err(PathCapabilityError::UnsafeLeaf);
        }
        let header = 20usize;
        let mut buf = vec![0u8; header + name_bytes];
        buf[0] = u8::from(replace);
        let root = dest_dir.as_raw_handle() as u64;
        buf[8..16].copy_from_slice(&root.to_le_bytes());
        buf[16..20].copy_from_slice(&(name_bytes as u32).to_le_bytes());
        let name_raw =
            unsafe { std::slice::from_raw_parts(utf16.as_ptr().cast::<u8>(), name_bytes) };
        buf[header..].copy_from_slice(name_raw);
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FILE_RENAME_INFO,
                buf.as_ptr().cast(),
                buf.len() as u32,
            )
        };
        if ok == 0 {
            return Err(PathCapabilityError::Io);
        }
        Ok(())
    }

    pub fn delete_on_close(file: &File) -> Result<(), PathCapabilityError> {
        #[repr(C)]
        struct FileDispositionInfo {
            delete_file: u8,
        }
        let info = FileDispositionInfo { delete_file: 1 };
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FILE_DISPOSITION_INFO,
                (&info as *const FileDispositionInfo).cast(),
                std::mem::size_of::<FileDispositionInfo>() as u32,
            )
        };
        if ok == 0 {
            return Err(PathCapabilityError::Io);
        }
        Ok(())
    }
}

#[cfg(windows)]
fn open_windows_dir(path: &Path) -> Result<File, PathCapabilityError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| PathCapabilityError::NotADirectory)
}

#[cfg(windows)]
fn reject_reparse_handle(file: &File) -> Result<(), PathCapabilityError> {
    let meta = file.metadata().map_err(|_| PathCapabilityError::Io)?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(PathCapabilityError::ReparseRejected);
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn random_token() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = rand::random::<[u8; 16]>();
    let mut out = String::with_capacity(32);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn safe_leaf_name_rejects_hostile_forms() {
        for name in [
            "",
            ".",
            "..",
            "../.ssh/config",
            "foo/bar",
            "foo\\bar",
            "C:foo",
            "C:\\Windows\\win.ini",
            "\\\\server\\share\\secret",
            "//server/share/secret",
            "\\\\?\\C:\\Windows\\win.ini",
            "//?/C:/Windows/win.ini",
            "a\0b",
        ] {
            assert!(
                SafeLeafName::parse(name).is_err(),
                "expected reject: {name:?}"
            );
        }
        assert!(SafeLeafName::parse("report.pdf").is_ok());
        assert!(SafeLeafName::parse("中文.txt").is_ok());
    }

    #[test]
    fn windows_ordinary_leaf_rejects_ads_trailing_and_reserved_names() {
        for name in [
            "report.txt:hidden",
            "file:",
            "report.txt.",
            "report.txt ",
            "CON",
            "con.txt",
            "PRN",
            "AUX",
            "NUL",
            "COM1",
            "lpt9.log",
        ] {
            assert!(
                !windows_ordinary_leaf(name),
                "expected windows reject: {name:?}"
            );
        }
        assert!(windows_ordinary_leaf("report.txt"));
        assert!(windows_ordinary_leaf("中文.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_safe_leaf_name_rejects_ads_and_reserved_forms() {
        for name in [
            "report.txt:hidden",
            "report.txt.",
            "report.txt ",
            "CON",
            "nul.txt",
        ] {
            assert!(
                SafeLeafName::parse(name).is_err(),
                "expected reject on windows: {name:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_identity_is_stable_and_distinguishes_live_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();

        let first_a = File::open(&first).unwrap();
        let first_b = File::open(&first).unwrap();
        let second = File::open(&second).unwrap();
        let first_a_id = windows_file_identity(&first_a).unwrap();
        let first_b_id = windows_file_identity(&first_b).unwrap();
        let second_id = windows_file_identity(&second).unwrap();

        assert_eq!(first_a_id, first_b_id);
        assert_ne!(first_a_id, second_id);
        assert!(!first_a_id.2);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_safe_leaf_name_still_allows_colon_in_ordinary_names() {
        assert!(SafeLeafName::parse("report.txt:hidden").is_ok());
        assert!(SafeLeafName::parse("report.txt.").is_ok());
        assert!(SafeLeafName::parse("CON").is_ok());
    }

    #[test]
    fn safe_relative_path_rejects_escape_and_separators() {
        assert!(SafeRelativePath::parse("src/main.rs").is_ok());
        assert!(SafeRelativePath::parse("main.rs").is_ok());
        for path in [
            "",
            "/etc/passwd",
            "src/../secret",
            "src\\secret",
            "C:\\Windows\\win.ini",
            "\\\\server\\share\\a",
            "//?/C:/a",
            "a//b",
        ] {
            assert!(
                SafeRelativePath::parse(path).is_err(),
                "expected reject: {path:?}"
            );
        }
    }

    #[test]
    fn dest_scratch_rejects_hostile_leaf_before_any_io() {
        let tmp = tempfile::tempdir().unwrap();
        for leaf in [
            "../.ssh/config",
            "foo\\bar",
            "C:foo",
            "\\\\server\\share\\a",
            "//?/C:/a",
        ] {
            let err = DestScratch::create(tmp.path(), leaf, "xfer-1")
                .err()
                .expect("hostile dest leaf must fail");
            assert_eq!(err, PathCapabilityError::UnsafeLeaf, "{leaf}");
        }
        assert!(tmp.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn dest_scratch_promotes_atomically_and_overwrites_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("report.pdf");
        std::fs::write(&dest, "old").unwrap();
        let mut scratch = DestScratch::create(tmp.path(), "report.pdf", "xfer-1").unwrap();
        scratch.take_file().write_all(b"new").unwrap();
        scratch.promote().unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
        let leftovers: Vec<_> = tmp
            .path()
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn dest_scratch_discard_removes_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = DestScratch::create(tmp.path(), "a.bin", "xfer-9").unwrap();
        scratch.discard();
        assert!(tmp.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn transfer_dest_set_rejects_same_destination() {
        let set = TransferDestSet::new();
        let first = set.acquire("local:1:a.txt".into()).unwrap();
        assert_eq!(
            set.acquire("local:1:a.txt".into())
                .err()
                .expect("same dest must be busy"),
            PathCapabilityError::DestinationBusy
        );
        drop(first);
        assert!(set.acquire("local:1:a.txt".into()).is_ok());
    }

    #[test]
    fn native_selection_grant_uses_backend_path_not_renderer_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("picked.txt");
        std::fs::write(&path, "hello").unwrap();
        let registry = SelectedPathRegistry::new();
        let grant = grant_native_selection(&registry, &path).unwrap();
        assert_eq!(grant.leaf, "picked.txt");
        assert!(grant.id.starts_with("sel-"));
        let opened = registry.take(&grant.id).unwrap();
        assert_eq!(opened.len, 5);
        assert!(registry.take(&grant.id).is_err());
    }

    #[test]
    fn selected_path_is_single_use_and_expires() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("picked.txt");
        std::fs::write(&path, "hello").unwrap();
        let registry = SelectedPathRegistry::new();
        let id = registry.grant(path.to_str().unwrap()).unwrap();
        assert_eq!(registry.peek_leaf(&id).unwrap(), "picked.txt");
        let opened = registry.take(&id).unwrap();
        assert_eq!(opened.len, 5);
        assert!(registry.take(&id).is_err());
    }

    #[test]
    fn workspace_capability_rejects_forged_and_stale_ids() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("first.txt"), "first").unwrap();
        std::fs::write(second.path().join("second.txt"), "second").unwrap();
        let registry = WorkspacePathRegistry::new();
        let first_id = registry.activate(first.path()).unwrap();
        assert_eq!(
            registry.canonical_root(&first_id).unwrap(),
            first.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert!(registry.canonical_root("ws-forged").is_err());
        assert!(registry.open_file("ws-forged", "first.txt").is_err());
        let second_id = registry.activate(second.path()).unwrap();
        assert!(registry.open_file(&first_id, "first.txt").is_err());
        let mut opened = registry.open_file(&second_id, "second.txt").unwrap();
        let mut text = String::new();
        opened.file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "second");
    }

    #[test]
    fn download_destination_capability_is_pinned_and_single_use() {
        let dest = tempfile::tempdir().unwrap();
        let registry = DownloadDestinationRegistry::new();
        let grant = registry.grant(&dest.path().join("report.txt")).unwrap();
        assert_eq!(grant.leaf, "report.txt");
        let mut scratch = registry.take_scratch(&grant.id, "xfer-1").unwrap();
        scratch.take_file().write_all(b"downloaded").unwrap();
        scratch.promote().unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.path().join("report.txt")).unwrap(),
            "downloaded"
        );
        assert_eq!(
            registry
                .take_scratch(&grant.id, "xfer-2")
                .err()
                .expect("capability must be single use"),
            PathCapabilityError::DownloadCapabilityMissing
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_symlink_to_outside_secret_is_rejected() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top-secret").unwrap();
        std::os::unix::fs::symlink(&secret, workspace.path().join("link.txt")).unwrap();
        std::fs::write(workspace.path().join("ok.txt"), "visible").unwrap();

        let err = open_workspace_file(workspace.path().to_str().unwrap(), "link.txt")
            .err()
            .expect("workspace symlink must fail");
        assert_eq!(err, PathCapabilityError::SymlinkRejected);

        let mut opened = open_workspace_file(workspace.path().to_str().unwrap(), "ok.txt").unwrap();
        let mut buf = String::new();
        opened.file.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "visible");
        assert_eq!(opened.len, 7);
    }

    #[cfg(unix)]
    #[test]
    fn parent_and_final_symlink_swap_cannot_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top-secret").unwrap();
        let sub = workspace.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "inside").unwrap();

        let pinned = PinnedDir::open_dir(workspace.path()).unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        std::os::unix::fs::symlink(outside.path(), &sub).unwrap();
        let err = pinned
            .open_file(&SafeRelativePath::parse("sub/file.txt").unwrap())
            .err()
            .expect("parent symlink swap must fail");
        assert_eq!(err, PathCapabilityError::SymlinkRejected);

        std::fs::remove_dir_all(workspace.path().join("sub")).unwrap();
        std::fs::write(workspace.path().join("final.txt"), "ok").unwrap();
        let pinned = PinnedDir::open_dir(workspace.path()).unwrap();
        std::fs::remove_file(workspace.path().join("final.txt")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            workspace.path().join("final.txt"),
        )
        .unwrap();
        let err = pinned
            .open_file(&SafeRelativePath::parse("final.txt").unwrap())
            .err()
            .expect("final symlink swap must fail");
        assert_eq!(err, PathCapabilityError::SymlinkRejected);
    }

    #[test]
    fn windows_capability_backend_is_handle_relative_or_absent() {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(tmp.path().join("a.txt"), b"hi").unwrap();
            let pinned = PinnedDir::open_dir(tmp.path()).unwrap();
            let _ = pinned.handle.as_raw_handle();
            let opened = pinned
                .open_file(&SafeRelativePath::parse("a.txt").unwrap())
                .unwrap();
            assert_eq!(opened.len, 2);
        }
        #[cfg(not(windows))]
        {
            assert!(
                cfg!(unix) || !cfg!(windows),
                "non-Windows builds must not compile a path-join Windows fallback"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_parent_reparse_swap_is_rejected() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top-secret").unwrap();
        let sub = workspace.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "inside").unwrap();
        let pinned = PinnedDir::open_dir(workspace.path()).unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        std::os::windows::fs::symlink_dir(outside.path(), &sub).unwrap();
        let err = pinned
            .open_file(&SafeRelativePath::parse("sub/file.txt").unwrap())
            .err()
            .expect("parent reparse swap must fail");
        assert_eq!(err, PathCapabilityError::ReparseRejected);
    }

    #[cfg(unix)]
    #[test]
    fn selected_path_final_symlink_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top-secret").unwrap();
        let link = tmp.path().join("picked.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let registry = SelectedPathRegistry::new();
        let err = registry.grant(link.to_str().unwrap()).unwrap_err();
        assert_eq!(err, PathCapabilityError::SymlinkRejected);
    }
}
