//! The little Git the shell shows: a folder's branch and how far it is ahead
//! of and behind its upstream (the status bar's "⎇ feat/dock ↑2"), here or on
//! a machine (`hc exec --on`). Everything else Git is lane L2's (`yuzora:git`).
//!
//! The status bar asks on its own as soon as a terminal `cd`s into a folder,
//! so it only reads refs, config and commits. `git status` would look at the
//! working tree and run what the repo's own config names there (a clean
//! filter, an fsmonitor hook); a partial clone would fetch a missing commit
//! with the repo's ssh command, so there the walk runs with `--no-lazy-fetch`
//! (git 2.44 and later) or not at all. Changed and conflicted files need
//! `git status`: they stay null until the folder is trusted (L2's check).

use serde_json::{Value, json};
use std::time::Duration;
use yuzora_core::remote::{ExecError, ExecOutput, ExecRequest};

/// A status bar look shouldn't wait on a slow disk or machine for long (each step).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Before every step: no optional locks and no fsmonitor, so a look writes
/// nothing under `.git` (that would wake every folder watch on the repo).
const BASE: [&str; 3] = ["--no-optional-locks", "-c", "core.fsmonitor=false"];

/// Config keys that make a remote a promisor one: git fetches what's missing from it.
const PROMISOR: &str = r"^(extensions\.partialclone|remote\..+\.(promisor|partialclonefilter))$";

/// The upstream and how far apart: `name\0ahead 2, behind 1` (for-each-ref's
/// plumbing words, never translated).
const UPSTREAM: &str = "--format=%(upstream:short)%00%(upstream:track,nobracket)";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// `None` when detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    /// Files with staged or unstaged changes, and conflicted ones: `None` until
    /// the folder is trusted (they need `git status`).
    pub changed: Option<u64>,
    pub conflicted: Option<u64>,
}

impl Summary {
    pub fn to_json(&self) -> Value {
        json!({
            "branch": self.branch, "upstream": self.upstream, "ahead": self.ahead, "behind": self.behind,
            "changed": self.changed, "conflicted": self.conflicted,
        })
    }
}

/// The steps' arguments after `git`.
pub fn args<'a>(step: &[&'a str]) -> Vec<&'a str> {
    BASE.iter().copied().chain(step.iter().copied()).collect()
}

/// Reads `for-each-ref` with [`UPSTREAM`]: the upstream (`None` without one)
/// and ahead/behind (nothing when it's gone or even).
pub fn parse_upstream(out: &str) -> (Option<String>, u64, u64) {
    let line = out.lines().next().unwrap_or_default();
    let (name, track) = line.split_once('\0').unwrap_or((line, ""));
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    ((!name.is_empty()).then(|| name.to_owned()), ahead, behind)
}

/// `cwd`'s summary (on `machine`, else here); `None` when it isn't in a repo.
/// Blocking: call from a worker thread.
pub fn status(cwd: &str, machine: Option<String>) -> Result<Option<Summary>, ExecError> {
    status_with(cwd, machine, &yuzora_core::remote::run)
}

