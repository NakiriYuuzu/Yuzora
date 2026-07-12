use ignore::WalkBuilder;
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

const WORKSPACE_PATH_INDEX_CAP: usize = 50_000;
const VCS_METADATA_DIRS: [&str; 3] = [".git", ".hg", ".svn"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePathIndexEntry {
    pub relative_path: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePathIndexResult {
    pub workspace: String,
    pub entries: Vec<WorkspacePathIndexEntry>,
    pub truncated: bool,
}

fn is_vcs_metadata_dir(entry: &ignore::DirEntry) -> bool {
    entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
        && VCS_METADATA_DIRS
            .iter()
            .any(|name| entry.file_name() == *name)
}

fn stable_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                Some(component.as_os_str().to_string_lossy().into_owned())
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn build_workspace_path_index(
    canonical_root: PathBuf,
    cap: usize,
) -> Result<WorkspacePathIndexResult, String> {
    let mut builder = WalkBuilder::new(&canonical_root);
    builder
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .sort_by_file_path(|left, right| left.cmp(right));

    let mut entries = Vec::with_capacity(cap.min(1024));
    let mut truncated = false;

    for walked in builder
        .filter_entry(|entry| !is_vcs_metadata_dir(entry))
        .build()
    {
        let entry = match walked {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }

        let canonical_path = match std::fs::canonicalize(entry.path()) {
            Ok(path) if path.starts_with(&canonical_root) => path,
            _ => continue,
        };
        let relative = match canonical_path.strip_prefix(&canonical_root) {
            Ok(relative) if !relative.as_os_str().is_empty() => relative,
            _ => continue,
        };

        if entries.len() == cap {
            truncated = true;
            break;
        }
        entries.push(WorkspacePathIndexEntry {
            relative_path: stable_relative_path(relative),
            canonical_path: canonical_path.to_string_lossy().into_owned(),
        });
    }

    entries.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.canonical_path.cmp(&right.canonical_path))
    });

    Ok(WorkspacePathIndexResult {
        workspace: canonical_root.to_string_lossy().into_owned(),
        entries,
        truncated,
    })
}

#[tauri::command]
pub async fn workspace_path_index(workspace: String) -> Result<WorkspacePathIndexResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canonical_root = std::fs::canonicalize(&workspace)
            .map_err(|error| format!("invalid workspace path: {error}"))?;
        if !canonical_root.is_dir() {
            return Err("workspace path is not a directory".to_string());
        }
        build_workspace_path_index(canonical_root, WORKSPACE_PATH_INDEX_CAP)
    })
    .await
    .map_err(|error| format!("workspace path index worker failed: {error}"))?
}

#[cfg(test)]
mod workspace_path_index_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn workspace_path_index_honors_ignores_includes_hidden_and_excludes_vcs_metadata() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join(".gitignore"),
            "ignored.txt\nignored-dir/\n",
        );
        write(&temp.path().join("visible.ts"), "visible");
        write(&temp.path().join(".hidden.ts"), "hidden");
        write(&temp.path().join("ignored.txt"), "ignored");
        write(&temp.path().join("ignored-dir/nope.ts"), "ignored");
        write(&temp.path().join(".git/config"), "secret");
        write(&temp.path().join(".hg/store/data"), "secret");
        write(&temp.path().join(".svn/wc.db"), "secret");

        let root = fs::canonicalize(temp.path()).unwrap();
        let result = build_workspace_path_index(root, 50).unwrap();
        let relative: Vec<_> = result
            .entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect();

        assert_eq!(relative, [".gitignore", ".hidden.ts", "visible.ts"]);
        assert!(!result.truncated);
    }

    #[test]
    fn workspace_path_index_is_file_only_contained_and_stably_sorted() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("empty-dir")).unwrap();
        write(&temp.path().join("zeta.ts"), "z");
        write(&temp.path().join("alpha/beta.ts"), "b");
        write(&temp.path().join("alpha/a.ts"), "a");

        let root = fs::canonicalize(temp.path()).unwrap();
        let result = build_workspace_path_index(root.clone(), 50).unwrap();

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["alpha/a.ts", "alpha/beta.ts", "zeta.ts"]
        );
        assert!(result.entries.iter().all(|entry| {
            let canonical = PathBuf::from(&entry.canonical_path);
            canonical.starts_with(&root) && canonical.is_file()
        }));
    }

    #[test]
    fn workspace_path_index_cap_only_marks_truncated_when_an_extra_file_exists() {
        let temp = TempDir::new().unwrap();
        write(&temp.path().join("a.ts"), "a");
        write(&temp.path().join("b.ts"), "b");
        let root = fs::canonicalize(temp.path()).unwrap();

        let exact = build_workspace_path_index(root.clone(), 2).unwrap();
        assert_eq!(exact.entries.len(), 2);
        assert!(!exact.truncated);

        write(&temp.path().join("c.ts"), "c");
        let capped = build_workspace_path_index(root, 2).unwrap();
        assert_eq!(capped.entries.len(), 2);
        assert!(capped.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_path_index_never_follows_file_or_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(&temp.path().join("inside.ts"), "inside");
        write(&outside.path().join("outside.ts"), "outside");
        symlink(
            outside.path().join("outside.ts"),
            temp.path().join("file-link.ts"),
        )
        .unwrap();
        symlink(outside.path(), temp.path().join("dir-link")).unwrap();

        let root = fs::canonicalize(temp.path()).unwrap();
        let result = build_workspace_path_index(root, 50).unwrap();
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["inside.ts"]
        );
    }

    #[test]
    fn workspace_path_index_implementation_does_not_read_file_contents() {
        let source = include_str!("workspace_path_index.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap_or(source);
        let forbidden_calls = [
            ["File::", "open"].concat(),
            ["fs::", "read("].concat(),
            ["fs::", "read_to_string"].concat(),
            ["read_to_", "end"].concat(),
        ];
        for forbidden in forbidden_calls {
            assert!(
                !implementation.contains(&forbidden),
                "workspace path index must not read file contents: {forbidden}"
            );
        }
    }
}
