//! Workspace copy / move / import engine shared by the desktop app and the
//! remote host. Every workspace-side read and write goes through `PinnedDir`
//! (descriptor-relative, never following links); only `import_into` reads
//! external absolute paths with std::fs.
//!
//! All paths are workspace-relative and `/`-separated; `""` is the workspace
//! root. Results are the created/moved TOP-LEVEL paths, in source order.
//!
//! Naming rule on collision (never overwrite, like Finder / VS Code): a file
//! `report.txt` becomes `report copy.txt`, then `report copy 2.txt`, ... The
//! split happens at the LAST dot of a file name (`archive.tar.gz` ->
//! `archive.tar copy.gz`); a leading dot is not an extension (`.env` ->
//! `.env copy`). Directories are never split (`src` -> `src copy`,
//! `v1.2` -> `v1.2 copy`).

use crate::path_capability::{NodeKind, PinnedDir, SafeLeafName, SafeRelativePath};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const MAX_COPY_ENTRIES: usize = 20_000;
pub const MAX_COPY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const COPY_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_DEPTH: usize = 128;
const MAX_NAME_ATTEMPTS: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Limits {
    max_entries: usize,
    max_bytes: u64,
    timeout: Duration,
}

const DEFAULT_LIMITS: Limits = Limits {
    max_entries: MAX_COPY_ENTRIES,
    max_bytes: MAX_COPY_BYTES,
    timeout: COPY_TIMEOUT,
};

struct Budget {
    limits: Limits,
    deadline: Instant,
    entries: usize,
    bytes: u64,
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            deadline: Instant::now() + limits.timeout,
            entries: 0,
            bytes: 0,
        }
    }

    fn check(&self) -> Result<(), String> {
        if self.entries > self.limits.max_entries
            || self.bytes > self.limits.max_bytes
            || Instant::now() >= self.deadline
        {
            return Err("copy-too-large".into());
        }
        Ok(())
    }

    fn entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        self.check()
    }

    fn add_bytes(&mut self, count: u64) -> Result<(), String> {
        self.bytes += count;
        self.check()
    }
}

/// A copy source: a no-follow workspace entry or an external absolute path.
enum Src {
    Pinned {
        parent: Rc<PinnedDir>,
        leaf: SafeLeafName,
    },
    External(PathBuf),
}

impl Src {
    fn open_file(&self) -> Result<(File, std::fs::Metadata), String> {
        let file = match self {
            Self::Pinned { parent, leaf } => {
                parent
                    .open_file(&SafeRelativePath::parse(leaf.as_str())?)?
                    .file
            }
            Self::External(path) => File::open(path).map_err(|e| e.to_string())?,
        };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("not-a-regular-file".into());
        }
        Ok((file, metadata))
    }

    /// Copyable children only (files and directories); links and special
    /// files are skipped, never followed.
    fn children(&self) -> Result<Vec<(SafeLeafName, NodeKind, Src)>, String> {
        let mut out = Vec::new();
        match self {
            Self::Pinned { parent, leaf } => {
                let dir = Rc::new(parent.open_subdir(leaf.as_str())?);
                let entries = dir.list_entries(MAX_COPY_ENTRIES).map_err(|error| {
                    if error == "sftp-tree-entry-limit" {
                        "copy-too-large".to_string()
                    } else {
                        error
                    }
                })?;
                for (name, kind) in entries {
                    if matches!(kind, NodeKind::File | NodeKind::Directory) {
                        let leaf = SafeLeafName::parse(&name)?;
                        out.push((
                            leaf.clone(),
                            kind,
                            Src::Pinned {
                                parent: dir.clone(),
                                leaf,
                            },
                        ));
                    }
                }
            }
            Self::External(path) => {
                for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    let Ok(leaf) = SafeLeafName::parse(&name) else {
                        continue;
                    };
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let kind = crate::path_capability::node_kind_from_file_type(file_type);
                    if matches!(kind, NodeKind::File | NodeKind::Directory) {
                        out.push((leaf, kind, Src::External(entry.path())));
                    }
                    if out.len() > MAX_COPY_ENTRIES {
                        return Err("copy-too-large".into());
                    }
                }
            }
        }
        Ok(out)
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn join(dir: &str, leaf: &str) -> String {
    if dir.is_empty() {
        leaf.to_owned()
    } else {
        format!("{dir}/{leaf}")
    }
}

