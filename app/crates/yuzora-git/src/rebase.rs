//! Interactive rebase (IntelliJ A6): the pane edits the todo list, then git
//! runs it with Yuzora's helpers as its editors, as IntelliJ points
//! `GIT_EDITOR` at its own helper:
//!
//! - `GIT_SEQUENCE_EDITOR` copies the todo the pane wrote into
//!   `<git dir>/yuzora-rebase/todo` over git's;
//! - `GIT_EDITOR` answers git's message prompts: it reads which commit git
//!   is on from the last line of `rebase-merge/done` and writes the message
//!   the pane left for it (`yuzora-rebase/msg-<hash>`), or leaves git's own.
//!   So a reword or squash message lands on the right commit even after the
//!   rebase stopped for an edit or a conflict and was continued.
//!
//! Both are short shell functions, so they work on another machine too
//! (git runs editors through `sh`); nothing is installed.
//!
//! In-memory history editing (IntelliJ's `G/inMemory`, after git-revise):
//! [`reword_in_memory`] rewrites a message with `commit-tree` alone, never
//! touching the work tree or index. Drop, fixup and reorder change trees
//! and would need a merge per commit (`git merge-tree --write-tree`), left
//! for later: see docs/beta2/git.md.

use serde_json::{Value, json};

use crate::exec::{Git, GitError, GitResult, LONG_TIMEOUT};
use crate::repo::Repo;

const DIR: &str = "yuzora-rebase";

pub const SEQUENCE_EDITOR: &str = r#"f(){ cat "$(git rev-parse --git-dir)/yuzora-rebase/todo" > "$1"; }; f"#;
pub const MESSAGE_EDITOR: &str = r#"f(){ g="$(git rev-parse --git-dir)"; h=$(tail -n 1 "$g/rebase-merge/done" 2>/dev/null | awk '{print $2}'); if [ -n "$h" ]; then for m in "$g/yuzora-rebase/msg-$h"*; do if [ -f "$m" ]; then cat "$m" > "$1"; rm -f "$m"; break; fi; done; fi; :; }; f"#;

/// Adds the message helper to a rebase command (start or `--continue`).
pub fn with_editor(cmd: Git) -> Git {
    cmd.env("GIT_EDITOR", MESSAGE_EDITOR)
}

/// One todo line.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub action: String,
    /// The commit (or, for `update-ref`, the ref).
    pub target: String,
    pub subject: String,
    /// For reword, and for the last squash of a chain.
    pub message: Option<String>,
}

impl Entry {
    pub fn from_json(v: &Value) -> GitResult<Entry> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let action = s("action");
        if !matches!(action.as_str(), "pick" | "reword" | "edit" | "squash" | "fixup" | "drop" | "update-ref") {
            return Err(GitError::Other(format!("unknown rebase action {action}")));
        }
        let target = if action == "update-ref" { s("ref") } else { s("hash") };
        if target.is_empty() || target.contains(['\n', ' ']) {
            return Err(GitError::other("a rebase entry needs its commit"));
        }
        Ok(Entry { action, target, subject: s("subject").replace('\n', " "), message: v.get("message").and_then(Value::as_str).map(str::to_owned).filter(|m| !m.trim().is_empty()) })
    }
}

/// The todo text git will run.
pub fn todo_text(entries: &[Entry]) -> GitResult<String> {
    let first = entries.iter().find(|e| e.action != "drop" && e.action != "update-ref");
    if first.is_some_and(|e| e.action == "squash" || e.action == "fixup") {
        return Err(GitError::other("the first commit can't be squashed or fixed up into nothing"));
    }
    Ok(entries
        .iter()
        .map(|e| if e.action == "update-ref" { format!("update-ref {}\n", e.target) } else { format!("{} {} {}\n", e.action, e.target, e.subject) })
        .collect())
}

