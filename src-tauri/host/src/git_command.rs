//! Typed Git operations. Repository authority belongs to one workspace capability.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "command",
    content = "args",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GitCommand {
    #[serde(rename = "git_detect")]
    Detect,
    #[serde(rename = "git_bootstrap")]
    Bootstrap,
    #[serde(rename = "git_discover")]
    Discover,
    #[serde(rename = "git_status_cmd")]
    Status { pathspec: Option<Vec<String>> },
    #[serde(rename = "git_stage")]
    Stage { paths: Vec<String> },
    #[serde(rename = "git_unstage")]
    Unstage { paths: Vec<String> },
    #[serde(rename = "git_discard")]
    Discard {
        paths: Vec<String>,
        untracked: Vec<String>,
    },
    #[serde(rename = "git_rollback_paths")]
    Rollback {
        targets: Vec<crate::git_service::GitRollbackTarget>,
        delete_untracked_or_added: bool,
    },
    #[serde(rename = "git_commit_cmd")]
    Commit {
        message: String,
        amend_head: Option<String>,
    },
    #[serde(rename = "git_branches")]
    Branches,
    #[serde(rename = "git_create_branch")]
    CreateBranch {
        name: String,
        start_point: Option<String>,
        smart: Option<bool>,
    },
    #[serde(rename = "git_checkout_detached")]
    CheckoutDetached { rev: String, smart: Option<bool> },
    #[serde(rename = "git_checkout")]
    Checkout { name: String, smart: Option<bool> },
    #[serde(rename = "git_cherry_pick")]
    CherryPick { hash: String },
    #[serde(rename = "git_fetch_cmd")]
    Fetch,
    #[serde(rename = "git_pull_cmd")]
    Pull { mode: Option<String> },
    #[serde(rename = "git_push_cmd")]
    Push {
        force_with_lease: Option<bool>,
        tags: Option<bool>,
    },
    #[serde(rename = "git_merge_branch")]
    MergeBranch { name: String },
    #[serde(rename = "git_rebase_onto")]
    RebaseOnto { upstream: String },
    #[serde(rename = "git_rename_branch")]
    RenameBranch { old_name: String, new_name: String },
    #[serde(rename = "git_delete_branch")]
    DeleteBranch { name: String, force: bool },
    #[serde(rename = "git_revert_commit")]
    RevertCommit { hash: String },
    #[serde(rename = "git_reset_branch")]
    ResetBranch { hash: String, mode: String },
    #[serde(rename = "git_stash_list")]
    StashList,
    #[serde(rename = "git_stash_push")]
    StashPush {
        message: Option<String>,
        include_untracked: bool,
        keep_index: bool,
    },
    #[serde(rename = "git_stash_apply")]
    StashApply { index: u32, oid: String, pop: bool },
    #[serde(rename = "git_stash_drop")]
    StashDrop { index: u32, oid: String },
    #[serde(rename = "git_remote_probe")]
    RemoteProbe,
    #[serde(rename = "git_diff_content")]
    Diff {
        path: String,
        staged: bool,
        orig_path: Option<String>,
    },
    #[serde(rename = "git_conflict_abort")]
    ConflictAbort { op: String },
    #[serde(rename = "git_conflict_continue")]
    ConflictContinue { op: String },
    #[serde(rename = "git_conflict_skip")]
    ConflictSkip { op: String },
    #[serde(rename = "git_conflict_sides")]
    ConflictSides { path: String },
    #[serde(rename = "git_conflict_resolve")]
    ConflictResolve { paths: Vec<String>, side: String },
    #[serde(rename = "git_log_page")]
    Log {
        cursor: Option<String>,
        limit: u32,
        query: Option<String>,
        author: Option<String>,
        since: Option<String>,
        until: Option<String>,
    },
    #[serde(rename = "git_commit_detail")]
    CommitDetail { hash: String },
    #[serde(rename = "git_log_authors")]
    LogAuthors,
    #[serde(rename = "git_file_at_rev")]
    FileAtRev { rev: String, path: String },
}

impl GitCommand {
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Self::Detect
                | Self::Bootstrap
                | Self::Discover
                | Self::Status { .. }
                | Self::Branches
                | Self::RemoteProbe
                | Self::Diff { .. }
                | Self::ConflictSides { .. }
                | Self::StashList
                | Self::Log { .. }
                | Self::CommitDetail { .. }
                | Self::LogAuthors
                | Self::FileAtRev { .. }
        )
    }
}

mod host {
    use super::*;
    use crate::{
        files::WorkspaceFiles,
        git_log,
        git_service::*,
        workspace_trust::{observe_identity, WorkspaceIdentity, WorkspaceTrustState},
    };
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::path::Path;

