use crate::content::classify_bytes;
use crate::path_capability::{NodeKind, PinnedDir, SafeLeafName, SafeRelativePath};
use crate::protocol::MAX_FILE_BYTES;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

struct Workspace {
    canonical: String,
    root: PinnedDir,
}

#[derive(Default)]
pub struct WorkspaceFiles {
    roots: HashMap<String, Workspace>,
}

fn digest(bytes: &[u8], metadata: &std::fs::Metadata, identity: &str) -> String {
    let hash = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{identity}:{}:{:?}:{hash}",
        metadata.len(),
        metadata.modified().ok()
    )
}

fn read_bounded(mut file: std::fs::File) -> Result<(Vec<u8>, std::fs::Metadata, String), String> {
    let identity = crate::path_capability::opened_file_identity(&file)?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if before.len() > MAX_FILE_BYTES {
        return Err("file-too-large".into());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("file-too-large".into());
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    if before.len() != after.len()
        || before.modified().map_err(|e| e.to_string())?
            != after.modified().map_err(|e| e.to_string())?
    {
        return Err("file-changed-during-read".into());
    }
    Ok((bytes, after, identity))
}

fn sort_listing_entries(entries: &mut [(String, NodeKind)]) {
    // Cache Unicode keys before JSON construction; equal keys keep reader order.
    entries.sort_by_cached_key(|(name, kind)| (*kind != NodeKind::Directory, name.to_lowercase()));
}

impl WorkspaceFiles {
    pub fn open(&mut self, path: &str) -> Result<Value, String> {
        if self.roots.len() >= 128 {
            return Err("too-many-workspaces".into());
        }
        let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        let root = PinnedDir::open_dir(&canonical)?;
        let canonical = canonical.to_str().ok_or("path-not-utf8")?.to_owned();
        let id = format!("workspace-{:032x}", rand::random::<u128>());
        let result = json!({"canonicalPath":canonical,"capabilityId":id});
        self.roots.insert(id, Workspace { canonical, root });
        Ok(result)
    }

    pub fn close(&mut self, id: &str) {
        self.roots.remove(id);
    }

    pub fn canonical_root(&self, id: &str) -> Result<&str, String> {
        Ok(&self.get(id)?.canonical)
    }

    pub(crate) fn file_name_search_root(
        &self,
        id: &str,
    ) -> Result<crate::file_name_search::PinnedSearchRoot, String> {
        let workspace = self.get(id)?;
        Ok(crate::file_name_search::PinnedSearchRoot {
            canonical: workspace.canonical.clone().into(),
            dir: workspace.root.open_subdir("")?,
        })
    }

    pub fn create(&self, id: &str, path: &str, directory: bool) -> Result<Value, String> {
        let root = &self.get(id)?.root;
        let relative = SafeRelativePath::parse(path)?;
        let (parent, _) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = root.open_subdir(parent)?;
        if directory {
            parent.mkdir(relative.leaf())?;
        } else {
            parent
                .create_exclusive(relative.leaf())?
                .sync_all()
                .map_err(|e| e.to_string())?;
        }
        Ok(Value::Null)
    }

    pub fn rename(&self, id: &str, from: &str, to: &str) -> Result<Value, String> {
        let root = &self.get(id)?.root;
        let source = SafeRelativePath::parse(from)?;
        let target = SafeRelativePath::parse(to)?;
        let (source_parent, _) = from.rsplit_once('/').unwrap_or(("", from));
        let (target_parent, _) = to.rsplit_once('/').unwrap_or(("", to));
        root.open_subdir(source_parent)?.rename_new(
            source.leaf(),
            &root.open_subdir(target_parent)?,
            target.leaf(),
        )?;
        Ok(Value::Null)
    }

    pub fn delete(&self, id: &str, path: &str) -> Result<Value, String> {
        let root = &self.get(id)?.root;
        let relative = SafeRelativePath::parse(path)?;
        let (parent, _) = path.rsplit_once('/').unwrap_or(("", path));
        root.open_subdir(parent)?.remove_tree(
            relative.leaf(),
            &mut 50_000,
            std::time::Instant::now() + std::time::Duration::from_secs(10),
        )?;
        Ok(Value::Null)
    }

    /// An owned clone of the workspace root for work moved onto a blocking thread.
    pub(crate) fn pinned_root(&self, id: &str) -> Result<PinnedDir, String> {
        Ok(self.get(id)?.root.open_subdir("")?)
    }

    pub fn copy(&self, id: &str, sources: &[String], target_dir: &str) -> Result<Value, String> {
        let created = crate::file_transfer::copy_into(&self.get(id)?.root, sources, target_dir)?;
        Ok(json!(created))
    }

    pub fn move_paths(
        &self,
        id: &str,
        sources: &[String],
        target_dir: &str,
    ) -> Result<Value, String> {
        let moved = crate::file_transfer::move_into(&self.get(id)?.root, sources, target_dir)?;
        Ok(json!(moved))
    }

    pub fn read_base64(&self, id: &str, path: &str, max_bytes: u64) -> Result<Value, String> {
        use base64::Engine;
        let file = self
            .get(id)?
            .root
            .open_file(&SafeRelativePath::parse(path)?)?;
        if file.len > max_bytes.min(MAX_FILE_BYTES) {
            return Err("file-too-large".into());
        }
        let (bytes, _, _) = read_bounded(file.file)?;
        if bytes.len() as u64 > max_bytes {
            return Err("file-too-large".into());
        }
        Ok(
            json!({"size":bytes.len(),"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}),
        )
    }

    fn get(&self, id: &str) -> Result<&Workspace, String> {
        let workspace = self.roots.get(id).ok_or("workspace-capability-missing")?;
        let current = PinnedDir::open_dir(Path::new(&workspace.canonical))?;
        if current.id_key() != workspace.root.id_key() {
            return Err("workspace-identity-changed".into());
        }
        Ok(workspace)
    }

    pub fn list(&self, id: &str, path: &str) -> Result<Value, String> {
        let workspace = self.get(id)?;
        let dir = workspace.root.open_subdir(path)?;
        let mut listed = dir.list_entries(50_000).map_err(|error| {
            if error == "sftp-tree-entry-limit" {
                "directory-too-large".into()
            } else {
                error
            }
        })?;
        sort_listing_entries(&mut listed);
        let mut entries = Vec::new();
        for (name, kind) in listed {
            let relative = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            entries.push(json!({"name":name,"path":relative,"isDir":kind == NodeKind::Directory,"kind":match kind { NodeKind::Symlink => "symlink", NodeKind::Directory => "directory", NodeKind::File => "file", NodeKind::Other => "other" }}));
            if entries.len() > 50_000 {
                return Err("directory-too-large".into());
            }
        }
        Ok(Value::Array(entries))
    }

    pub fn read(&self, id: &str, path: &str) -> Result<Value, String> {
        let opened = self
            .get(id)?
            .root
            .open_file(&SafeRelativePath::parse(path)?)?;
        if opened.len > MAX_FILE_BYTES {
            return Ok(json!({"file":{"kind":"tooLarge","size":opened.len},"revision":null}));
        }
        let (bytes, metadata, identity) = read_bounded(opened.file)?;
        Ok(json!({"file":classify_bytes(&bytes),"revision":digest(&bytes,&metadata,&identity)}))
    }

    pub fn write(
        &self,
        id: &str,
        path: &str,
        content: &str,
        revision: &str,
    ) -> Result<Value, String> {
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err("file-too-large".into());
        }
        let workspace = self.get(id)?;
        let relative = SafeRelativePath::parse(path)?;
        let (parent, _) = path.rsplit_once('/').unwrap_or(("", path));
        let dir = workspace.root.open_subdir(parent)?;
        let opened = dir.open_file(&SafeRelativePath::parse(relative.leaf().as_str())?)?;
        let (bytes, metadata, identity) = read_bounded(opened.file)?;
        if digest(&bytes, &metadata, &identity) != revision {
            return Err("file-conflict".into());
        }
        let scratch =
            SafeLeafName::parse(&format!(".yuzora-save-{:032x}", rand::random::<u128>()))?;
        let result = (|| -> Result<Value, String> {
            let mut file = dir.create_exclusive(&scratch)?;
            #[cfg(unix)]
            file.set_permissions(std::fs::Permissions::from_mode(metadata.mode() & 0o777))
                .map_err(|e| e.to_string())?;
            #[cfg(windows)]
            file.set_permissions(metadata.permissions())
                .map_err(|e| e.to_string())?;
            file.write_all(content.as_bytes())
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            // DrvFS can keep the renamed destination invisible to a new openat
            // until the scratch writer closes. Finish that handle before the
            // replacement so the response can read back the committed revision.
            drop(file);
            let current = dir.open_file(&SafeRelativePath::parse(relative.leaf().as_str())?)?;
            let (current_bytes, current_meta, current_identity) = read_bounded(current.file)?;
            if digest(&current_bytes, &current_meta, &current_identity) != revision {
                return Err("file-conflict".into());
            }
            // Serialize helper writes; independent external writers are checked
            // immediately before rename, without claiming filesystem CAS.
            dir.promote(&scratch, relative.leaf())?;
            self.read(id, path)
        })();
        let _ = dir.unlink(&scratch);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_and_move_run_against_the_workspace_capability() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("dest")).unwrap();
        std::fs::write(root.path().join("a.txt"), b"a").unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        let sources = vec!["a.txt".to_string()];
        assert_eq!(
            files.copy(id, &sources, "dest").unwrap(),
            json!(["dest/a.txt"])
        );
        assert_eq!(
            files.move_paths(id, &sources, "dest").unwrap(),
            json!(["dest/a copy.txt"])
        );
        assert!(!root.path().join("a.txt").exists());
        assert!(files.copy("nope", &sources, "dest").is_err());
        assert!(files.copy(id, &sources, "../out").is_err());
    }
    #[test]
    fn new_file_save_returns_the_revision_used_by_the_next_save() {
        // Run with TMPDIR on /mnt/c, /mnt/d and /home for the WSL/DrvFS matrix.
        let root = tempfile::tempdir().unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        let path = "新建 文件.txt";
        files.create(id, path, false).unwrap();
        let mut snapshot = files.read(id, path).unwrap();
        for index in 0..30 {
            let content = format!("第 {index} 次儲存\r\n");
            let saved = files
                .write(id, path, &content, snapshot["revision"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                std::fs::read(root.path().join(path)).unwrap(),
                content.as_bytes()
            );
            let read = files.read(id, path).unwrap();
            assert_eq!(saved["revision"], read["revision"]);
            assert_eq!(saved["file"]["content"], content);
            snapshot = saved;
        }
        std::fs::write(root.path().join(path), "external").unwrap();
        assert_eq!(
            files
                .write(id, path, "mine", snapshot["revision"].as_str().unwrap())
                .unwrap_err(),
            "file-conflict"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(path)).unwrap(),
            "external"
        );
    }

    #[test]
    #[cfg(unix)]
    fn mutations_are_owned_and_never_replace_existing_targets_or_follow_links() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep"), "outside").unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        files.create(id, "中文 folder", true).unwrap();
        files.create(id, "中文 folder/a.txt", false).unwrap();
        files.create(id, "existing", false).unwrap();
        std::fs::write(root.path().join("existing"), "keep").unwrap();
        assert_eq!(files.list(id, "").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(files.list(id, "").unwrap().as_array().unwrap().len(), 2);
        assert!(files.create(id, "existing", false).is_err());
        assert!(files.rename(id, "中文 folder/a.txt", "existing").is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("existing")).unwrap(),
            "keep"
        );
        files
            .rename(id, "中文 folder/a.txt", "中文 folder/b.txt")
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("中文 folder/link")).unwrap();
        assert!(files.create(id, "中文 folder/link/new", false).is_err());
        assert!(files.delete(id, "中文 folder/link/keep").is_err());
        assert!(files.delete(id, "").is_err());
        assert!(files.rename(id, "existing", "../escape").is_err());
        files.delete(id, "中文 folder").unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep")).unwrap(),
            "outside"
        );
        assert!(!root.path().join("中文 folder").exists());
    }

    #[test]
    fn binary_reads_are_bounded_and_root_replacements_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspace");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("image"), [0, 1, 2]).unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(path.to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        assert_eq!(files.read_base64(id, "image", 3).unwrap()["data"], "AAEC");
        assert!(files.read_base64(id, "image", 2).is_err());
        std::fs::rename(&path, root.path().join("old")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            files.create(id, "file", false).unwrap_err(),
            "workspace-identity-changed"
        );
        assert!(!path.join("file").exists());
    }
    #[test]
    fn save_detects_external_change_and_preserves_bytes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "original").unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        let read = files.read(id, "a.txt").unwrap();
        let revision = read["revision"].as_str().unwrap();
        std::fs::write(root.path().join("a.txt"), "external").unwrap();
        assert_eq!(
            files.write(id, "a.txt", "mine", revision).unwrap_err(),
            "file-conflict"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "external"
        );
    }
    #[test]
    #[cfg(unix)]
    fn safe_save_and_symlink_rejection() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "old").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        assert!(files.list(id, "escape").is_err());
        assert!(files.read(id, "../a.txt").is_err());
        let read = files.read(id, "a.txt").unwrap();
        let saved = files
            .write(id, "a.txt", "new", read["revision"].as_str().unwrap())
            .unwrap();
        assert_eq!(saved["file"]["content"], "new");
        files.close(id);
        assert!(files.read(id, "a.txt").is_err());
    }

    #[test]
    fn listing_sort_matches_legacy_json_order_and_stable_unicode_ties() {
        let names = [
            "",
            "a",
            "A",
            "ä",
            "Ä",
            "Σ",
            "σ",
            "ς",
            "İ",
            "i\u{307}",
            "I",
            "i",
            "ß",
            "SS",
            "ss",
            "é",
            "e\u{301}",
            "中文😀",
            "\"quote\"",
            "line\nB",
        ];
        let kinds = [
            NodeKind::File,
            NodeKind::Directory,
            NodeKind::Symlink,
            NodeKind::Other,
        ];
        let records = names
            .iter()
            .flat_map(|name| kinds.iter().map(move |kind| (name.to_string(), *kind)))
            .collect::<Vec<_>>();
        let mut seed = 43_u64;
        for variant in 0..128 {
            let mut actual = records.clone();
            if variant == 0 {
                actual.sort_by(|a, b| a.0.cmp(&b.0));
            } else if variant == 1 {
                actual.reverse();
            } else {
                for i in (1..actual.len()).rev() {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    actual.swap(i, (seed as usize) % (i + 1));
                }
            }
            let before = actual.clone();
            let mut legacy = before.iter().enumerate().map(|(index, (name, kind))| {
                json!({ "name": name, "isDir": *kind == NodeKind::Directory, "index": index })
            }).collect::<Vec<_>>();
            // Original JSON comparator is the oracle, including stable ties.
            legacy.sort_by(|a, b| {
                b["isDir"]
                    .as_bool()
                    .cmp(&a["isDir"].as_bool())
                    .then_with(|| {
                        a["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .cmp(&b["name"].as_str().unwrap_or_default().to_lowercase())
                    })
            });
            let expected = legacy
                .iter()
                .map(|entry| before[entry["index"].as_u64().unwrap() as usize].clone())
                .collect::<Vec<_>>();
            sort_listing_entries(&mut actual);
            assert_eq!(actual, expected, "permutation {variant}");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "files/listing_perf_tests.rs"]
mod listing_perf_tests;
