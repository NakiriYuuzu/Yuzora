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
    Commit { message: String },
    #[serde(rename = "git_branches")]
    Branches,
    #[serde(rename = "git_create_branch")]
    CreateBranch {
        name: String,
        start_point: Option<String>,
    },
    #[serde(rename = "git_checkout_detached")]
    CheckoutDetached { rev: String },
    #[serde(rename = "git_checkout")]
    Checkout { name: String },
    #[serde(rename = "git_cherry_pick")]
    CherryPick { hash: String },
    #[serde(rename = "git_fetch_cmd")]
    Fetch,
    #[serde(rename = "git_pull_cmd")]
    Pull,
    #[serde(rename = "git_push_cmd")]
    Push,
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
                | Self::Status { .. }
                | Self::Branches
                | Self::RemoteProbe
                | Self::Diff { .. }
                | Self::Log { .. }
                | Self::CommitDetail { .. }
                | Self::LogAuthors
                | Self::FileAtRev { .. }
        )
    }
}

#[cfg(unix)]
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
            let identity = trust.require_trusted(path)?;
            if matches!(call, GitCommand::Detect | GitCommand::Bootstrap) {
                self.roots.remove(workspace);
                let environment = detect_environment(Path::new(path));
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
            if requested_root != Some(expected.canonical_path.as_str()) {
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
                GitCommand::Commit { message } => value(commit(root, &message)),
                GitCommand::Branches => value(branches(root)),
                GitCommand::CreateBranch { name, start_point } => {
                    value(create_branch(root, &name, start_point.as_deref()))
                }
                GitCommand::CheckoutDetached { rev } => value(checkout_detached(root, &rev)),
                GitCommand::Checkout { name } => value(checkout(root, &name)),
                GitCommand::CherryPick { hash } => value(cherry_pick(root, &hash)),
                GitCommand::Fetch => {
                    value(run_ok(root, &["fetch"], REMOTE_TIMEOUT, &[]).map(|_| ()))
                }
                GitCommand::Pull => {
                    value(run_ok(root, &["pull"], REMOTE_TIMEOUT, &editor_true()).map(|_| ()))
                }
                GitCommand::Push => value(run_ok(root, &["push"], REMOTE_TIMEOUT, &[]).map(|_| ())),
                GitCommand::RemoteProbe => value(remote_probe(root, &[])),
                GitCommand::Diff {
                    path,
                    staged,
                    orig_path,
                } => value(diff_content(root, &path, staged, orig_path.as_deref())),
                GitCommand::ConflictAbort { op } => value(conflict_abort(root, &op)),
                GitCommand::ConflictContinue { op } => value(conflict_continue(root, &op)),
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
                GitCommand::Detect | GitCommand::Bootstrap => unreachable!(),
            }
        }
    }
}
#[cfg(unix)]
pub use host::HostGit;