/// The commits `base..HEAD`, oldest first, as the pane lists them.
pub fn todo_list(repo: &Repo, base: &str) -> GitResult<Value> {
    crate::log::not_an_option(base)?;
    let out = repo.text(Git::read(["log", "--reverse", "--format=%x01%x01%H%x02%x02%P%x02%x02%s%x02%x02%an%x02%x02%at%x02%x02%B%x03%x03", &format!("{base}..HEAD"), "--"]))?;
    let mut list = Vec::new();
    for rec in out.split(crate::log::RECORD_START).filter_map(|r| r.split(crate::log::RECORD_END).next()).filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = rec.split(crate::log::FIELD).collect();
        if f.len() < 6 {
            continue;
        }
        let merge = f[1].split_whitespace().count() > 1;
        list.push(json!({ "hash": f[0], "short": &f[0][..f[0].len().min(8)], "subject": f[2], "author": f[3], "time": f[4].parse::<i64>().unwrap_or(0), "message": f[5].trim_end(), "merge": merge }));
    }
    // Branches pointing into the range (for update-ref).
    let refs = repo.text(Git::read(["for-each-ref", "--format=%(refname) %(objectname)", "refs/heads"]))?;
    let current = repo.run(Git::read(["symbolic-ref", "-q", "HEAD"]))?.stdout_text().trim().to_owned();
    let hashes: Vec<String> = list.iter().filter_map(|c| c["hash"].as_str().map(str::to_owned)).collect();
    let pointing: Vec<Value> = refs.lines().filter_map(|l| l.split_once(' ')).filter(|(r, h)| *r != current && hashes.iter().any(|x| x == h)).map(|(r, h)| json!({ "ref": r, "hash": h })).collect();
    Ok(json!({ "commits": list, "refs": pointing, "hasMerges": list.iter().any(|c| c["merge"] == true) }))
}

/// Runs `git rebase -i <base>` with the pane's todo and messages.
pub fn run(repo: &Repo, base: &str, entries: &[Entry], update_refs: bool) -> GitResult<Value> {
    // After `rebase -i`'s options: `--exec=<cmd>` would run the command.
    crate::log::not_an_option(base)?;
    // The messages of a stopped rebase are still waiting in DIR.
    let op = crate::repo::operation(repo);
    if !op.kind.is_empty() {
        return Err(GitError::Other(format!("cannot start a rebase while {}", op.kind)));
    }
    let todo = todo_text(entries)?;
    let dir = repo.git_path(DIR);
    crate::repo::remove_path(&repo.place, &dir)?;
    repo.write_file(&format!("{dir}/todo"), todo.as_bytes())?;
    // Messages, keyed by the commit git is on when it asks: a reword's own,
    // a squash chain's last member.
    for (i, e) in entries.iter().enumerate() {
        let Some(msg) = &e.message else { continue };
        let asks = match e.action.as_str() {
            "reword" => true,
            "squash" | "fixup" => !entries.get(i + 1).is_some_and(|n| n.action == "squash" || n.action == "fixup"),
            _ => false,
        };
        if asks {
            repo.write_file(&format!("{dir}/msg-{}", e.target), msg.as_bytes())?;
        }
    }
    let mut cmd = with_editor(Git::write(["rebase", "-i", "--no-autosquash"]).env("GIT_SEQUENCE_EDITOR", SEQUENCE_EDITOR).timeout(LONG_TIMEOUT));
    if update_refs {
        cmd = cmd.arg("--update-refs");
    }
    let out = repo.run(cmd.arg(base))?;
    let op = crate::repo::operation(repo);
    if op.kind.is_empty() {
        let _ = crate::repo::remove_path(&repo.place, &dir);
    }
    let conflicts: Vec<String> = repo.run(Git::read(["diff", "--name-only", "--diff-filter=U", "-z"])).ok().map(|o| crate::status::split_z(&o.stdout).collect()).unwrap_or_default();
    repo.invalidate_refs();
    let text = [out.stdout_text(), out.stderr_text()].join("\n").trim().to_owned();
    Ok(json!({ "ok": out.ok(), "output": text, "stopped": !op.kind.is_empty(), "conflicts": conflicts, "operation": op.kind }))
}