fn open_target(root: &PinnedDir, target_dir: &str) -> Result<PinnedDir, String> {
    if !target_dir.is_empty() {
        SafeRelativePath::parse(target_dir)?;
    }
    Ok(root.open_subdir(target_dir)?)
}

/// True when `target_dir` is the directory identified by `id` or lies inside it.
/// Compares directory identities, so case-insensitive filesystems cannot slip
/// past a lexical prefix check.
fn target_within(root: &PinnedDir, target_dir: &str, id: &str) -> Result<bool, String> {
    if root.id_key() == id {
        return Ok(true);
    }
    if target_dir.is_empty() {
        return Ok(false);
    }
    let mut prefix = String::new();
    for component in SafeRelativePath::parse(target_dir)?.components() {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component.as_str());
        if root.open_subdir(&prefix)?.id_key() == id {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Pick `name`, else `stem copy.ext`, `stem copy 2.ext`, ... that is free in `dir`.
fn unique_leaf(dir: &PinnedDir, name: &str, is_dir: bool) -> Result<SafeLeafName, String> {
    let (stem, ext) = match name.rfind('.') {
        Some(index) if index > 0 && !is_dir => name.split_at(index),
        _ => (name, ""),
    };
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let candidate = match attempt {
            1 => name.to_owned(),
            2 => format!("{stem} copy{ext}"),
            n => format!("{stem} copy {}{ext}", n - 1),
        };
        let leaf = SafeLeafName::parse(&candidate)?;
        if dir.existing_kind(&leaf)?.is_none() {
            return Ok(leaf);
        }
    }
    Err("name-conflict".into())
}

fn copy_file(
    src: &Src,
    dest: &PinnedDir,
    leaf: &SafeLeafName,
    budget: &mut Budget,
    created: Option<&mut bool>,
) -> Result<(), String> {
    let (mut input, metadata) = src.open_file()?;
    let mut output = dest.create_exclusive(leaf)?;
    if let Some(created) = created {
        *created = true;
    }
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        budget.add_bytes(read as u64)?;
        output
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        output
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = metadata;
    output.flush().map_err(|e| e.to_string())
}

/// Copy one node. For the top-level item `created` (Some) flips to true once
/// the entry exists, so failure cleanup never removes an entry this copy did
/// not create.
fn copy_node(
    src: &Src,
    kind: NodeKind,
    dest: &PinnedDir,
    leaf: &SafeLeafName,
    budget: &mut Budget,
    depth: usize,
    created: Option<&mut bool>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("copy-too-large".into());
    }
    budget.entry()?;
    match kind {
        NodeKind::File => copy_file(src, dest, leaf, budget, created),
        NodeKind::Directory => {
            dest.mkdir(leaf)?;
            if let Some(created) = created {
                *created = true;
            }
            let child = dest.open_subdir(leaf.as_str())?;
            for (name, child_kind, child_src) in src.children()? {
                copy_node(
                    &child_src,
                    child_kind,
                    &child,
                    &name,
                    budget,
                    depth + 1,
                    None,
                )?;
            }
            Ok(())
        }
        _ => Err("copy-unsupported-kind".into()),
    }
}

/// Copy one top-level source into `target` under a unique leaf, cleaning up the
/// partially created item on failure. Returns the leaf actually used.
fn copy_top(
    src: &Src,
    kind: NodeKind,
    name: &str,
    target: &PinnedDir,
    budget: &mut Budget,
) -> Result<SafeLeafName, String> {
    let leaf = unique_leaf(target, name, kind == NodeKind::Directory)?;
    let mut created = false;
    match copy_node(src, kind, target, &leaf, budget, 0, Some(&mut created)) {
        Ok(()) => Ok(leaf),
        Err(error) => {
            if created {
                let _ = target.remove_tree(
                    &leaf,
                    &mut 1_000_000,
                    Instant::now() + Duration::from_secs(30),
                );
            }
            Err(error)
        }
    }
}