/// [`status`] with the way a request is run (tests count the calls).
fn status_with(cwd: &str, machine: Option<String>, run: &(dyn Fn(&ExecRequest) -> Result<ExecOutput, ExecError> + Sync)) -> Result<Option<Summary>, ExecError> {
    let git = |step: &[&str]| -> Result<ExecOutput, ExecError> { run(&ExecRequest::new("git", args(step)).cwd(Some(cwd)).on(machine.clone()).timeout(TIMEOUT).max_output(1 << 16)) };
    // yuzora(perf): on a machine every step is a round trip through hc: the
    // three steps run there as one script. A machine without `sh` gets the steps one by one.
    if machine.is_some() {
        let req = ExecRequest::new("sh", ["-c", REMOTE_SCRIPT, "sh", PROMISOR, UPSTREAM]).cwd(Some(cwd)).on(machine.clone()).timeout(TIMEOUT * 2).max_output(1 << 17);
        match run(&req) {
            Ok(out) => {
                if let Some(answer) = parse_script(&out) {
                    return Ok(answer);
                }
            }
            Err(e @ ExecError::TimedOut { .. }) => return Err(e),
            // No `sh` there, or the machine isn't answering: the steps tell which.
            Err(_) => {}
        }
    }
    // The first two steps don't depend on each other (a folder outside a repo
    // answers the second one from the global config only, and it is ignored there).
    let (head, promisor) = std::thread::scope(|s| {
        let promisor = s.spawn(|| git(&["config", "--get-regexp", PROMISOR]));
        let head = git(&["symbolic-ref", "-q", "HEAD"]);
        (head, promisor.join().unwrap_or_else(|_| Err(ExecError::Spawn("the config look panicked".into()))))
    });
    let head = head?;
    let mut s = Summary::default();
    let full = match head.code {
        0 => head.stdout_text().trim_end().to_owned(),
        1 => return Ok(Some(s)), // detached
        _ => return Ok(None),
    };
    s.branch = Some(full.strip_prefix("refs/heads/").unwrap_or(&full).to_owned());
    // 1 is "no such key"; anything else might be one.
    let promisor = promisor?.code != 1;
    let upstream = if promisor { git(&["--no-lazy-fetch", "for-each-ref", UPSTREAM, full.as_str()])? } else { git(&["for-each-ref", UPSTREAM, full.as_str()])? };
    if upstream.code == 0 {
        (s.upstream, s.ahead, s.behind) = parse_upstream(&upstream.stdout_text());
    }
    Ok(Some(s))
}

/// The steps of [`status`] on a machine, in order and with the same options:
/// `$1` is [`PROMISOR`], `$2` [`UPSTREAM`]. It stops where [`status`] would
/// (not a repo, detached), and walks to the upstream with `--no-lazy-fetch`
/// when a remote is a promisor one. Ref names hold no line feed.
const REMOTE_SCRIPT: &str = r#"b="--no-optional-locks -c core.fsmonitor=false"
h=$(git $b symbolic-ref -q HEAD); rc=$?
printf 'rc=%s\n' "$rc"
[ "$rc" = 0 ] || exit 0
printf 'head=%s\n' "$h"
git $b config --get-regexp "$1" >/dev/null; p=$?
printf 'promisor=%s\n' "$p"
if [ "$p" = 1 ]; then git $b for-each-ref "$2" "$h"; else git $b --no-lazy-fetch for-each-ref "$2" "$h"; fi
printf '\nfe=%s\n' "$?"
"#;

