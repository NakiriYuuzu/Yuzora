//! Bounded discovery of Git repositories below a workspace directory, so a
//! folder that is not itself a repository (or that nests more repositories)
//! can list every repository it contains. Filesystem-only: no Git is spawned.
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Directory levels below the workspace that are searched.
pub const MAX_DEPTH: usize = 6;
const MAX_DIRECTORIES: usize = 50_000;
const MAX_REPOSITORIES: usize = 200;
const TIME_BUDGET: Duration = Duration::from_secs(5);

/// Dependency, build-output and tool-cache directories that never hold the
/// user's own repositories but can contain huge trees.
const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    "bower_components",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    ".gradle",
    ".terraform",
    ".idea",
    ".vscode",
    "Pods",
    "DerivedData",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRepository {
    /// `/`-separated path relative to the workspace; empty for the workspace itself.
    pub relative_path: String,
    pub name: String,
    /// `.git` is a file: a submodule or a linked worktree.
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiscovery {
    pub repositories: Vec<DiscoveredRepository>,
    /// A budget (time, directory count or repository count) stopped the walk.
    pub truncated: bool,
}

pub fn discover_repositories(workspace: &Path) -> Result<GitDiscovery, String> {
    discover_with_budget(workspace, Instant::now() + TIME_BUDGET)
}

fn discover_with_budget(workspace: &Path, deadline: Instant) -> Result<GitDiscovery, String> {
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("git discovery could not resolve workspace: {e}"))?;
    let workspace_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut repositories = Vec::new();
    let mut truncated = false;
    let mut visited = 0usize;
    let mut queue: VecDeque<(PathBuf, Vec<String>)> = VecDeque::from([(root, Vec::new())]);

    while let Some((dir, components)) = queue.pop_front() {
        visited += 1;
        if visited > MAX_DIRECTORIES || Instant::now() >= deadline {
            truncated = true;
            break;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(dir.join(".git")) {
            if metadata.is_dir() || metadata.is_file() {
                repositories.push(DiscoveredRepository {
                    relative_path: components.join("/"),
                    name: components
                        .last()
                        .cloned()
                        .unwrap_or_else(|| workspace_name.clone()),
                    linked: metadata.is_file(),
                });
                if repositories.len() >= MAX_REPOSITORIES {
                    truncated = true;
                    break;
                }
            }
        }
        if components.len() >= MAX_DEPTH {
            continue;
        }
        // Unreadable directories (permissions, races) are skipped, not fatal.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<String> = entries
            .filter_map(Result::ok)
            // DirEntry::file_type does not follow symlinks, so links are never walked.
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !SKIPPED_DIRECTORIES.contains(&name.as_str()))
            .collect();
        children.sort();
        for name in children {
            let mut next = components.clone();
            next.push(name.clone());
            queue.push_back((dir.join(&name), next));
        }
    }

    repositories.sort_by(|a, b| {
        (!a.relative_path.is_empty(), a.relative_path.to_lowercase())
            .cmp(&(!b.relative_path.is_empty(), b.relative_path.to_lowercase()))
    });
    Ok(GitDiscovery {
        repositories,
        truncated,
    })
}

/// Resolves where repository detection starts for a multi-repository
/// workspace. `None` (or the workspace itself) keeps the workspace path, so a
/// workspace inside a parent repository still resolves that parent. A nested
/// path, relative or absolute, must stay inside the workspace after symlinks
/// are resolved. An absolute path outside the workspace (the parent repository
/// root reported for the workspace) also falls back to the workspace path.
pub fn repository_target(
    workspace: &Path,
    repository_path: Option<&str>,
) -> Result<PathBuf, String> {
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("git could not resolve workspace: {e}"))?;
    let Some(requested) = repository_path.filter(|value| !value.is_empty()) else {
        return Ok(root);
    };
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        if requested.contains('\0')
            || requested_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "git repository path is not a plain relative path: {requested}"
            ));
        }
        root.join(requested_path)
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("git could not resolve repository {requested}: {e}"))?;
    if canonical.starts_with(&root) {
        Ok(canonical)
    } else if requested_path.is_absolute() {
        Ok(root)
    } else {
        Err(format!(
            "git repository path escapes the workspace: {requested}"
        ))
    }
}

