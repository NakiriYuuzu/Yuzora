//! The little Git the shell shows: a folder's branch, how far it is ahead of
//! and behind its upstream, and how many files changed (the status bar's
//! "⎇ feat/dock ↑2"). One `git status --porcelain=v2 --branch` here or on a
//! machine (`hc exec --on`). Everything else Git is lane L2's (`yuzora:git`).

use serde_json::{Value, json};
use std::time::Duration;
use yuzora_core::remote::{ExecError, ExecRequest};

/// A status bar look shouldn't wait on a slow disk or machine for long.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// `None` when detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    /// Tracked files with staged or unstaged changes (untracked ones aren't asked for).
    pub changed: u64,
    pub conflicted: u64,
}

impl Summary {
    pub fn to_json(&self) -> Value {
        json!({
            "branch": self.branch, "upstream": self.upstream, "ahead": self.ahead, "behind": self.behind,
            "changed": self.changed, "conflicted": self.conflicted,
        })
    }
}

/// The arguments: no optional locks and no fsmonitor, so a look writes nothing
/// under `.git` (that would wake every folder watch on the repo).
pub fn args() -> Vec<&'static str> {
    vec!["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch", "--untracked-files=no"]
}

/// Reads `git status --porcelain=v2 --branch`.
pub fn parse(out: &str) -> Summary {
    let mut s = Summary::default();
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            s.branch = (head != "(detached)").then(|| head.to_owned());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(up.to_owned());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace().map(|n| n.trim_start_matches(['+', '-']).parse::<u64>().unwrap_or(0));
            s.ahead = it.next().unwrap_or(0);
            s.behind = it.next().unwrap_or(0);
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            s.changed += 1;
        } else if line.starts_with("u ") {
            s.conflicted += 1;
        }
    }
    s
}

/// `cwd`'s summary (on `machine`, else here); `None` when it isn't in a repo.
/// Blocking: call from a worker thread.
pub fn status(cwd: &str, machine: Option<String>) -> Result<Option<Summary>, ExecError> {
    let req = ExecRequest::new("git", args()).cwd(Some(cwd)).on(machine).timeout(TIMEOUT).max_output(1 << 20);
    let out = yuzora_core::remote::run(&req)?;
    Ok((out.code == 0).then(|| parse(&out.stdout_text())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_branch_upstream_and_counts() {
        let out = "# branch.oid 1234\n# branch.head feat/gpui-dock\n# branch.upstream origin/feat/gpui-dock\n# branch.ab +2 -1\n\
                   1 .M N... 100644 100644 100644 a b src/a.rs\n1 A. N... 0 100644 100644 0 b src/b.rs\n\
                   2 R. N... 100644 100644 100644 a b R100 new\told\nu UU N... 1 2 3 4 a b c d conflict.rs\n";
        let s = parse(out);
        assert_eq!(s.branch.as_deref(), Some("feat/gpui-dock"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feat/gpui-dock"));
        assert_eq!((s.ahead, s.behind, s.changed, s.conflicted), (2, 1, 3, 1));
        assert_eq!(s.to_json()["ahead"], 2);
    }

    #[test]
    fn detached_and_unpublished_branches() {
        let s = parse("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(s, Summary::default());
        let s = parse("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream, None);
        assert_eq!(s.ahead, 0);
    }

    #[test]
    fn a_status_writes_nothing_under_git() {
        let a = args();
        assert!(a.contains(&"--no-optional-locks") && a.contains(&"core.fsmonitor=false"));
    }

    #[test]
    fn asks_a_real_repo_and_says_none_outside_one() {
        let dir = std::env::temp_dir().join(format!("yuzora-shell-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        // No git on this machine: nothing to check.
        let Ok(none) = status(&path, None) else { return };
        assert_eq!(none, None);
        let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&dir).output().unwrap();
        git(&["init", "-q", "-b", "trunk"]);
        let s = status(&path, None).unwrap().unwrap();
        assert_eq!(s.branch.as_deref(), Some("trunk"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