/// What [`REMOTE_SCRIPT`] said; `None` when it didn't run (no `sh` there): the caller asks step by step.
fn parse_script(out: &ExecOutput) -> Option<Option<Summary>> {
    let text = out.stdout_text();
    let (rc, rest) = text.strip_prefix("rc=")?.split_once('\n')?;
    let mut s = Summary::default();
    match rc {
        "0" => {}
        "1" => return Some(Some(s)), // detached
        _ => return Some(None),
    }
    let (head, rest) = rest.strip_prefix("head=")?.split_once('\n')?;
    s.branch = Some(head.strip_prefix("refs/heads/").unwrap_or(head).to_owned());
    let (_promisor, rest) = rest.strip_prefix("promisor=")?.split_once('\n')?;
    let (body, fe) = rest.rsplit_once("\nfe=")?;
    if fe.trim_end() == "0" {
        (s.upstream, s.ahead, s.behind) = parse_upstream(body);
    }
    Some(Some(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn reads_upstream_and_how_far_apart() {
        assert_eq!(parse_upstream("origin/feat/gpui-dock\0ahead 2, behind 1\n"), (Some("origin/feat/gpui-dock".into()), 2, 1));
        assert_eq!(parse_upstream("origin/main\0behind 3\n"), (Some("origin/main".into()), 0, 3));
        assert_eq!(parse_upstream("origin/main\0gone\n"), (Some("origin/main".into()), 0, 0));
        assert_eq!(parse_upstream("origin/main\0\n"), (Some("origin/main".into()), 0, 0));
        assert_eq!(parse_upstream("\0\n"), (None, 0, 0));
        assert_eq!(parse_upstream(""), (None, 0, 0));
        let s = Summary { ahead: 2, ..Summary::default() };
        assert_eq!(s.to_json()["ahead"], 2);
        assert_eq!(s.to_json()["changed"], Value::Null);
    }

    #[test]
    fn every_step_writes_nothing_under_git() {
        for step in [&["symbolic-ref", "-q", "HEAD"][..], &["for-each-ref", UPSTREAM][..]] {
            let a = args(step);
            assert!(a.contains(&"--no-optional-locks") && a.contains(&"core.fsmonitor=false"));
            assert!(!a.contains(&"status"));
        }
    }

    /// A folder under the temp dir, gone when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yuzora-shell-git-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }

        /// Git as the test sets it up: none of this computer's own config.
        fn git(&self, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(&self.0)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
        }

        fn write(&self, file: &str, text: &str) {
            std::fs::write(self.0.join(file), text).unwrap();
        }

        /// A script that leaves `marker` behind when it runs.
        #[cfg(unix)]
        fn program(&self, name: &str, marker: &Path, then: &str) -> String {
            use std::os::unix::fs::PermissionsExt;
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\ntouch '{}'\n{then}\n", marker.display())).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git_here() -> bool {
        std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn asks_a_real_repo_and_says_none_outside_one() {
        if !git_here() {
            return;
        }
        let dir = Scratch::new("plain");
        assert_eq!(status(&dir.path(), None).unwrap(), None);
        dir.git(&["init", "-q", "-b", "trunk"]);
        let s = status(&dir.path(), None).unwrap().unwrap();
        assert_eq!(s.branch.as_deref(), Some("trunk"));
        assert_eq!((s.upstream, s.ahead, s.behind), (None, 0, 0));
        dir.write("a.txt", "1\n");
        dir.git(&["add", "a.txt"]);
        dir.git(&["commit", "-qm", "one"]);
        dir.git(&["checkout", "-q", "--detach"]);
        assert_eq!(status(&dir.path(), None).unwrap(), Some(Summary::default()));
    }

    #[cfg(unix)]
    #[test]
    fn a_repos_own_filter_never_runs() {
        if !git_here() {
            return;
        }
        let dir = Scratch::new("filter");
        dir.git(&["init", "-q", "-b", "feat/dock"]);
        dir.write("a.txt", "1\n");
        dir.git(&["add", "a.txt"]);
        dir.git(&["commit", "-qm", "one"]);
        dir.git(&["branch", "base"]);
        dir.write("a.txt", "2\n");
        dir.git(&["commit", "-qam", "two"]);
        // base gets a commit of its own: one ahead, one behind.
        let side = dir.git(&["commit-tree", "-p", "base", "-m", "side", "base^{tree}"]);
        dir.git(&["update-ref", "refs/heads/base", &side]);
        dir.git(&["branch", "-q", "-u", "base"]);
        // A clean filter in the repo's own config, on a file that changed since.
        let marker = dir.0.join("ran");
        let clean = dir.program("clean.sh", &marker, "cat");
        dir.git(&["config", "filter.evil.clean", &clean]);
        std::fs::write(dir.0.join(".git/info/attributes"), "* filter=evil\n").unwrap();
        dir.write("a.txt", "3\n");
        let s = status(&dir.path(), None).unwrap().unwrap();
        assert_eq!(s.branch.as_deref(), Some("feat/dock"));
        assert_eq!((s.upstream.as_deref(), s.ahead, s.behind), (Some("base"), 1, 1));
        assert_eq!((s.changed, s.conflicted), (None, None));
        assert!(!marker.exists(), "the status bar's look ran the repo's clean filter");
        // What it used to ask does run it: the marker would have caught it.
        dir.git(&["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch", "--untracked-files=no"]);
        assert!(marker.exists(), "git status didn't run the filter, so this test proves nothing");
    }

    #[cfg(unix)]
    #[test]
    fn a_partial_clones_missing_commit_is_not_fetched() {
        if !git_here() {
            return;
        }
        let dir = Scratch::new("promisor");
        dir.git(&["init", "-q", "-b", "trunk"]);
        dir.write("a.txt", "1\n");
        dir.git(&["add", "a.txt"]);
        dir.git(&["commit", "-qm", "one"]);
        // The upstream points at a commit the clone doesn't have; its remote
        // is a promisor whose ssh command is the repo's own.
        let marker = dir.0.join("ran");
        let ssh = dir.program("ssh.sh", &marker, "exit 1");
        for (key, value) in [
            ("remote.origin.url", "ssh://example.invalid/x"),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("remote.origin.promisor", "true"),
            ("core.sshCommand", ssh.as_str()),
            ("branch.trunk.remote", "origin"),
            ("branch.trunk.merge", "refs/heads/main"),
        ] {
            dir.git(&["config", key, value]);
        }
        let gone = dir.git(&["commit-tree", "-p", "HEAD", "-m", "gone", "HEAD^{tree}"]);
        dir.git(&["update-ref", "refs/remotes/origin/main", &gone]);
        std::fs::remove_file(dir.0.join(format!(".git/objects/{}/{}", &gone[..2], &gone[2..]))).unwrap();
        let s = status(&dir.path(), None).unwrap().unwrap();
        assert_eq!(s.branch.as_deref(), Some("trunk"));
        assert_eq!((s.ahead, s.behind), (0, 0));
        assert!(!marker.exists(), "the status bar's look fetched with the repo's ssh command");
        // Walking to it the plain way does fetch: the marker would have caught it.
        dir.git(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]);
        assert!(marker.exists(), "rev-list didn't fetch, so this test proves nothing");
    }

    /// The machine "m" is this computer, the way the tests ask: every request
    /// it gets is counted and run here.
    fn on_this_computer(calls: &std::sync::Mutex<Vec<String>>) -> impl Fn(&ExecRequest) -> Result<ExecOutput, ExecError> + '_ {
        move |req| {
            calls.lock().unwrap().push(format!("{} {}", req.program, req.args.first().map(String::as_str).unwrap_or_default()));
            yuzora_core::remote::run(&ExecRequest { on: None, ..req.clone() })
        }
    }

    fn repo_with_upstream(name: &str) -> Scratch {
        let dir = Scratch::new(name);
        dir.git(&["init", "-q", "-b", "feat/dock"]);
        dir.write("a.txt", "1\n");
        dir.git(&["add", "a.txt"]);
        dir.git(&["commit", "-qm", "one"]);
        dir.git(&["branch", "base"]);
        dir.write("a.txt", "2\n");
        dir.git(&["commit", "-qam", "two"]);
        let side = dir.git(&["commit-tree", "-p", "base", "-m", "side", "base^{tree}"]);
        dir.git(&["update-ref", "refs/heads/base", &side]);
        dir.git(&["branch", "-q", "-u", "base"]);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_machine_answers_in_one_round_trip_with_the_same_summary() {
        if !git_here() {
            return;
        }
        let m = Some("m".to_owned());
        // Outside a repo, in a repo with an upstream, unborn, and detached: the same answers as here.
        let plain = Scratch::new("m-plain");
        let repo = repo_with_upstream("m-up");
        let detached = repo_with_upstream("m-detached");
        detached.git(&["checkout", "-q", "--detach"]);
        let unborn = Scratch::new("m-unborn");
        unborn.git(&["init", "-q", "-b", "fresh"]);
        for dir in [&plain, &repo, &detached, &unborn] {
            let calls = std::sync::Mutex::new(Vec::new());
            let there = status_with(&dir.path(), m.clone(), &on_this_computer(&calls)).unwrap();
            assert_eq!(calls.lock().unwrap().len(), 1, "{:?}", calls.lock().unwrap());
            assert_eq!(there, status(&dir.path(), None).unwrap(), "{}", dir.path());
        }
        let s = status_with(&repo.path(), m.clone(), &on_this_computer(&Default::default())).unwrap().unwrap();
        assert_eq!((s.branch.as_deref(), s.upstream.as_deref(), s.ahead, s.behind), (Some("feat/dock"), Some("base"), 1, 1));
        assert_eq!(status_with(&unborn.path(), m, &on_this_computer(&Default::default())).unwrap().unwrap().branch.as_deref(), Some("fresh"));
    }

    #[test]
    fn a_machine_without_sh_is_asked_step_by_step() {
        if !git_here() {
            return;
        }
        let repo = repo_with_upstream("m-nosh");
        let calls = std::sync::Mutex::new(Vec::new());
        let run = |req: &ExecRequest| {
            calls.lock().unwrap().push(req.program.clone());
            if req.program == "sh" {
                return Err(ExecError::Remote("sh: not found".into()));
            }
            yuzora_core::remote::run(&ExecRequest { on: None, ..req.clone() })
        };
        let s = status_with(&repo.path(), Some("m".into()), &run).unwrap().unwrap();
        assert_eq!((s.branch.as_deref(), s.upstream.as_deref(), s.ahead, s.behind), (Some("feat/dock"), Some("base"), 1, 1));
        assert_eq!(calls.lock().unwrap().iter().filter(|p| *p == "git").count(), 3);
        // A timeout is not "no sh": it is the answer.
        let slow = |_: &ExecRequest| Err(ExecError::TimedOut { seconds: 1.0, machine: Some("m".into()) });
        assert!(matches!(status_with(&repo.path(), Some("m".into()), &slow), Err(ExecError::TimedOut { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn a_machines_partial_clone_is_not_fetched_either() {
        if !git_here() {
            return;
        }
        let dir = Scratch::new("m-promisor");
        dir.git(&["init", "-q", "-b", "trunk"]);
        dir.write("a.txt", "1\n");
        dir.git(&["add", "a.txt"]);
        dir.git(&["commit", "-qm", "one"]);
        let marker = dir.0.join("ran");
        let ssh = dir.program("ssh.sh", &marker, "exit 1");
        for (key, value) in [
            ("remote.origin.url", "ssh://example.invalid/x"),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("remote.origin.promisor", "true"),
            ("core.sshCommand", ssh.as_str()),
            ("branch.trunk.remote", "origin"),
            ("branch.trunk.merge", "refs/heads/main"),
        ] {
            dir.git(&["config", key, value]);
        }
        let gone = dir.git(&["commit-tree", "-p", "HEAD", "-m", "gone", "HEAD^{tree}"]);
        dir.git(&["update-ref", "refs/remotes/origin/main", &gone]);
        std::fs::remove_file(dir.0.join(format!(".git/objects/{}/{}", &gone[..2], &gone[2..]))).unwrap();
        let s = status_with(&dir.path(), Some("m".into()), &on_this_computer(&Default::default())).unwrap().unwrap();
        assert_eq!(s.branch.as_deref(), Some("trunk"));
        assert_eq!((s.ahead, s.behind), (0, 0));
        assert!(!marker.exists(), "the script fetched with the repo's ssh command");
    }

    #[cfg(unix)]
    #[test]
    fn a_machines_repo_filter_never_runs() {
        if !git_here() {
            return;
        }
        let dir = repo_with_upstream("m-filter");
        let marker = dir.0.join("ran");
        let clean = dir.program("clean.sh", &marker, "cat");
        dir.git(&["config", "filter.evil.clean", &clean]);
        std::fs::write(dir.0.join(".git/info/attributes"), "* filter=evil\n").unwrap();
        dir.write("a.txt", "3\n");
        status_with(&dir.path(), Some("m".into()), &on_this_computer(&Default::default())).unwrap().unwrap();
        assert!(!marker.exists());
    }

    /// Round trips through a real `hc` machine (set `YUZORA_PERF_MACHINE`):
    /// `cargo test -p yuzora-shell --lib -- --ignored --nocapture machine_round_trips`.
    #[test]
    #[ignore]
    fn machine_round_trips() {
        let (Ok(m), Ok(dir)) = (std::env::var("YUZORA_PERF_MACHINE"), std::env::var("YUZORA_PERF_REPO")) else { return };
        let med = |f: &dyn Fn()| {
            f();
            let mut v: Vec<_> = (0..20).map(|_| { let t = std::time::Instant::now(); f(); t.elapsed() }).collect();
            v.sort();
            v[10]
        };
        let step = |a: &[&str]| { yuzora_core::remote::run(&ExecRequest::new("git", args(a)).cwd(Some(dir.as_str())).on(Some(m.clone())).timeout(TIMEOUT)).unwrap(); };
        let before = med(&|| { step(&["symbolic-ref", "-q", "HEAD"]); step(&["config", "--get-regexp", PROMISOR]); step(&["for-each-ref", UPSTREAM, "refs/heads/main"]); });
        let nosh = |r: &ExecRequest| if r.program == "sh" { Err(ExecError::Remote("no sh".into())) } else { yuzora_core::remote::run(r) };
        let fallback = med(&|| { status_with(&dir, Some(m.clone()), &nosh).unwrap(); });
        let after = med(&|| { status(&dir, Some(m.clone())).unwrap(); });
        eprintln!("PERF status bar on a machine: before (3 sequential) {before:?}, fallback (2 rounds) {fallback:?}, after (1 script) {after:?}");
    }
}