/// Drops the messages of a rebase that ended, so a later one never uses them.
pub fn forget(repo: &Repo) {
    let _ = crate::repo::remove_path(&repo.place, &repo.git_path(DIR));
}

/// Rewrites one commit's message without touching the work tree or index:
/// the commit and everything after it are recreated (same trees, same
/// authors, the later messages byte for byte), then the branch moves. Only
/// on a linear history up to HEAD, on a branch.
pub fn reword_in_memory(repo: &Repo, hash: &str, message: &str) -> GitResult<Value> {
    if message.trim().is_empty() {
        return Err(GitError::other("the message is empty"));
    }
    let branch = repo.run(Git::read(["symbolic-ref", "-q", "HEAD"]))?;
    if !branch.ok() {
        return Err(GitError::other("HEAD is detached"));
    }
    let branch = branch.stdout_text().trim().to_owned();
    if !crate::repo::operation(repo).kind.is_empty() {
        return Err(GitError::other("another operation is in progress"));
    }
    let target = repo.text(Git::read(["rev-parse", "--verify", &format!("{hash}^{{commit}}")]))?.trim().to_owned();
    let old_head = repo.text(Git::read(["rev-parse", "HEAD"]))?.trim().to_owned();
    if repo.run(Git::read(["merge-base", "--is-ancestor", &target, "HEAD"]))?.code != 0 {
        return Err(GitError::other("the commit isn't on the current branch"));
    }
    // target and its descendants up to HEAD, oldest first, each one parent.
    let chain = repo.text(Git::read(["rev-list", "--reverse", "--parents", "--ancestry-path", &format!("{target}^..HEAD")]))?;
    let root = repo.run(Git::read(["rev-parse", "-q", "--verify", &format!("{target}^")]))?;
    let parent_of_target = if root.ok() { Some(root.stdout_text().trim().to_owned()) } else { None };
    let mut rows: Vec<Vec<String>> = if parent_of_target.is_some() {
        chain.lines().map(|l| l.split_whitespace().map(str::to_owned).collect()).collect()
    } else {
        let all = repo.text(Git::read(["rev-list", "--reverse", "--parents", "HEAD"]))?;
        all.lines().map(|l| l.split_whitespace().map(str::to_owned).collect()).collect()
    };
    rows.retain(|r| !r.is_empty());
    if rows.first().map(|r| r[0].as_str()) != Some(target.as_str()) || rows.iter().any(|r| r.len() > 2) {
        return Err(GitError::other("only a linear history (no merges) can be reworded in place"));
    }
    let mut new_parent = parent_of_target;
    // Who commits them now, as `git commit-tree` would say.
    let committer = repo.text(Git::read(["var", "GIT_COMMITTER_IDENT"]))?.trim().to_owned();
    for (i, r) in rows.iter().enumerate() {
        // Bytes: a message or a name in another encoding stays as it is.
        let raw = repo.run(Git::read(["cat-file", "commit", &r[0]]))?.into_result()?;
        let msg = (i == 0).then(|| message.trim_end().to_owned() + "\n");
        let object = replay(&raw, new_parent.as_deref(), &committer, msg.as_deref());
        // The write lock is taken per command; the branch only moves at the end.
        let new = repo.text(Git::write(["hash-object", "-t", "commit", "-w", "--stdin"]).stdin(object))?.trim().to_owned();
        new_parent = Some(new);
    }
    let new_head = new_parent.ok_or_else(|| GitError::other("nothing was rewritten"))?;
    repo.run(Git::write(["update-ref", "-m", &format!("yuzora: reword {}", &target[..8.min(target.len())]), &branch, &new_head, &old_head]))?.into_result()?;
    repo.invalidate_refs();
    Ok(json!({ "ok": true, "head": new_head }))
}