struct Planned {
    parent: Rc<PinnedDir>,
    leaf: SafeLeafName,
    kind: NodeKind,
}

/// Validate every workspace source before touching anything.
fn plan_sources(
    root: &PinnedDir,
    sources: &[String],
    target_dir: &str,
    itself_error: &str,
    allow_symlink: bool,
) -> Result<Vec<Planned>, String> {
    let mut planned = Vec::with_capacity(sources.len());
    for source in sources {
        let relative = SafeRelativePath::parse(source)?;
        let parent = Rc::new(root.open_subdir(split_parent(source).0)?);
        let leaf = relative.leaf().clone();
        let kind = parent.existing_kind(&leaf)?.ok_or("source-not-found")?;
        match kind {
            NodeKind::Symlink if !allow_symlink => return Err("copy-symlink-unsupported".into()),
            NodeKind::Other => return Err("copy-unsupported-kind".into()),
            NodeKind::Directory => {
                let id = parent.open_subdir(leaf.as_str())?.id_key();
                if target_within(root, target_dir, &id)? {
                    return Err(itself_error.into());
                }
            }
            _ => {}
        }
        planned.push(Planned { parent, leaf, kind });
    }
    Ok(planned)
}

pub fn copy_into(
    root: &PinnedDir,
    sources: &[String],
    target_dir: &str,
) -> Result<Vec<String>, String> {
    copy_into_with(root, sources, target_dir, DEFAULT_LIMITS)
}

fn copy_into_with(
    root: &PinnedDir,
    sources: &[String],
    target_dir: &str,
    limits: Limits,
) -> Result<Vec<String>, String> {
    let target = open_target(root, target_dir)?;
    let planned = plan_sources(root, sources, target_dir, "copy-into-itself", false)?;
    let mut budget = Budget::new(limits);
    let mut results = Vec::with_capacity(planned.len());
    for item in planned {
        let src = Src::Pinned {
            parent: item.parent,
            leaf: item.leaf.clone(),
        };
        let leaf = copy_top(&src, item.kind, item.leaf.as_str(), &target, &mut budget)?;
        results.push(join(target_dir, leaf.as_str()));
    }
    Ok(results)
}

pub fn move_into(
    root: &PinnedDir,
    sources: &[String],
    target_dir: &str,
) -> Result<Vec<String>, String> {
    let target = open_target(root, target_dir)?;
    let planned = plan_sources(root, sources, target_dir, "move-into-itself", true)?;
    let target_id = target.id_key();
    let mut results = Vec::with_capacity(planned.len());
    for (source, item) in sources.iter().zip(planned) {
        if item.parent.id_key() == target_id {
            results.push(source.clone());
            continue;
        }
        let leaf = unique_leaf(
            &target,
            item.leaf.as_str(),
            item.kind == NodeKind::Directory,
        )?;
        item.parent.rename_entry_new(&item.leaf, &target, &leaf)?;
        results.push(join(target_dir, leaf.as_str()));
    }
    Ok(results)
}

const MAX_IMPORT_SOURCES: usize = 4096;

/// Absolute host paths for `import_into`; relative or `..` spellings never name a source.
pub fn import_sources(sources: &[String]) -> Result<Vec<PathBuf>, String> {
    if sources.is_empty() || sources.len() > MAX_IMPORT_SOURCES {
        return Err("import-source-invalid".into());
    }
    sources
        .iter()
        .map(|source| {
            let path = PathBuf::from(source);
            let escapes = path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir));
            if source.contains('\0') || !path.is_absolute() || escapes {
                return Err("import-source-invalid".to_string());
            }
            Ok(path)
        })
        .collect()
}

/// `workspace` is the path `root` was opened from; a source directory that
/// contains it would copy itself into itself, so it is refused before any copy.
pub fn import_into(
    root: &PinnedDir,
    workspace: &Path,
    sources: &[PathBuf],
    target_dir: &str,
) -> Result<Vec<String>, String> {
    import_into_with(root, workspace, sources, target_dir, DEFAULT_LIMITS)
}

