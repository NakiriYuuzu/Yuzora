//! Git status for the Files lane: the branch and a mark per changed path, read
//! with `git status --porcelain` here or on a herdr-core machine. Blocking.
//! Writes nothing under `.git` (no optional locks, so no index refresh).

use std::time::Duration;

use yuzora_core::remote::{self, ExecError, ExecRequest};

use crate::fsops::join;

const TIMEOUT: Duration = Duration::from_secs(15);
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
}

/// The status of the work tree around `cwd`; `None` when `cwd` is not inside
/// one (or git is not there).
pub fn status(cwd: &str, on: Option<&str>, limit: usize) -> Result<Option<GitStatus>, String> {
    // Never an fsmonitor command (it would run per call) nor an index lock.
    let git = |args: &[&str]| {
        let hardened = ["--no-optional-locks", "-c", "core.fsmonitor=false"];
        let req = ExecRequest::new("git", hardened.iter().chain(args).copied())
            .on(on)
            .cwd(Some(cwd))
            .timeout(TIMEOUT)
            .max_output(MAX_OUTPUT);
        remote::run(&req)
    };
    let top = match git(&["rev-parse", "--show-toplevel"]) {
        Ok(out) if out.code == 0 => out.stdout_text().trim_end_matches(['\n', '\r']).to_owned(),
        Ok(_) | Err(ExecError::Spawn(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if top.is_empty() {
        return Ok(None);
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
    let mut status = parse_porcelain(&top, &String::from_utf8_lossy(bytes), limit);
    status.truncated |= capped;
    Ok(Some(status))
}

/// Reads `git status --porcelain=v1 -z --branch` output.
pub fn parse_porcelain(top: &str, out: &str, limit: usize) -> GitStatus {
    let mut status = GitStatus {
        top: top.to_owned(),
        branch: None,
        detached: false,
        ahead: 0,
        behind: 0,
        marks: Vec::new(),
        truncated: false,
    };
    let mut records = out.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("## ") {
            parse_branch(header, &mut status);
            continue;
        }
        let mut chars = record.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let path = chars.as_str();
        // A rename or copy is followed by its source path.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            records.next();
        }
        let Some(mark) = mark_of(x, y) else { continue };
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
        let head = |h: &str| parse_porcelain("/r", &nul(&[h]), 10);
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
                truncated: false
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
        let s = parse_porcelain("/r", &nul(&["R  new.rs", "old.rs", " M x.rs"]), 10);
        assert_eq!(
            s.marks,
            [("/r/new.rs".to_owned(), 'M'), ("/r/x.rs".to_owned(), 'M')]
        );
        // Junk is ignored.
        assert!(
            parse_porcelain("/r", &nul(&["x", "", "?"]), 10)
                .marks
                .is_empty()
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
            status(&root.to_string_lossy(), None, 100),
            Ok(None),
            "not a repo yet"
        );
        git_in(root, &["init", "-q"]);
        // Before the first commit.
        let fresh = status(&root.to_string_lossy(), None, 100).unwrap().unwrap();
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
        let from_sub = status(&root.join("sub").to_string_lossy(), None, 100)
            .unwrap()
            .unwrap();
        let s = status(&root.to_string_lossy(), None, 100).unwrap().unwrap();
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

        let cut = status(&root.to_string_lossy(), None, 2).unwrap().unwrap();
        assert_eq!((cut.marks.len(), cut.truncated), (2, true));

        git_in(root, &["checkout", "-q", "--detach"]);
        let detached = status(&root.to_string_lossy(), None, 100).unwrap().unwrap();
        assert_eq!((detached.branch, detached.detached), (None, true));
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