/// A nested repository selection must resolve to a repository inside the
/// workspace; a non-repository subfolder would otherwise resolve upwards.
pub fn require_root_inside(workspace: &Path, root: &str) -> Result<(), String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("git could not resolve workspace: {e}"))?;
    let root = Path::new(root)
        .canonicalize()
        .map_err(|e| format!("git could not resolve repository root: {e}"))?;
    if root.starts_with(&workspace) {
        Ok(())
    } else {
        Err("git repository is not inside the workspace".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    fn paths(discovery: &GitDiscovery) -> Vec<&str> {
        discovery
            .repositories
            .iter()
            .map(|repo| repo.relative_path.as_str())
            .collect()
    }

    #[test]
    fn lists_nested_repositories_below_a_plain_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("api"));
        repo(&root.join("apps/web"));
        repo(&root.join("apps/web/packages/ui"));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("libs/core")).unwrap();
        std::fs::write(
            root.join("libs/core/.git"),
            "gitdir: ../../.git/modules/core\n",
        )
        .unwrap();

        let discovery = discover_repositories(root).unwrap();
        assert_eq!(
            paths(&discovery),
            ["api", "apps/web", "apps/web/packages/ui", "libs/core"]
        );
        assert!(!discovery.truncated);
        let linked = discovery
            .repositories
            .iter()
            .find(|r| r.relative_path == "libs/core")
            .unwrap();
        assert!(linked.linked);
        assert_eq!(linked.name, "core");
    }

    #[test]
    fn lists_the_workspace_itself_first_and_skips_dependency_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        repo(&root.join("node_modules/pkg"));
        repo(&root.join("target/debug/x"));
        repo(&root.join("Zeta"));
        repo(&root.join("alpha"));

        let discovery = discover_repositories(root).unwrap();
        assert_eq!(paths(&discovery), ["", "alpha", "Zeta"]);
        assert_eq!(
            discovery.repositories[0].name,
            root.canonicalize()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
    }

    #[test]
    fn stops_at_the_depth_limit_and_reports_budget_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("1/2/3/4/5/6"));
        repo(&root.join("1/2/3/4/5/6/7"));
        assert_eq!(
            paths(&discover_repositories(root).unwrap()),
            ["1/2/3/4/5/6"]
        );

        let expired = discover_with_budget(root, Instant::now()).unwrap();
        assert!(expired.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinked_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        repo(&outside.path().join("secret"));
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        assert!(discover_repositories(tmp.path())
            .unwrap()
            .repositories
            .is_empty());
    }

    #[test]
    fn repository_target_stays_inside_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("apps/web")).unwrap();

        assert_eq!(repository_target(&root, None).unwrap(), root);
        assert_eq!(repository_target(&root, Some("")).unwrap(), root);
        assert_eq!(
            repository_target(&root, Some("apps/web")).unwrap(),
            root.join("apps/web")
        );
        let absolute = root.join("apps/web");
        assert_eq!(
            repository_target(&root, absolute.to_str()).unwrap(),
            absolute
        );
        assert!(repository_target(&root, Some("../x")).is_err());
        assert!(repository_target(&root, Some("apps/missing")).is_err());
        // An absolute parent-repository root falls back to the workspace.
        assert_eq!(
            repository_target(&root, tmp.path().parent().unwrap().to_str()).unwrap(),
            root
        );

        assert!(require_root_inside(&root, absolute.to_str().unwrap()).is_ok());
        assert!(require_root_inside(&absolute, root.to_str().unwrap()).is_err());
    }
}