fn import_into_with(
    root: &PinnedDir,
    workspace: &Path,
    sources: &[PathBuf],
    target_dir: &str,
    limits: Limits,
) -> Result<Vec<String>, String> {
    let target = open_target(root, target_dir)?;
    let workspace = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
    let mut planned = Vec::with_capacity(sources.len());
    for source in sources {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("import-source-invalid")?;
        // A top-level link is followed to its target (Finder pastes the target).
        let metadata = std::fs::metadata(source).map_err(|e| e.to_string())?;
        let kind = if metadata.is_dir() {
            let canonical = std::fs::canonicalize(source).map_err(|e| e.to_string())?;
            if workspace.starts_with(&canonical) {
                return Err("copy-into-itself".into());
            }
            let id = PinnedDir::open_dir(&canonical)?.id_key();
            if target_within(root, target_dir, &id)? {
                return Err("copy-into-itself".into());
            }
            NodeKind::Directory
        } else if metadata.is_file() {
            NodeKind::File
        } else {
            return Err("copy-unsupported-kind".into());
        };
        planned.push((Src::External(source.clone()), kind, name.to_owned()));
    }
    let mut budget = Budget::new(limits);
    let mut results = Vec::with_capacity(planned.len());
    for (src, kind, name) in planned {
        let leaf = copy_top(&src, kind, &name, &target, &mut budget)?;
        results.push(join(target_dir, leaf.as_str()));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, PinnedDir) {
        let tmp = tempfile::tempdir().unwrap();
        let root = PinnedDir::open_dir(tmp.path()).unwrap();
        (tmp, root)
    }

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn import_sources_must_be_absolute_and_free_of_parent_components() {
        let ok = import_sources(&s(&["/mnt/c/a b", "/home/u/x"])).unwrap();
        assert_eq!(
            ok,
            vec![PathBuf::from("/mnt/c/a b"), PathBuf::from("/home/u/x")]
        );
        for bad in ["", "rel/a", "./a", "/a/../b", "/mnt/c/..", "/a\0b"] {
            assert_eq!(
                import_sources(&s(&[bad])).unwrap_err(),
                "import-source-invalid",
                "{bad:?}"
            );
        }
        assert!(import_sources(&[]).is_err());
        assert!(import_sources(&vec!["/a".to_string(); MAX_IMPORT_SOURCES + 1]).is_err());
    }

    #[test]
    fn copies_a_file_into_another_directory() {
        let (tmp, root) = setup();
        fs::create_dir(tmp.path().join("dest")).unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let out = copy_into(&root, &s(&["a.txt"]), "dest").unwrap();
        assert_eq!(out, s(&["dest/a.txt"]));
        assert_eq!(fs::read(tmp.path().join("dest/a.txt")).unwrap(), b"hello");
        assert!(tmp.path().join("a.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_unix_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, root) = setup();
        fs::write(tmp.path().join("run.sh"), b"#!/bin/sh").unwrap();
        fs::set_permissions(tmp.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        copy_into(&root, &s(&["run.sh"]), "").unwrap();
        let mode = fs::metadata(tmp.path().join("run copy.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn copies_directory_trees_with_nested_bytes() {
        let (tmp, root) = setup();
        fs::create_dir_all(tmp.path().join("src/a/b")).unwrap();
        fs::create_dir(tmp.path().join("dest")).unwrap();
        fs::write(tmp.path().join("src/top.bin"), [0u8, 159, 146, 150]).unwrap();
        fs::write(tmp.path().join("src/a/b/deep.txt"), b"deep").unwrap();
        fs::create_dir(tmp.path().join("src/empty")).unwrap();
        let big = vec![7u8; 200_000];
        fs::write(tmp.path().join("src/a/big.dat"), &big).unwrap();
        let out = copy_into(&root, &s(&["src"]), "dest").unwrap();
        assert_eq!(out, s(&["dest/src"]));
        let copy = tmp.path().join("dest/src");
        assert_eq!(
            fs::read(copy.join("top.bin")).unwrap(),
            [0u8, 159, 146, 150]
        );
        assert_eq!(fs::read(copy.join("a/b/deep.txt")).unwrap(), b"deep");
        assert_eq!(fs::read(copy.join("a/big.dat")).unwrap(), big);
        assert!(copy.join("empty").is_dir());
    }

    #[test]
    fn collisions_get_unique_finder_style_names() {
        let (tmp, root) = setup();
        fs::write(tmp.path().join("a.txt"), b"1").unwrap();
        fs::write(tmp.path().join(".env"), b"2").unwrap();
        fs::write(tmp.path().join("archive.tar.gz"), b"3").unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::create_dir(tmp.path().join("v1.2")).unwrap();
        let mut got = Vec::new();
        for _ in 0..3 {
            got.extend(copy_into(&root, &s(&["a.txt"]), "").unwrap());
        }
        assert_eq!(got, s(&["a copy.txt", "a copy 2.txt", "a copy 3.txt"]));
        assert_eq!(
            copy_into(&root, &s(&[".env"]), "").unwrap(),
            s(&[".env copy"])
        );
        assert_eq!(
            copy_into(&root, &s(&[".env"]), "").unwrap(),
            s(&[".env copy 2"])
        );
        assert_eq!(
            copy_into(&root, &s(&["archive.tar.gz"]), "").unwrap(),
            s(&["archive.tar copy.gz"])
        );
        assert_eq!(
            copy_into(&root, &s(&["src"]), "").unwrap(),
            s(&["src copy"])
        );
        assert_eq!(
            copy_into(&root, &s(&["src"]), "").unwrap(),
            s(&["src copy 2"])
        );
        assert_eq!(
            copy_into(&root, &s(&["v1.2"]), "").unwrap(),
            s(&["v1.2 copy"])
        );
        // A batch with the same source twice never collides with itself.
        assert_eq!(
            copy_into(&root, &s(&["a.txt", "a.txt"]), "").unwrap(),
            s(&["a copy 4.txt", "a copy 5.txt"])
        );
    }

    #[test]
    fn rejects_copying_a_directory_into_itself_or_a_descendant() {
        let (tmp, root) = setup();
        fs::create_dir_all(tmp.path().join("src/inner")).unwrap();
        for target in ["src", "src/inner"] {
            assert_eq!(
                copy_into(&root, &s(&["src"]), target).unwrap_err(),
                "copy-into-itself"
            );
        }
        assert_eq!(
            fs::read_dir(tmp.path().join("src/inner")).unwrap().count(),
            0
        );
        // A sibling named with the same prefix is not a descendant.
        fs::create_dir(tmp.path().join("src2")).unwrap();
        assert_eq!(
            copy_into(&root, &s(&["src"]), "src2").unwrap(),
            s(&["src2/src"])
        );
    }

    #[test]
    fn rejects_missing_sources_and_unsafe_paths_before_copying_anything() {
        let (tmp, root) = setup();
        fs::write(tmp.path().join("a.txt"), b"1").unwrap();
        assert!(copy_into(&root, &s(&["a.txt", "missing"]), "").is_err());
        assert!(!tmp.path().join("a copy.txt").exists());
        assert!(copy_into(&root, &s(&["../a.txt"]), "").is_err());
        assert!(copy_into(&root, &s(&["a.txt"]), "../x").is_err());
        assert!(copy_into(&root, &s(&[""]), "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_directories_are_skipped_and_top_level_links_rejected() {
        let (tmp, root) = setup();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/real.txt"), b"real").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            tmp.path().join("src/link.txt"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("src/dirlink")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("toplink")).unwrap();
        copy_into(&root, &s(&["src"]), "").unwrap();
        let copy = tmp.path().join("src copy");
        assert!(copy.join("real.txt").exists());
        assert!(fs::symlink_metadata(copy.join("link.txt")).is_err());
        assert!(fs::symlink_metadata(copy.join("dirlink")).is_err());
        assert_eq!(
            copy_into(&root, &s(&["toplink"]), "").unwrap_err(),
            "copy-symlink-unsupported"
        );
    }

    #[test]
    fn exceeding_a_budget_fails_and_removes_the_partial_copy() {
        let (tmp, root) = setup();
        fs::create_dir(tmp.path().join("src")).unwrap();
        for index in 0..6 {
            fs::write(tmp.path().join(format!("src/f{index}.txt")), [1u8; 100]).unwrap();
        }
        let entries = Limits {
            max_entries: 3,
            ..DEFAULT_LIMITS
        };
        assert_eq!(
            copy_into_with(&root, &s(&["src"]), "", entries).unwrap_err(),
            "copy-too-large"
        );
        assert!(!tmp.path().join("src copy").exists());
        let bytes = Limits {
            max_bytes: 250,
            ..DEFAULT_LIMITS
        };
        assert_eq!(
            copy_into_with(&root, &s(&["src"]), "", bytes).unwrap_err(),
            "copy-too-large"
        );
        assert!(!tmp.path().join("src copy").exists());
        let expired = Limits {
            timeout: Duration::ZERO,
            ..DEFAULT_LIMITS
        };
        assert_eq!(
            copy_into_with(&root, &s(&["src"]), "", expired).unwrap_err(),
            "copy-too-large"
        );
        assert!(!tmp.path().join("src copy").exists());
        // Within budget the same copy succeeds.
        copy_into(&root, &s(&["src"]), "").unwrap();
        assert_eq!(
            fs::read_dir(tmp.path().join("src copy")).unwrap().count(),
            6
        );
    }

    #[test]
    fn a_failed_copy_never_removes_a_preexisting_entry() {
        let (tmp, root) = setup();
        fs::write(tmp.path().join("a.txt"), b"keep").unwrap();
        let expired = Limits {
            timeout: Duration::ZERO,
            ..DEFAULT_LIMITS
        };
        assert!(copy_into_with(&root, &s(&["a.txt"]), "", expired).is_err());
        assert_eq!(fs::read(tmp.path().join("a.txt")).unwrap(), b"keep");
    }

    #[test]
    fn moves_into_a_directory_and_keeps_same_directory_as_a_noop() {
        let (tmp, root) = setup();
        fs::create_dir_all(tmp.path().join("dest")).unwrap();
        fs::create_dir_all(tmp.path().join("dir/sub")).unwrap();
        fs::write(tmp.path().join("dir/a.txt"), b"a").unwrap();
        fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        let out = move_into(&root, &s(&["dir/a.txt", "b.txt", "dir/sub"]), "dest").unwrap();
        assert_eq!(out, s(&["dest/a.txt", "dest/b.txt", "dest/sub"]));
        assert!(!tmp.path().join("dir/a.txt").exists());
        assert_eq!(fs::read(tmp.path().join("dest/a.txt")).unwrap(), b"a");
        assert!(tmp.path().join("dest/sub").is_dir());
        // Already in the target directory.
        assert_eq!(
            move_into(&root, &s(&["dest/b.txt"]), "dest").unwrap(),
            s(&["dest/b.txt"])
        );
        assert_eq!(fs::read(tmp.path().join("dest/b.txt")).unwrap(), b"b");
        assert_eq!(fs::read_dir(tmp.path().join("dest")).unwrap().count(), 3);
    }

    #[test]
    fn move_collisions_rename_instead_of_replacing() {
        let (tmp, root) = setup();
        fs::create_dir(tmp.path().join("dest")).unwrap();
        fs::write(tmp.path().join("dest/a.txt"), b"old").unwrap();
        fs::write(tmp.path().join("a.txt"), b"new").unwrap();
        let out = move_into(&root, &s(&["a.txt"]), "dest").unwrap();
        assert_eq!(out, s(&["dest/a copy.txt"]));
        assert_eq!(fs::read(tmp.path().join("dest/a.txt")).unwrap(), b"old");
        assert_eq!(
            fs::read(tmp.path().join("dest/a copy.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn rejects_moving_a_directory_into_itself_or_a_descendant() {
        let (tmp, root) = setup();
        fs::create_dir_all(tmp.path().join("src/inner")).unwrap();
        for target in ["src", "src/inner"] {
            assert_eq!(
                move_into(&root, &s(&["src"]), target).unwrap_err(),
                "move-into-itself"
            );
        }
        assert!(tmp.path().join("src/inner").is_dir());
    }

    #[test]
    fn imports_an_external_directory_tree() {
        let (tmp, root) = setup();
        let external = tempfile::tempdir().unwrap();
        let tree = external.path().join("photos");
        fs::create_dir_all(tree.join("2026")).unwrap();
        fs::write(tree.join("2026/a.jpg"), b"jpg").unwrap();
        fs::write(external.path().join("note.txt"), b"note").unwrap();
        fs::create_dir(tmp.path().join("dest")).unwrap();
        let out = import_into(
            &root,
            tmp.path(),
            &[tree.clone(), external.path().join("note.txt")],
            "dest",
        )
        .unwrap();
        assert_eq!(out, s(&["dest/photos", "dest/note.txt"]));
        assert_eq!(
            fs::read(tmp.path().join("dest/photos/2026/a.jpg")).unwrap(),
            b"jpg"
        );
        let again = import_into(&root, tmp.path(), &[tree], "dest").unwrap();
        assert_eq!(again, s(&["dest/photos copy"]));
        assert!(import_into(&root, tmp.path(), &[external.path().join("missing")], "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn import_follows_top_level_links_but_never_links_inside_directories() {
        let (tmp, root) = setup();
        let external = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        fs::write(secret.path().join("s.txt"), b"s").unwrap();
        fs::write(external.path().join("target.txt"), b"target").unwrap();
        std::os::unix::fs::symlink(
            external.path().join("target.txt"),
            external.path().join("link.txt"),
        )
        .unwrap();
        fs::create_dir(external.path().join("dir")).unwrap();
        fs::write(external.path().join("dir/ok.txt"), b"ok").unwrap();
        std::os::unix::fs::symlink(secret.path(), external.path().join("dir/escape")).unwrap();
        let out = import_into(
            &root,
            tmp.path(),
            &[
                external.path().join("link.txt"),
                external.path().join("dir"),
            ],
            "",
        )
        .unwrap();
        assert_eq!(out, s(&["link.txt", "dir"]));
        assert_eq!(fs::read(tmp.path().join("link.txt")).unwrap(), b"target");
        assert!(!fs::symlink_metadata(tmp.path().join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(tmp.path().join("dir/ok.txt").exists());
        assert!(fs::symlink_metadata(tmp.path().join("dir/escape")).is_err());
    }

    #[test]
    fn importing_the_workspace_into_itself_is_rejected() {
        let (tmp, root) = setup();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        assert_eq!(
            import_into(&root, tmp.path(), &[tmp.path().to_path_buf()], "sub").unwrap_err(),
            "copy-into-itself"
        );
        assert_eq!(
            import_into(&root, tmp.path(), &[tmp.path().join("sub")], "sub").unwrap_err(),
            "copy-into-itself"
        );
    }

    #[test]
    fn importing_a_folder_that_contains_the_workspace_copies_nothing() {
        let outer = tempfile::tempdir().unwrap();
        let project = outer.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let sibling = outer.path().join("sibling");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("a.txt"), b"a").unwrap();
        let root = PinnedDir::open_dir(&project).unwrap();
        assert_eq!(
            import_into(
                &root,
                &project,
                &[sibling.clone(), outer.path().to_path_buf()],
                ""
            )
            .unwrap_err(),
            "copy-into-itself"
        );
        assert_eq!(fs::read_dir(&project).unwrap().count(), 0);
        let out = import_into(&root, &project, &[sibling], "").unwrap();
        assert_eq!(out, s(&["sibling"]));
    }

    #[test]
    fn unique_leaf_naming_rule() {
        let (tmp, root) = setup();
        fs::write(tmp.path().join("noext"), b"").unwrap();
        fs::write(tmp.path().join("noext copy"), b"").unwrap();
        assert_eq!(
            unique_leaf(&root, "noext", false).unwrap().as_str(),
            "noext copy 2"
        );
        assert_eq!(
            unique_leaf(&root, "fresh.md", false).unwrap().as_str(),
            "fresh.md"
        );
    }
}