    #[derive(Default)]
    pub struct HostGit {
        roots: HashMap<String, WorkspaceIdentity>,
    }
    impl HostGit {
        pub fn close(&mut self, workspace: &str) {
            self.roots.remove(workspace);
        }
        pub fn execute(
            &mut self,
            files: &WorkspaceFiles,
            trust: &WorkspaceTrustState,
            workspace: &str,
            requested_root: Option<&str>,
            call: GitCommand,
        ) -> Result<Value, String> {
            let path = files.canonical_root(workspace)?;
            self.execute_root(path, trust, workspace, requested_root, call)
        }
        pub fn execute_root(
            &mut self,
            path: &str,
            trust: &WorkspaceTrustState,
            workspace: &str,
            requested_root: Option<&str>,
            call: GitCommand,
        ) -> Result<Value, String> {
            if matches!(call, GitCommand::Discover) {
                // Lists repository directories only (no Git process), with the
                // same authority as the workspace file tree, so an untrusted
                // folder can still show which repositories it contains.
                return serde_json::to_value(crate::git_discovery::discover_repositories(
                    Path::new(path),
                )?)
                .map_err(|e| e.to_string());
            }
            if matches!(call, GitCommand::Detect | GitCommand::Bootstrap)
                && requested_root.is_none()
                && Path::new(path).is_dir()
                && !crate::workspace_trust::project_repo_presence(path)
            {
                // A non-repository has no Git operation to authorize. Answer
                // like the local detector: no trust and no Git process.
                self.roots.remove(workspace);
                let environment = GitEnvironment::NotARepo;
                return if matches!(call, GitCommand::Bootstrap) {
                    Ok(
                        json!({"environment":environment,"status":null,"branches":null,"snapshotError":null}),
                    )
                } else {
                    serde_json::to_value(environment).map_err(|e| e.to_string())
                };
            }
            let identity = trust.require_trusted(path)?;
            if matches!(call, GitCommand::Detect | GitCommand::Bootstrap) {
                self.roots.remove(workspace);
                // `requested_root` selects a repository nested in the workspace
                // (multi-repository workspaces); `None` detects the workspace.
                let target =
                    crate::git_discovery::repository_target(Path::new(path), requested_root)?;
                let environment = detect_environment(&target);
                if let GitEnvironment::Ready { root, .. } = &environment {
                    if target.as_path() != Path::new(path) {
                        crate::git_discovery::require_root_inside(Path::new(path), root)?;
                    }
                }
                let bootstrap = matches!(call, GitCommand::Bootstrap);
                let mut status = None;
                let mut branch_list = None;
                let mut snapshot_error = None;
                if let GitEnvironment::Ready { root, .. } = &environment {
                    let root_identity = observe_identity(root).map_err(|e| e.to_frontend())?;
                    trust.bind_session_git_root(&identity, root);
                    self.roots.insert(workspace.into(), root_identity);
                    if bootstrap {
                        match status_of(Path::new(root), None).and_then(|status| {
                            branches(Path::new(root)).map(|branches| (status, branches))
                        }) {
                            Ok((s, b)) => {
                                status = Some(s);
                                branch_list = Some(b);
                            }
                            Err(error) => snapshot_error = Some(error),
                        }
                    }
                }
                return if bootstrap {
                    Ok(
                        json!({"environment":environment,"status":status,"branches":branch_list,"snapshotError":snapshot_error}),
                    )
                } else {
                    serde_json::to_value(environment).map_err(|e| e.to_string())
                };
            }
            let expected = self.roots.get(workspace).ok_or("git-repository-not-open")?;
            // Git reports a Windows root as `C:/…` while the stored identity is
            // canonical (`\\?\C:\…`), so compare canonical forms.
            let same_root = requested_root.is_some_and(|requested| {
                requested == expected.canonical_path
                    || observe_identity(requested)
                        .is_ok_and(|seen| seen.canonical_path == expected.canonical_path)
            });
            if !same_root {
                return Err("git-repository-identity-mismatch".into());
            }
            let current =
                observe_identity(&expected.canonical_path).map_err(|e| e.to_frontend())?;
            if &current != expected {
                return Err("git-repository-replaced".into());
            }
            trust.require_trusted_git(&expected.canonical_path)?;
            let root = Path::new(&expected.canonical_path);
            fn value<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
                serde_json::to_value(result?).map_err(|e| e.to_string())
            }
            match call {
                GitCommand::Status { pathspec } => value(status_of(root, pathspec)),
                GitCommand::Stage { paths } => value(stage(root, &paths)),
                GitCommand::Unstage { paths } => value(unstage(root, &paths)),
                GitCommand::Discard { paths, untracked } => {
                    value(discard(root, &paths, &untracked))
                }
                GitCommand::Rollback {
                    targets,
                    delete_untracked_or_added,
                } => value(rollback_paths(root, &targets, delete_untracked_or_added)),
                GitCommand::Commit {
                    message,
                    amend_head,
                } => value(commit_with_options(root, &message, amend_head.as_deref())),
                GitCommand::Branches => value(branches(root)),
                GitCommand::CreateBranch {
                    name,
                    start_point,
                    smart,
                } => value(switch_keeping_changes(root, smart.unwrap_or(false), || {
                    create_branch(root, &name, start_point.as_deref())
                })),
                GitCommand::CheckoutDetached { rev, smart } => {
                    value(switch_keeping_changes(root, smart.unwrap_or(false), || {
                        checkout_detached(root, &rev)
                    }))
                }
                GitCommand::Checkout { name, smart } => {
                    value(switch_keeping_changes(root, smart.unwrap_or(false), || {
                        checkout(root, &name)
                    }))
                }
                GitCommand::CherryPick { hash } => value(cherry_pick(root, &hash)),
                GitCommand::Fetch => {
                    value(run_ok(root, &["fetch"], REMOTE_TIMEOUT, &[]).map(|_| ()))
                }
                GitCommand::Pull { mode } => {
                    value(pull_args_with_mode(root, mode.as_deref()).and_then(|args| {
                        let args: Vec<&str> = args.iter().map(String::as_str).collect();
                        run_ok(root, &args, REMOTE_TIMEOUT, &editor_true()).map(|_| ())
                    }))
                }
                GitCommand::Push {
                    force_with_lease,
                    tags,
                } => value(
                    push_args_with_options(
                        root,
                        force_with_lease.unwrap_or(false),
                        tags.unwrap_or(false),
                    )
                    .and_then(|args| {
                        let args: Vec<&str> = args.iter().map(String::as_str).collect();
                        run_ok(root, &args, REMOTE_TIMEOUT, &[]).map(|_| ())
                    }),
                ),
                GitCommand::MergeBranch { name } => value(merge_branch(root, &name)),
                GitCommand::RebaseOnto { upstream } => value(rebase_onto(root, &upstream)),
                GitCommand::RenameBranch { old_name, new_name } => {
                    value(rename_branch(root, &old_name, &new_name))
                }
                GitCommand::DeleteBranch { name, force } => {
                    value(delete_branch(root, &name, force))
                }
                GitCommand::RevertCommit { hash } => value(revert_commit(root, &hash)),
                GitCommand::ResetBranch { hash, mode } => value(reset_branch(root, &hash, &mode)),
                GitCommand::StashList => value(stash_list(root)),
                GitCommand::StashPush {
                    message,
                    include_untracked,
                    keep_index,
                } => value(stash_push(
                    root,
                    message.as_deref(),
                    include_untracked,
                    keep_index,
                )),
                GitCommand::StashApply { index, oid, pop } => {
                    value(stash_apply(root, index, &oid, pop))
                }
                GitCommand::StashDrop { index, oid } => value(stash_drop(root, index, &oid)),
                GitCommand::RemoteProbe => value(remote_probe(root, &[])),
                GitCommand::Diff {
                    path,
                    staged,
                    orig_path,
                } => value(diff_content(root, &path, staged, orig_path.as_deref())),
                GitCommand::ConflictAbort { op } => value(conflict_abort(root, &op)),
                GitCommand::ConflictContinue { op } => value(conflict_continue(root, &op)),
                GitCommand::ConflictSkip { op } => value(conflict_skip(root, &op)),
                GitCommand::ConflictSides { path } => value(conflict_sides(root, &path)),
                GitCommand::ConflictResolve { paths, side } => {
                    value(conflict_resolve(root, &paths, &side))
                }
                GitCommand::Log {
                    cursor,
                    limit,
                    query,
                    author,
                    since,
                    until,
                } => value(git_log::log_page(
                    root,
                    cursor.as_deref(),
                    limit,
                    query.as_deref(),
                    author.as_deref(),
                    since.as_deref(),
                    until.as_deref(),
                )),
                GitCommand::CommitDetail { hash } => value(git_log::commit_detail(root, &hash)),
                GitCommand::LogAuthors => value(git_log::log_authors(root)),
                GitCommand::FileAtRev { rev, path } => {
                    value(git_log::file_at_rev(root, &rev, &path))
                }
                GitCommand::Detect | GitCommand::Bootstrap | GitCommand::Discover => unreachable!(),
            }
        }
    }
}
pub use host::HostGit;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_service::test_repo;
    use crate::workspace_trust::WorkspaceTrustState;

    #[test]
    fn remote_workspace_discovers_and_opens_a_nested_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = WorkspaceTrustState::at(tmp.path().join("trust.json"));
        std::fs::create_dir_all(tmp.path().join("plain")).unwrap();
        let workspace = tmp.path().join("plain").canonicalize().unwrap();
        let nested = workspace.join("services/api");
        std::fs::create_dir_all(&nested).unwrap();
        test_repo::init(&nested);
        test_repo::write_and_commit(&nested, "a.txt", "1", "c1");
        let path = workspace.to_str().unwrap();
        let mut git = HostGit::default();

        // Discovery needs no trust: it never runs Git.
        let found = git
            .execute_root(path, &trust, "ws", None, GitCommand::Discover)
            .unwrap();
        assert_eq!(found["repositories"][0]["relativePath"], "services/api");

        // The plain folder itself is not a repository: no trust needed.
        let plain = git
            .execute_root(path, &trust, "ws", None, GitCommand::Bootstrap)
            .unwrap();
        assert_eq!(plain["environment"]["status"], "notARepo");
        assert!(plain["status"].is_null());
        assert_eq!(
            git.execute_root(path, &trust, "ws", None, GitCommand::Detect)
                .unwrap()["status"],
            "notARepo"
        );

        assert!(git
            .execute_root(
                path,
                &trust,
                "ws",
                Some("services/api"),
                GitCommand::Bootstrap
            )
            .unwrap_err()
            .contains("untrustedWorkspace"));
        trust.0.grant_for_tests(path);
        let opened = git
            .execute_root(
                path,
                &trust,
                "ws",
                Some("services/api"),
                GitCommand::Bootstrap,
            )
            .unwrap();
        let root = opened["environment"]["root"].as_str().unwrap().to_string();
        assert_eq!(
            std::path::Path::new(&root).canonicalize().unwrap(),
            nested.canonicalize().unwrap()
        );
        assert_eq!(opened["status"]["branch"], "main");

        // Later calls address the nested root; a lane re-detect passes it back.
        assert!(git
            .execute_root(path, &trust, "ws", Some(&root), GitCommand::Branches)
            .is_ok());
        let redetected = git
            .execute_root(path, &trust, "ws", Some(&root), GitCommand::Detect)
            .unwrap();
        assert_eq!(redetected["root"], root.as_str());
        assert!(git
            .execute_root(path, &trust, "ws", Some("../outside"), GitCommand::Detect)
            .is_err());
    }

    #[test]
    fn untrusted_repository_still_refuses_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = WorkspaceTrustState::at(tmp.path().join("trust.json"));
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("nested")).unwrap();
        test_repo::init(&repo);
        let mut git = HostGit::default();
        for path in [repo.clone(), repo.join("nested")] {
            for call in [GitCommand::Detect, GitCommand::Bootstrap] {
                assert!(git
                    .execute_root(path.to_str().unwrap(), &trust, "ws", None, call)
                    .unwrap_err()
                    .contains("untrustedWorkspace"));
            }
        }
    }

    // Windows reports `C:/…` roots against a `\\?\C:\…` identity; an alias of
    // the open repository is the same root, any other repository is not.
    #[cfg(unix)]
    #[test]
    fn open_repository_accepts_another_spelling_of_its_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = WorkspaceTrustState::at(tmp.path().join("trust.json"));
        let workspace = tmp.path().join("ws");
        let other = tmp.path().join("other");
        for repo in [&workspace, &other] {
            std::fs::create_dir_all(repo).unwrap();
            test_repo::init(repo);
            test_repo::write_and_commit(repo, "a.txt", "1", "c1");
        }
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&workspace, &alias).unwrap();
        let path = workspace.to_str().unwrap();
        trust.0.grant_for_tests(path);
        let mut git = HostGit::default();
        git.execute_root(path, &trust, "ws", None, GitCommand::Detect)
            .unwrap();

        assert!(git
            .execute_root(path, &trust, "ws", alias.to_str(), GitCommand::Branches)
            .is_ok());
        assert_eq!(
            git.execute_root(path, &trust, "ws", other.to_str(), GitCommand::Branches)
                .unwrap_err(),
            "git-repository-identity-mismatch"
        );
        assert!(git
            .execute_root(path, &trust, "ws", None, GitCommand::Branches)
            .is_err());
    }
}
