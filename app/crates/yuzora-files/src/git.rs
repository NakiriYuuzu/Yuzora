//! Git status for the Files lane: the branch and a mark per changed path, read
//! with `git status --porcelain` here or on a herdr-core machine. Blocking.
//! Writes nothing under `.git` (no optional locks, so no index refresh).
//! In a folder not trusted yet whose own config names programs (a clean
//! filter runs during status), only the refs are read: no marks.

use std::path::Path;
use std::time::Duration;

use yuzora_core::git_trust;
use yuzora_core::remote::{self, ExecError, ExecOutput, ExecRequest};

use crate::fsops::join;

const TIMEOUT: Duration = Duration::from_secs(15);
/// Config keys that make a remote a promisor one: git fetches what's missing from it.
const PROMISOR: &str = r"^(extensions\.partialclone|remote\..+\.(promisor|partialclonefilter))$";
const MAX_OUTPUT: u64 = 16 << 20;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GitStatus {
    /// The work tree's root (absolute).
    pub top: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    /// (absolute path, mark), mark in `M A D U ?`.
    pub marks: Vec<(String, char)>,
    /// More changed paths than `limit` (or than the output could hold).
    pub truncated: bool,
    /// The folder waits for trust: no `git status` ran, so no marks.
    pub untrusted: bool,
}

impl GitStatus {
    fn new(top: &str) -> Self {
        GitStatus {
            top: top.to_owned(),
            branch: None,
            detached: false,
            ahead: 0,
            behind: 0,
            marks: Vec::new(),
            truncated: false,
            untrusted: false,
        }
    }
}

/// git in `cwd` (on `on`): never an fsmonitor command (it would run per
/// call) nor an index lock.
fn runner<'a>(
    cwd: &'a str,
    on: Option<&'a str>,
) -> impl Fn(&[&str]) -> Result<ExecOutput, ExecError> + 'a {
    move |args| {
        let req = ExecRequest::new("git", git_trust::READ_FLAGS.iter().chain(args).copied())
            .on(on)
            .cwd(Some(cwd))
            .timeout(TIMEOUT)
            .max_output(MAX_OUTPUT);
        remote::run(&req)
    }
}

/// The status of the work tree around `cwd`; `None` when `cwd` is not inside
/// one (or git is not there).
pub fn status(cwd: &str, on: Option<&str>, limit: usize) -> Result<Option<GitStatus>, String> {
    status_with(&git_trust::trusted_file(), cwd, on, limit)
}