/// The commit `raw` again on `parent`, committed by `committer`: its other
/// headers (author, encoding...) and message byte for byte, or `message`
/// in its place (UTF-8, so no `encoding`). A signature no longer holds.
fn replay(raw: &[u8], parent: Option<&str>, committer: &str, message: Option<&str>) -> Vec<u8> {
    let (headers, body) = match raw.windows(2).position(|w| w == b"\n\n") {
        Some(i) => (&raw[..i], &raw[i + 2..]),
        None => (raw, &b""[..]),
    };
    let mut out = Vec::with_capacity(raw.len() + 100);
    let mut line = |l: &[u8]| {
        out.extend_from_slice(l);
        out.push(b'\n');
    };
    let mut dropped = false;
    for l in headers.split(|b| *b == b'\n') {
        // A header's further lines start with a space.
        if l.starts_with(b" ") {
            if !dropped {
                line(l);
            }
            continue;
        }
        dropped = false;
        match l.split(|b| *b == b' ').next().unwrap_or_default() {
            b"tree" => {
                line(l);
                if let Some(p) = parent {
                    line(format!("parent {p}").as_bytes());
                }
            }
            b"committer" => line(format!("committer {committer}").as_bytes()),
            b"parent" => {}
            b"encoding" if message.is_some() => {}
            b"gpgsig" | b"gpgsig-sha256" => dropped = true,
            _ => line(l),
        }
    }
    out.push(b'\n');
    out.extend_from_slice(message.map_or(body, str::as_bytes));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;

    fn entry(action: &str, hash: &str, message: Option<&str>) -> Entry {
        Entry { action: action.into(), target: hash.into(), subject: "s".into(), message: message.map(str::to_owned) }
    }

    #[test]
    fn todo_refuses_a_leading_squash() {
        assert!(todo_text(&[entry("squash", "a", None)]).is_err());
        assert_eq!(todo_text(&[entry("drop", "a", None), entry("pick", "b", None)]).unwrap(), "drop a s\npick b s\n");
    }

    #[test]
    fn an_interactive_rebase_rewords_squashes_drops_and_reorders() {
        let s = scratch();
        let base = s.commit("base.txt", "0\n", "base");
        let a = s.commit("a.txt", "a\n", "add a");
        let b = s.commit("b.txt", "b\n", "add b");
        let c = s.commit("c.txt", "c\n", "add c");
        let d = s.commit("d.txt", "d\n", "add d");
        let list = todo_list(&s.repo, &base).unwrap();
        assert_eq!(list["commits"].as_array().unwrap().len(), 4);
        assert_eq!(list["commits"][0]["subject"], "add a");
        let entries = vec![
            entry("pick", &c, None),
            entry("reword", &a, Some("A, reworded")),
            entry("squash", &b, Some("A and B together")),
            entry("drop", &d, None),
        ];
        let r = run(&s.repo, &base, &entries, false).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        let log = s.git(&["log", "--format=%s", &format!("{base}..HEAD")]);
        assert_eq!(log, "A and B together\nadd c\n");
        assert!(!std::path::Path::new(&s.repo.abs("d.txt")).exists());
        assert!(!std::path::Path::new(&s.repo.git_path(DIR)).exists(), "cleaned up");
    }

    #[test]
    fn a_stopped_rebase_keeps_its_messages_until_it_ends() {
        let s = scratch();
        let base = s.commit("base.txt", "0\n", "base");
        let a = s.commit("a.txt", "a\n", "add a");
        let b = s.commit("b.txt", "b\n", "add b");
        let r = run(&s.repo, &base, &[entry("edit", &a, None), entry("reword", &b, Some("B, later"))], false).unwrap();
        assert_eq!(r["stopped"], true, "{r}");
        let msg = format!("{}/msg-{b}", s.repo.git_path(DIR));
        // A second start is refused and leaves the pending message alone.
        assert!(run(&s.repo, &base, &[entry("pick", &a, None)], false).is_err());
        assert!(std::path::Path::new(&msg).exists());
        crate::ops::sequencer(&s.repo, "abort").unwrap();
        assert!(!std::path::Path::new(&s.repo.git_path(DIR)).exists(), "an abort leaves stale messages");
    }

    #[test]
    fn an_edit_stops_and_a_reword_after_it_still_lands() {
        let s = scratch();
        let base = s.commit("base.txt", "0\n", "base");
        let a = s.commit("a.txt", "a\n", "add a");
        let b = s.commit("b.txt", "b\n", "add b");
        let r = run(&s.repo, &base, &[entry("edit", &a, None), entry("reword", &b, Some("B, later"))], false).unwrap();
        assert_eq!(r["stopped"], true);
        let cont = crate::ops::sequencer(&s.repo, "continue").unwrap();
        assert_eq!(cont["ok"], true, "{cont}");
        assert_eq!(s.git(&["log", "-1", "--format=%s"]).trim(), "B, later");
    }

    #[test]
    fn reword_in_memory_leaves_the_work_tree_alone() {
        let s = scratch();
        s.commit("a.txt", "a\n", "first");
        let mid = s.commit("b.txt", "b\n", "middle");
        s.commit("c.txt", "c\n", "last");
        s.write("dirty.txt", "untracked\n");
        s.write("a.txt", "edited\n");
        reword_in_memory(&s.repo, &mid, "middle, better").unwrap();
        assert_eq!(s.git(&["log", "--format=%s"]), "last\nmiddle, better\nfirst\n");
        assert_eq!(s.read("a.txt"), "edited\n");
        assert_eq!(s.git(&["status", "--porcelain"]), " M a.txt\n?? dirty.txt\n");
    }

    #[cfg(unix)]
    #[test]
    fn rewording_keeps_later_commits_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;
        let s = scratch();
        s.commit("base.txt", "0\n", "base");
        let a = s.commit("a.txt", "a\n", "add a");
        s.write("b.txt", "b\n");
        s.git(&["add", "b.txt"]);
        // A later commit in Latin-1, as its encoding header says, by an
        // author whose name is Latin-1 too.
        let msg = format!("{}/.git/yz-msg", s.root());
        std::fs::write(&msg, b"caf\xe9\n").unwrap();
        let out = std::process::Command::new("git").args(["-c", "i18n.commitEncoding=ISO-8859-1", "commit", "-q", "-F", &msg]).env("GIT_AUTHOR_NAME", std::ffi::OsStr::from_bytes(b"Jos\xe9")).current_dir(s.root()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let raw = |rev: &str| s.repo.run(Git::read(["cat-file", "commit", rev])).unwrap().into_result().unwrap();
        // All of a commit but what rewording must change.
        let kept = |c: Vec<u8>| c.split(|b| *b == b'\n').filter(|l| !l.starts_with(b"parent ") && !l.starts_with(b"committer ")).map(<[u8]>::to_vec).collect::<Vec<_>>();
        let before = kept(raw("HEAD"));
        reword_in_memory(&s.repo, &a, "A, reworded").unwrap();
        assert_eq!(s.git(&["log", "-1", "--format=%s", "HEAD~1"]), "A, reworded\n");
        assert_eq!(kept(raw("HEAD")), before, "the later commit's message, encoding or author changed");
    }

    #[test]
    fn a_base_is_never_read_as_an_option() {
        let s = scratch();
        let base = s.commit("base.txt", "0\n", "base");
        let a = s.commit("a.txt", "1\n", "a");
        let file = s.dir.path().join("written");
        // What git writes is named after the whole argument (`written..HEAD`).
        let written = || std::fs::read_dir(s.dir.path()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("written"));
        let output = format!("--output={}", file.display());
        assert!(todo_list(&s.repo, &output).is_err_and(|e| e.to_string().starts_with("not a revision")));
        assert!(!written(), "the list's log took the base for an option");
        // `rebase -i --exec=<cmd>` would run the command after each commit.
        let exec = format!("--exec=touch {}", file.display());
        assert!(run(&s.repo, &exec, &[entry("pick", &a, None)], false).is_err_and(|e| e.to_string().starts_with("not a revision")));
        assert!(!written(), "the rebase took the base for an option");
        assert!(todo_list(&s.repo, &base).is_ok());
    }
}
