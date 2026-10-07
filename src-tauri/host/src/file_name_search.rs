use ignore::WalkBuilder;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod pinned;
pub(crate) use pinned::{run_pinned_file_name_search, PinnedSearchRoot};

const MAX_FILES: usize = 200;
const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNameMatch {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub kind: FileNameKind,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileNameKind {
    File,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNameSearchResult {
    pub files: Vec<FileNameMatch>,
    pub incomplete: bool,
}

/// Name-only search: never opens result files or follows directory symlinks.
/// Remote callers use a shorter budget because helper requests are serialized.
pub fn run_file_name_search(
    root: &Path,
    query: &str,
    generation: u64,
    gen_source: &AtomicU64,
    time_budget: Duration,
) -> Result<FileNameSearchResult, String> {
    let started = Instant::now();
    let check_current = || {
        if gen_source.load(Ordering::Relaxed) != generation {
            Err("file-name-search-cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    check_current()?;
    // Match list_dir's root validation, preserving the caller's operational path.
    std::fs::read_dir(root).map_err(|e| format!("read_dir failed: {e}"))?;
    let needle = query.trim().replace('\\', "/").to_lowercase();
    let mut result = FileNameSearchResult::default();
    if needle.is_empty() {
        return Ok(result);
    }
    let walker = WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for (scanned, entry) in walker.enumerate() {
        check_current()?;
        if scanned >= MAX_ENTRIES || started.elapsed() >= time_budget {
            result.incomplete = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.incomplete = true;
                continue;
            }
        };
        if entry.error().is_some() {
            result.incomplete = true;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>();
        let Some(relative) = relative else {
            continue;
        };
        if !relative.join("/").to_lowercase().contains(&needle) {
            continue;
        }
        let (Some(name), Some(path)) = (entry.file_name().to_str(), entry.path().to_str()) else {
            continue;
        };
        result.files.push(FileNameMatch {
            name: name.to_owned(),
            path: path.to_owned(),
            is_dir: false,
            kind: FileNameKind::File,
        });
        if result.files.len() == MAX_FILES {
            result.incomplete = true;
            break;
        }
    }
    check_current()?;
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(root: &Path, query: &str) -> FileNameSearchResult {
        let canonical = std::fs::canonicalize(root).unwrap();
        let local = run_file_name_search(
            &canonical,
            query,
            1,
            &AtomicU64::new(1),
            Duration::from_secs(5),
        )
        .unwrap();
        let mut files = crate::files::WorkspaceFiles::default();
        let opened = files.open(root.to_str().unwrap()).unwrap();
        let pinned = files
            .file_name_search_root(opened["capabilityId"].as_str().unwrap())
            .unwrap();
        let started = Instant::now();
        let remote = run_pinned_file_name_search(
            &pinned,
            query,
            1,
            &AtomicU64::new(1),
            Duration::from_secs(5),
        )
        .unwrap();
        println!(
            "pinned search: incomplete={}, wall_time={:?}",
            remote.incomplete,
            started.elapsed()
        );
        assert_eq!(remote.incomplete, local.incomplete);
        assert_eq!(remote.files.len(), local.files.len());
        if !remote.incomplete {
            assert_eq!(
                serde_json::to_value(&remote).unwrap(),
                serde_json::to_value(local).unwrap()
            );
        }
        remote
    }

    fn write(root: &Path, path: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn file_name_search_respects_ignores_and_includes_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".git/info/exclude");
        std::fs::write(root.join(".git/info/exclude"), "excluded/\n").unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(root.join(".ignore"), "build/\n").unwrap();
        for path in [
            "node_modules/target.ts",
            "build/target.ts",
            "excluded/target.ts",
            ".git/target",
            ".hidden/target.ts",
            "src/target.ts",
        ] {
            write(root, path);
        }
        let result = search(root, "target");
        assert!(!result.incomplete);
        assert_eq!(result.files.len(), 2);
        // Component-wise, so Windows `\` separators match too.
        assert!(Path::new(&result.files[0].path).ends_with(".hidden/target.ts"));
        assert!(Path::new(&result.files[1].path).ends_with("src/target.ts"));
    }

    #[test]
    fn file_name_search_skips_git_file_in_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git");
        assert!(search(tmp.path(), ".git").files.is_empty());
    }

    #[test]
    fn file_name_search_matches_case_insensitive_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "Src/Deep/Target.ts");
        let result = search(tmp.path(), "  SRC\\deep\\TARGET  ");
        assert_eq!(result.files.len(), 1);
        assert!(!result.files[0].is_dir);
        assert!(matches!(result.files[0].kind, FileNameKind::File));
        assert!(search(tmp.path(), tmp.path().to_str().unwrap())
            .files
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_name_search_does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "target.ts");
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("target-dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target.ts"),
            tmp.path().join("target.ts"),
        )
        .unwrap();
        assert!(search(tmp.path(), "target").files.is_empty());
    }

    #[test]
    fn file_name_search_caps_results() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..210 {
            write(tmp.path(), &format!("target-{i}.ts"));
        }
        let result = search(tmp.path(), "target");
        assert_eq!(result.files.len(), MAX_FILES);
        assert!(result.incomplete);
        assert!(result
            .files
            .windows(2)
            .all(|pair| pair[0].path <= pair[1].path));
    }

    #[test]
    fn file_name_search_reports_time_budget() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "target.ts");
        let result =
            run_file_name_search(tmp.path(), "target", 1, &AtomicU64::new(1), Duration::ZERO)
                .unwrap();
        assert!(result.incomplete);
        assert!(result.files.is_empty());
    }

    #[test]
    fn file_name_search_cancels_stale_generation() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "target.ts");
        assert_eq!(
            run_file_name_search(
                tmp.path(),
                "target",
                1,
                &AtomicU64::new(2),
                Duration::from_secs(5)
            )
            .unwrap_err(),
            "file-name-search-cancelled"
        );
    }

    #[test]
    fn file_name_search_rejects_invalid_root() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "target.ts");
        assert!(run_file_name_search(
            &tmp.path().join("target.ts"),
            "target",
            1,
            &AtomicU64::new(1),
            Duration::from_secs(5)
        )
        .is_err());
    }

    #[test]
    #[ignore = "repository timing evidence; run explicitly with --ignored --nocapture"]
    fn file_name_search_worktree_evidence() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let started = Instant::now();
        let result = search(&root, "herdrtoolsdialog");
        let elapsed = started.elapsed();
        assert!(result.files.iter().any(|file| file
            .path
            .ends_with("src/app/workbench/herdr/HerdrToolsDialog.tsx")));
        assert!(!result.incomplete);
        println!(
            "herdrtoolsdialog: incomplete={}, wall_time={elapsed:?}, paths={:?}",
            result.incomplete,
            result
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }
}