/// [`status`] with the trusted folders listed in `trusted`.
fn status_with(
    trusted: &Path,
    cwd: &str,
    on: Option<&str>,
    limit: usize,
) -> Result<Option<GitStatus>, String> {
    let git = runner(cwd, on);
    let Some((dir, top)) = toplevel(&git)? else {
        return Ok(None);
    };
    if waits(trusted, on, &dir, &top) {
        return refs_only(&git, &top).map(Some);
    }
    let out = match git(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--branch",
        "--no-renames",
        "--untracked-files=all",
    ]) {
        Ok(out) if out.code == 0 => out,
        Ok(out) => {
            return Err(match out.stderr_text().trim() {
                "" => format!("git status failed (exit {})", out.code),
                e => e.to_owned(),
            });
        }
        Err(ExecError::Spawn(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    // At the output cap the last record may be cut: leave it out.
    let capped = out.stdout.len() as u64 >= MAX_OUTPUT;
    let mut bytes = &out.stdout[..];
    if capped && let Some(nul) = bytes.iter().rposition(|b| *b == 0) {
        bytes = &bytes[..=nul];
    }
    let mut status = parse_porcelain(&top, bytes, limit);
    status.truncated |= capped;
    Ok(Some(status))
}

/// The repository git finds where `git` runs: its git dir and the top level
/// of its work tree; `None` outside one (or without git).
fn toplevel(
    git: impl Fn(&[&str]) -> Result<ExecOutput, ExecError>,
) -> Result<Option<(String, String)>, String> {
    let out = match git(&["rev-parse", "--absolute-git-dir", "--show-toplevel"]) {
        Ok(out) if out.code == 0 => out.stdout_text(),
        Ok(_) | Err(ExecError::Spawn(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    Ok(out
        .trim_end_matches(['\n', '\r'])
        .split_once('\n')
        .map(|(dir, top)| (dir.trim_end_matches('\r').to_owned(), top.to_owned()))
        .filter(|(dir, top)| !dir.is_empty() && !top.is_empty()))
}

/// Only the refs may be read in the repository at `dir`, work tree `top`.
/// The trust check reads the repository git finds from `top`, which a
/// `core.worktree` can make another one: then it speaks for nothing here.
/// Otherwise, while its config names programs git would run on the work
/// tree (a clean filter during `status`), or that can't be told, and the
/// person hasn't trusted the folder.
fn waits(trusted: &Path, on: Option<&str>, dir: &str, top: &str) -> bool {
    let there = runner(top, on)(&["rev-parse", "--absolute-git-dir"]);
    let same = matches!(&there, Ok(out) if out.code == 0 && out.stdout_text().trim_end_matches(['\n', '\r']) == dir);
    !same || git_trust::pending_from(trusted, on, top).map_or(true, |keys| !keys.is_empty())
}

/// Whether git may read the work tree around `cwd` (list or compare it):
/// it is one, and it doesn't wait for trust.
pub(crate) fn work_tree_open(trusted: &Path, cwd: &str, on: Option<&str>) -> bool {
    matches!(toplevel(runner(cwd, on)), Ok(Some((dir, top))) if !waits(trusted, on, &dir, &top))
}

/// The branch and how far it is from its upstream, from the refs alone: no
/// `git status`, so nothing of the work tree is read and no filter or other
/// program of the repo's runs. For a folder that waits for trust.
fn refs_only(
    git: impl Fn(&[&str]) -> Result<ExecOutput, ExecError>,
    top: &str,
) -> Result<GitStatus, String> {
    let mut status = GitStatus::new(top);
    status.untrusted = true;
    match git(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        Ok(out) if out.code == 0 => {
            status.branch = Some(out.stdout_text().trim_end_matches(['\n', '\r']).to_owned());
        }
        // 1: HEAD is not a branch.
        Ok(out) if out.code == 1 => status.detached = true,
        Ok(out) => {
            return Err(match out.stderr_text().trim() {
                "" => format!("git symbolic-ref failed (exit {})", out.code),
                e => e.to_owned(),
            });
        }
        Err(e) => return Err(e.to_string()),
    }
    // A partial clone would fetch a commit it lacks with the repo's own ssh
    // command: there the walk never fetches (git 2.44 and later; older git
    // shows no counts). 1 is "no such key"; anything else might be one.
    let promisor = git(&["config", "--get-regexp", PROMISOR]).map_or(true, |out| out.code != 1);
    let walk = ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"];
    let no_fetch = ["--no-lazy-fetch"]
        .iter()
        .chain(&walk)
        .copied()
        .collect::<Vec<_>>();
    // Fails without an upstream (or a commit): 0 and 0, as status shows it.
    if !status.detached
        && let Ok(out) = git(if promisor { &no_fetch } else { &walk })
        && out.code == 0
    {
        let text = out.stdout_text();
        let mut counts = text.split_whitespace().map(|n| n.parse().unwrap_or(0));
        status.ahead = counts.next().unwrap_or(0);
        status.behind = counts.next().unwrap_or(0);
    }
    Ok(status)
}

/// Reads `git status --porcelain=v1 -z --branch` output. A path that is not
/// UTF-8 gets no mark: a stand-in for it (with U+FFFD) could name another file.
pub fn parse_porcelain(top: &str, out: impl AsRef<[u8]>, limit: usize) -> GitStatus {
    let mut status = GitStatus::new(top);
    let mut records = out.as_ref().split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix(b"## ") {
            parse_branch(&String::from_utf8_lossy(header), &mut status);
            continue;
        }
        let [x, y, b' ', path @ ..] = record else {
            continue;
        };
        let (x, y) = (char::from(*x), char::from(*y));
        // A rename or copy is followed by its source path.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            records.next();
        }
        let Some(mark) = mark_of(x, y) else { continue };
        let Ok(path) = std::str::from_utf8(path) else {
            continue;
        };
        if status.marks.len() >= limit {
            status.truncated = true;
            break;
        }
        status.marks.push((join(top, path), mark));
    }
    status
}

/// The one-letter mark of an `XY` pair; `None` for ignored files.
fn mark_of(x: char, y: char) -> Option<char> {
    Some(match (x, y) {
        ('!', '!') => return None,
        ('?', '?') => '?',
        ('D', 'D')
        | ('A', 'U')
        | ('U', 'D')
        | ('U', 'A')
        | ('D', 'U')
        | ('A', 'A')
        | ('U', 'U') => 'U',
        _ if x == 'D' || y == 'D' => 'D',
        ('A', _) => 'A',
        _ => 'M',
    })
}

/// `main...origin/main [ahead 1, behind 2]`, `HEAD (no branch)`,
/// `No commits yet on main`.
fn parse_branch(header: &str, status: &mut GitStatus) {
    if header.starts_with("HEAD (no branch)") {
        status.detached = true;
        return;
    }
    if let Some(name) = header
        .strip_prefix("No commits yet on ")
        .or_else(|| header.strip_prefix("Initial commit on "))
    {
        status.branch = Some(name.to_owned());
        return;
    }
    let (names, counts) = match header.split_once(" [") {
        Some((names, counts)) => (names, counts.trim_end_matches(']')),
        None => (header, ""),
    };
    let branch = names
        .split_once("...")
        .map_or(names, |(branch, _upstream)| branch);
    status.branch = Some(branch.to_owned());
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            status.ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            status.behind = n.trim().parse().unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn nul(records: &[&str]) -> String {
        records.iter().map(|r| format!("{r}\0")).collect()
    }

    #[test]
    fn branch_headers() {
        let head = |h: &str| parse_porcelain("/r", nul(&[h]), 10);
        let s = head("## main...origin/main [ahead 3, behind 2]");
        assert_eq!(
            (s.branch.as_deref(), s.detached, s.ahead, s.behind),
            (Some("main"), false, 3, 2)
        );
        let s = head("## feat/x...origin/feat/x [behind 5]");
        assert_eq!(
            (s.branch.as_deref(), s.ahead, s.behind),
            (Some("feat/x"), 0, 5)
        );
        let s = head("## main...origin/main [ahead 1]");
        assert_eq!((s.ahead, s.behind), (1, 0));
        let s = head("## main...origin/main [gone]");
        assert_eq!(
            (s.branch.as_deref(), s.ahead, s.behind),
            (Some("main"), 0, 0)
        );
        let s = head("## main...origin/main");
        assert_eq!(
            (s.branch.as_deref(), s.ahead, s.behind),
            (Some("main"), 0, 0)
        );
        let s = head("## solo");
        assert_eq!((s.branch.as_deref(), s.detached), (Some("solo"), false));
        let s = head("## HEAD (no branch)");
        assert_eq!((s.branch, s.detached), (None, true));
        let s = head("## No commits yet on trunk");
        assert_eq!((s.branch.as_deref(), s.detached), (Some("trunk"), false));
        let s = head("## Initial commit on master");
        assert_eq!(s.branch.as_deref(), Some("master"));
        assert_eq!(
            parse_porcelain("/r", "", 10),
            GitStatus {
                top: "/r".into(),
                branch: None,
                detached: false,
                ahead: 0,
                behind: 0,
                marks: vec![],
                truncated: false,
                untrusted: false
            }
        );
    }

    #[test]
    fn every_mark_kind() {
        let out = nul(&[
            "## main",
            " M modified.rs",
            "M  staged.rs",
            "MM both.rs",
            "A  added.rs",
            "AM added-then-edited.rs",
            " D deleted.rs",
            "D  staged-delete.rs",
            "AD added-then-deleted.rs",
            "UU conflict.rs",
            "AA both-added.rs",
            "DD both-deleted.rs",
            "AU a.rs",
            "UD b.rs",
            "UA c.rs",
            "DU d.rs",
            "?? new file.rs",
            "!! ignored.rs",
            "T  typechange.rs",
        ]);
        let s = parse_porcelain("/repo", &out, 100);
        let marks: Vec<(&str, char)> = s.marks.iter().map(|(p, m)| (p.as_str(), *m)).collect();
        assert_eq!(
            marks,
            [
                ("/repo/modified.rs", 'M'),
                ("/repo/staged.rs", 'M'),
                ("/repo/both.rs", 'M'),
                ("/repo/added.rs", 'A'),
                ("/repo/added-then-edited.rs", 'A'),
                ("/repo/deleted.rs", 'D'),
                ("/repo/staged-delete.rs", 'D'),
                ("/repo/added-then-deleted.rs", 'D'),
                ("/repo/conflict.rs", 'U'),
                ("/repo/both-added.rs", 'U'),
                ("/repo/both-deleted.rs", 'U'),
                ("/repo/a.rs", 'U'),
                ("/repo/b.rs", 'U'),
                ("/repo/c.rs", 'U'),
                ("/repo/d.rs", 'U'),
                ("/repo/new file.rs", '?'),
                ("/repo/typechange.rs", 'M'),
            ]
        );
        assert!(!s.truncated);
    }

    #[test]
    fn records_are_nul_separated_so_odd_paths_survive() {
        let out = nul(&[
            "## main",
            "?? dir/with space/ünï.txt",
            " M a\nb.txt",
            "?? \"quoted\".txt",
        ]);
        let s = parse_porcelain("/", &out, 10);
        let paths: Vec<&str> = s.marks.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            ["/dir/with space/ünï.txt", "/a\nb.txt", "/\"quoted\".txt"]
        );
        // A rename's source is a record of its own and is not a change.
        let s = parse_porcelain("/r", nul(&["R  new.rs", "old.rs", " M x.rs"]), 10);
        assert_eq!(
            s.marks,
            [("/r/new.rs".to_owned(), 'M'), ("/r/x.rs".to_owned(), 'M')]
        );
        // Junk is ignored.
        assert!(
            parse_porcelain("/r", nul(&["x", "", "?"]), 10)
                .marks
                .is_empty()
        );
    }

    #[test]
    fn a_path_that_is_not_utf8_gets_no_mark() {
        // `b\xff.rs` read lossily is `b\u{FFFD}.rs`: the name of the file next to it.
        let out = b"## main\0 M b\xff.rs\0?? b\xef\xbf\xbd.rs\0R  c\xff.rs\0old.rs\0 M x.rs\0";
        let s = parse_porcelain("/r", out, 10);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(
            s.marks,
            [
                ("/r/b\u{FFFD}.rs".to_owned(), '?'),
                ("/r/x.rs".to_owned(), 'M')
            ]
        );
    }

    #[test]
    fn the_limit_truncates() {
        let out = nul(&["## main", " M a", " M b", " M c"]);
        let s = parse_porcelain("/r", &out, 2);
        assert_eq!((s.marks.len(), s.truncated), (2, true));
        let s = parse_porcelain("/r", &out, 3);
        assert_eq!((s.marks.len(), s.truncated), (3, false));
        let s = parse_porcelain("/r", &out, 0);
        assert_eq!((s.marks.len(), s.truncated), (0, true));
    }

    /// [`status`] here with no folder trusted (a plain repo needs none).
    fn local(cwd: &str, limit: usize) -> Result<Option<GitStatus>, String> {
        status_with(Path::new("/nonexistent/trusted.json"), cwd, None, limit)
    }

    fn git_present() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Runs git in `dir` with a config of its own (no user or system settings).
    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push((
                        e.path().to_string_lossy().into_owned(),
                        std::fs::read(e.path()).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, &mut out);
        out
    }

    #[test]
    fn a_real_repo() {
        if !git_present() {
            eprintln!("git not found: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            local(&root.to_string_lossy(), 100),
            Ok(None),
            "not a repo yet"
        );
        git_in(root, &["init", "-q"]);
        // Before the first commit.
        let fresh = local(&root.to_string_lossy(), 100).unwrap().unwrap();
        assert_eq!(fresh.branch.as_deref(), Some("main"));
        for (f, body) in [
            ("keep.txt", "keep\n"),
            ("edit.txt", "one\n"),
            ("gone.txt", "bye\n"),
        ] {
            std::fs::write(root.join(f), body).unwrap();
        }
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "first"]);
        std::fs::write(root.join("edit.txt"), "two\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/new file.txt"), "n\n").unwrap();
        std::fs::write(root.join("staged.txt"), "s\n").unwrap();
        git_in(root, &["add", "staged.txt"]);
        // Same content, new timestamp: a refresh of the index would rewrite it.
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();

        let before = snapshot(&root.join(".git"));
        let from_sub = local(&root.join("sub").to_string_lossy(), 100)
            .unwrap()
            .unwrap();
        let s = local(&root.to_string_lossy(), 100).unwrap().unwrap();
        assert_eq!(
            snapshot(&root.join(".git")),
            before,
            "status wrote under .git"
        );
        assert_eq!(from_sub, s, "same tree from a subfolder");

        let top = std::fs::canonicalize(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            (s.top.as_str(), s.branch.as_deref(), s.detached, s.truncated),
            (top.as_str(), Some("main"), false, false)
        );
        let mut marks = s.marks.clone();
        marks.sort();
        let at = |f: &str| join(&top, f);
        assert_eq!(
            marks,
            [
                (at("edit.txt"), 'M'),
                (at("gone.txt"), 'D'),
                (at("staged.txt"), 'A'),
                (at("sub/new file.txt"), '?')
            ]
        );

        let cut = local(&root.to_string_lossy(), 2).unwrap().unwrap();
        assert_eq!((cut.marks.len(), cut.truncated), (2, true));

        git_in(root, &["checkout", "-q", "--detach"]);
        let detached = local(&root.to_string_lossy(), 100).unwrap().unwrap();
        assert_eq!((detached.branch, detached.detached), (None, true));
    }

    #[cfg(unix)]
    #[test]
    fn an_untrusted_folder_runs_no_filter_of_the_repo() {
        use std::os::unix::fs::PermissionsExt;
        if !git_present() {
            eprintln!("git not found: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let fresh = |cwd: &str| refs_only(runner(cwd, None), "/r").unwrap();
        git_in(&root, &["init", "-q"]);
        assert_eq!(
            (fresh(&cwd).branch.as_deref(), fresh(&cwd).detached),
            (Some("main"), false),
            "before the first commit"
        );
        // A branch one commit ahead of its upstream and one behind it.
        std::fs::write(root.join(".gitattributes"), "*.txt filter=probe\n").unwrap();
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "first"]);
        git_in(&root, &["branch", "up"]);
        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "mine"]);
        git_in(&root, &["checkout", "-q", "up"]);
        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
        git_in(&root, &["checkout", "-q", "main"]);
        git_in(&root, &["branch", "-q", "--set-upstream-to=up"]);
        // The repo's own clean filter leaves a mark whenever it runs, and a
        // changed file is one status runs it on.
        let marker = tmp.path().join("ran");
        let program = tmp.path().join("clean.sh");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        git_in(
            &root,
            &["config", "filter.probe.clean", &program.to_string_lossy()],
        );
        std::fs::write(root.join("a.txt"), "2\n").unwrap();

        let trusted = tmp.path().join("trusted.json");
        let s = status_with(&trusted, &cwd, None, 100).unwrap().unwrap();
        assert!(
            !marker.exists(),
            "an untrusted folder's status ran the repo's filter"
        );
        assert_eq!(
            (s.branch.as_deref(), s.detached, s.ahead, s.behind),
            (Some("main"), false, 1, 1)
        );
        assert!(s.untrusted && s.marks.is_empty() && !s.truncated);

        // Trusted, git status runs as in a terminal: the oracle is live.
        git_trust::trust_at(&trusted, &git_trust::folder_id(None, &s.top)).unwrap();
        let full = status_with(&trusted, &cwd, None, 100).unwrap().unwrap();
        assert!(marker.exists(), "git status did not run the filter");
        assert!(!full.untrusted);
        assert_eq!(full.marks, [(join(&s.top, "a.txt"), 'M')]);
        assert_eq!(
            (full.branch, full.detached, full.ahead, full.behind),
            (s.branch, s.detached, s.ahead, s.behind),
            "the refs say what status says"
        );

        git_in(&root, &["checkout", "-q", "--detach"]);
        let s = fresh(&cwd);
        assert_eq!(
            (s.branch, s.detached, s.ahead, s.behind),
            (None, true, 0, 0)
        );
        // Outside a repo it fails (status() never gets that far).
        let elsewhere = tmp.path().join("plain");
        std::fs::create_dir(&elsewhere).unwrap();
        assert!(refs_only(runner(&elsewhere.to_string_lossy(), None), "/r").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_work_tree_set_elsewhere_is_checked_where_git_reads_it() {
        use std::os::unix::fs::PermissionsExt;
        if !git_present() {
            eprintln!("git not found: skipping");
            return;
        }
        // `evil`'s config names a clean filter and puts its work tree in
        // `clean`, a repository of its own with nothing to trust: the trust
        // check at the top level would read `clean`, git status from `evil`
        // runs `evil`'s filter.
        let tmp = tempfile::tempdir().unwrap();
        let (evil, clean) = (tmp.path().join("evil"), tmp.path().join("clean"));
        for dir in [&evil, &clean] {
            std::fs::create_dir(dir).unwrap();
            git_in(dir, &["init", "-q"]);
        }
        std::fs::write(clean.join("a.txt"), "1\n").unwrap();
        let marker = tmp.path().join("ran");
        let program = tmp.path().join("clean.sh");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        git_in(
            &evil,
            &["config", "filter.probe.clean", &program.to_string_lossy()],
        );
        git_in(
            &evil,
            &["config", "core.worktree", &clean.to_string_lossy()],
        );
        std::fs::write(evil.join(".git/info/attributes"), "* filter=probe\n").unwrap();
        git_in(&evil, &["add", "a.txt"]);
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(clean.join("a.txt"), "2\n").unwrap();

        let trusted = tmp.path().join("trusted.json");
        let cwd = evil.to_string_lossy().into_owned();
        let s = status_with(&trusted, &cwd, None, 100).unwrap().unwrap();
        assert!(
            !marker.exists(),
            "git status ran the filter of a repo the check never read"
        );
        assert!(s.untrusted && s.marks.is_empty());
        assert!(!work_tree_open(&trusted, &cwd, None));
        // Trusting the top level speaks for `clean`, not for `evil`.
        git_trust::trust_at(&trusted, &git_trust::folder_id(None, &s.top)).unwrap();
        assert!(
            status_with(&trusted, &cwd, None, 100)
                .unwrap()
                .unwrap()
                .untrusted
        );
        assert!(!work_tree_open(&trusted, &cwd, None));
        assert!(!marker.exists());
        // `clean` itself is what was trusted, and opens.
        assert!(work_tree_open(&trusted, &clean.to_string_lossy(), None));
        // The oracle is live: git status from `evil` does run the filter.
        git_in(&evil, &["status", "--porcelain"]);
        assert!(
            marker.exists(),
            "git status never ran the filter, so this proves nothing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reading_the_refs_alone_fetches_no_missing_commit() {
        use std::os::unix::fs::PermissionsExt;
        if !git_present() {
            eprintln!("git not found: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "one"]);
        // The upstream is a commit the clone doesn't have, from a promisor
        // remote reached with the repo's own ssh command.
        let marker = tmp.path().join("ran");
        let ssh = tmp.path().join("ssh.sh");
        std::fs::write(
            &ssh,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (key, value) in [
            ("remote.origin.url", "ssh://example.invalid/x"),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("remote.origin.promisor", "true"),
            ("core.sshCommand", &*ssh.to_string_lossy()),
            ("branch.main.remote", "origin"),
            ("branch.main.merge", "refs/heads/main"),
        ] {
            git_in(&root, &["config", key, value]);
        }
        let gone = Command::new("git")
            .args(["commit-tree", "-p", "HEAD", "-m", "gone", "HEAD^{tree}"])
            .current_dir(&root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        let gone = String::from_utf8_lossy(&gone.stdout).trim().to_owned();
        git_in(&root, &["update-ref", "refs/remotes/origin/main", &gone]);
        std::fs::remove_file(root.join(format!(".git/objects/{}/{}", &gone[..2], &gone[2..])))
            .unwrap();

        let s = refs_only(runner(&root.to_string_lossy(), None), "/r").unwrap();
        assert_eq!(
            (s.branch.as_deref(), s.ahead, s.behind),
            (Some("main"), 0, 0)
        );
        assert!(
            !marker.exists(),
            "reading the refs fetched with the repo's ssh command"
        );
        // The plain walk does fetch: the marker is a live oracle.
        let _ = Command::new("git")
            .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "rev-list did not fetch, so this proves nothing"
        );
    }

    #[test]
    fn a_missing_folder_is_not_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            status(&tmp.path().join("nope").to_string_lossy(), None, 10),
            Ok(None)
        );
    }
}
