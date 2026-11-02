//! Everything else git does for the panes: files in and out of the index,
//! commit, branches, fetch / pull / push, stash, cherry-pick, revert,
//! reset, tags, remotes, patches, worktrees, submodules and blame.
//!
//! Expected outcomes that aren't failures (a push rejected, a checkout
//! blocked by local changes, a merge that stopped on conflicts) come back as
//! fields of the answer, so the pane can offer the next step; git's own
//! errors come back as errors with git's message.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::exec::{Cancel, Git, GitError, GitResult, LONG_TIMEOUT};
use crate::log::{META_FORMAT_LIST, to_hex};
use crate::repo::Repo;
use crate::status::{Scope, split_z};

fn done(repo: &Repo, refs: bool) {
    if refs { repo.invalidate_refs() } else { repo.invalidate(Scope::All) }
}

/// git's answer as `{ok, output}` (stdout and stderr together).
fn outcome(out: &crate::exec::Output) -> Value {
    let text = [out.stdout_text(), out.stderr_text()].iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    json!({ "ok": out.ok(), "output": text })
}

fn conflicted(repo: &Repo) -> Vec<String> {
    repo.run(Git::read(["diff", "--name-only", "--diff-filter=U", "-z"])).ok().map(|o| split_z(&o.stdout).collect()).unwrap_or_default()
}

/// A write whose failure may mean conflicts: `{ok, output, conflicts}`.
fn with_conflicts(repo: &Repo, out: crate::exec::Output) -> Value {
    let mut v = outcome(&out);
    let c = if out.ok() { Vec::new() } else { conflicted(repo) };
    v["conflicts"] = json!(c);
    v["operation"] = json!(crate::repo::operation(repo).kind);
    done(repo, true);
    v
}

// ---- files in and out of the index ----

pub fn stage(repo: &Repo, paths: &[String]) -> GitResult<Value> {
    let cmd = if paths.is_empty() { Git::write(["add", "-A"]) } else { Git::write(["add", "-A"]).pathspec(paths) };
    repo.run(cmd)?.into_result()?;
    repo.invalidate(if paths.is_empty() { Scope::All } else { Scope::Paths(paths.to_vec()) });
    Ok(json!({ "ok": true }))
}

pub fn unstage(repo: &Repo, paths: &[String]) -> GitResult<Value> {
    let all = [".".to_string()];
    crate::diffview::unstage_paths(repo, if paths.is_empty() { &all } else { paths })?;
    repo.invalidate(if paths.is_empty() { Scope::All } else { Scope::Paths(paths.to_vec()) });
    Ok(json!({ "ok": true }))
}

/// Throws away work tree changes: tracked files back to the index,
/// untracked files deleted. `rollback` also resets the index to HEAD.
pub fn discard(repo: &Repo, paths: &[String], rollback: bool) -> GitResult<Value> {
    crate::status::refresh(repo)?;
    let (tracked, untracked): (Vec<String>, Vec<String>) = {
        let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
        let u: HashSet<&String> = c.untracked.iter().collect();
        paths.iter().cloned().partition(|p| !u.contains(p))
    };
    if !tracked.is_empty() {
        let cmd = if rollback && repo.run(Git::read(["rev-parse", "-q", "--verify", "HEAD"]))?.ok() {
            Git::write(["restore", "--source=HEAD", "--staged", "--worktree"])
        } else {
            Git::write(["restore", "--worktree"])
        };
        repo.run(cmd.pathspec(&tracked))?.into_result()?;
    }
    // clean takes no --pathspec-from-file: a long list goes in runs.
    for run in crate::exec::pathspec_runs(&untracked) {
        repo.run(Git::write(["clean", "-f", "-q", "--"]).args(run.iter().cloned()))?.into_result()?;
    }
    repo.invalidate(Scope::Paths(paths.to_vec()));
    Ok(json!({ "ok": true }))
}

// ---- commit ----

#[derive(Debug, Default)]
pub struct CommitOptions {
    pub message: String,
    pub amend: bool,
    pub sign_off: bool,
    pub no_verify: bool,
    pub author: Option<String>,
    pub date: Option<String>,
    /// Stage everything tracked first (`commit -a`).
    pub all: bool,
}

impl CommitOptions {
    pub fn from_json(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        CommitOptions { message: v.get("message").and_then(Value::as_str).unwrap_or_default().to_owned(), amend: b("amend"), sign_off: b("signOff"), no_verify: b("noVerify"), author: s("author"), date: s("date"), all: b("all") }
    }
}

/// Warnings to confirm before committing (IntelliJ's check-in handlers).
pub fn commit_checks(repo: &Repo, amend: bool) -> GitResult<Value> {
    let mut warnings = Vec::new();
    let name = repo.run(Git::read(["config", "user.name"]))?;
    let email = repo.run(Git::read(["config", "user.email"]))?;
    if !name.ok() || !email.ok() {
        warnings.push(json!({ "kind": "identity" }));
    }
    crate::status::refresh(repo)?;
    let (detached, op, conflicts, staged): (bool, String, usize, Vec<String>) = {
        let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
        (c.branch.detached, c.operation.kind.clone(), c.tracked.values().filter(|e| e.conflict).count(), c.tracked.values().filter(|e| e.x != '.' && e.x != 'D').map(|e| e.path.clone()).collect())
    };
    if conflicts > 0 {
        warnings.push(json!({ "kind": "conflicts", "count": conflicts }));
    }
    if crate::settings::flag("warnDetachedHead") && (detached || op == "rebasing") {
        warnings.push(json!({ "kind": if op == "rebasing" { "rebasing" } else { "detached" } }));
    }
    if staged.is_empty() && !amend {
        warnings.push(json!({ "kind": "nothing" }));
    }
    if crate::settings::flag("largeFileWarn") && !repo.place.is_remote() {
        let limit = (crate::settings::number("largeFileMb").max(1.0) * 1024.0 * 1024.0) as u64;
        let big: Vec<&String> = staged.iter().filter(|p| std::fs::metadata(repo.abs(p)).is_ok_and(|m| m.len() > limit)).take(20).collect();
        if !big.is_empty() {
            warnings.push(json!({ "kind": "large", "files": big }));
        }
    }
    if crate::settings::flag("warnCrlf") && !repo.place.is_remote() {
        let autocrlf = repo.run(Git::read(["config", "--get", "core.autocrlf"]))?.stdout_text();
        if autocrlf.trim().is_empty() || autocrlf.trim() == "false" {
            let small = |p: &String| std::fs::metadata(repo.abs(p)).is_ok_and(|m| m.is_file() && m.len() < (4 << 20));
            let crlf: Vec<&String> = staged.iter().filter(|p| small(p) && std::fs::read(repo.abs(p)).is_ok_and(|b| !b[..b.len().min(8000)].contains(&0) && b.windows(2).any(|w| w == b"\r\n"))).take(20).collect();
            if !crlf.is_empty() {
                warnings.push(json!({ "kind": "crlf", "files": crlf }));
            }
        }
    }
    Ok(Value::Array(warnings))
}

pub fn commit(repo: &Repo, o: &CommitOptions) -> GitResult<Value> {
    if o.message.trim().is_empty() && !o.amend {
        return Err(GitError::other("the commit message is empty"));
    }
    let mut cmd = Git::write(["commit", "-F", "-"]).stdin(o.message.clone().into_bytes()).timeout(LONG_TIMEOUT);
    let cleanup = crate::settings::text("commitCleanup");
    if !cleanup.is_empty() {
        cmd = cmd.arg(format!("--cleanup={cleanup}"));
    }
    if o.amend {
        cmd = cmd.arg("--amend");
        if o.message.trim().is_empty() {
            cmd = Git::write(["commit", "--amend", "--no-edit"]).timeout(LONG_TIMEOUT);
        }
    }
    if o.sign_off {
        cmd = cmd.arg("--signoff");
    }
    if o.no_verify {
        cmd = cmd.arg("--no-verify");
    }
    if o.all {
        cmd = cmd.arg("--all");
    }
    if let Some(a) = &o.author {
        cmd = cmd.arg(format!("--author={a}"));
    }
    if let Some(d) = &o.date {
        cmd = cmd.arg(format!("--date={d}"));
    }
    let out = repo.run(cmd)?;
    done(repo, true);
    if !out.ok() {
        return Err(GitError::Failed { code: out.code, message: [out.stderr_text(), out.stdout_text()].join("\n").trim().to_owned() });
    }
    if !o.message.trim().is_empty() {
        remember_message(repo, &o.message);
    }
    let hash = repo.text(Git::read(["rev-parse", "HEAD"]))?.trim().to_owned();
    Ok(json!({ "ok": true, "hash": hash, "output": out.stdout_text().trim() }))
}

fn messages_file() -> std::path::PathBuf {
    crate::settings::own_dir().join("git").join("messages.json")
}

fn remember_message(repo: &Repo, message: &str) {
    let file = messages_file();
    let mut all: BTreeMap<String, Vec<String>> = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let list = all.entry(repo.id.clone()).or_default();
    list.retain(|m| m.trim() != message.trim());
    list.insert(0, message.trim().to_owned());
    list.truncate((crate::settings::number("messageHistory") as usize).clamp(1, 100));
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, serde_json::to_string_pretty(&all).unwrap_or_default());
}

/// Earlier messages for this repo, newest first; the template; the last commit's message.
pub fn messages(repo: &Repo) -> GitResult<Value> {
    let all: BTreeMap<String, Vec<String>> = std::fs::read_to_string(messages_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let history = all.get(&repo.id).cloned().unwrap_or_default();
    let template = repo.run(Git::read(["config", "--path", "commit.template"]))?;
    let template = template.ok().then(|| template.stdout_text().trim().to_owned()).filter(|p| !p.is_empty()).and_then(|p| {
        let abs = if p.starts_with('/') || p.starts_with('~') { p.replacen('~', &yuzora_core::paths::home_dir().to_string_lossy(), 1) } else { repo.abs(&p) };
        repo.read_file(&abs).map(|b| String::from_utf8_lossy(&b).into_owned())
    });
    let last = repo.run(Git::read(["log", "-1", "--format=%B"]))?;
    let authors = repo.run(Git::read(["log", "-n", "300", "--format=%an <%ae>"]))?;
    let mut seen = HashSet::new();
    let authors: Vec<String> = authors.stdout_text().lines().filter(|a| seen.insert(a.to_string())).take(16).map(str::to_owned).collect();
    Ok(json!({ "history": history, "template": template, "last": last.ok().then(|| last.stdout_text().trim_end().to_owned()), "authors": authors }))
}

// ---- branches ----

pub fn branches(repo: &Repo) -> GitResult<Value> {
    let fmt = "--format=%(refname)%00%(objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(HEAD)%00%(committerdate:unix)%00%(subject)%00%(worktreepath)";
    let out = repo.text(Git::read(["for-each-ref", fmt, "refs/heads", "refs/remotes", "refs/tags"]))?;
    let (mut local, mut remote, mut tags) = (Vec::new(), Vec::new(), Vec::new());
    let mut current = None;
    for line in out.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 8 {
            continue;
        }
        let time = f[5].parse::<i64>().unwrap_or(0);
        if let Some(name) = f[0].strip_prefix("refs/heads/") {
            let track = f[3];
            let num = |k: &str| track.split(", ").find_map(|p| p.strip_prefix(k)).and_then(|n| n.trim().parse::<u32>().ok()).unwrap_or(0);
            if f[4] == "*" {
                current = Some(name.to_owned());
            }
            local.push(json!({
                "name": name, "hash": f[1], "upstream": (!f[2].is_empty()).then_some(f[2]),
                "ahead": num("ahead "), "behind": num("behind "), "gone": track == "gone",
                "current": f[4] == "*", "time": time, "subject": f[6],
                "worktree": (!f[7].is_empty() && f[4] != "*").then_some(f[7]),
            }));
        } else if let Some(name) = f[0].strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            let (r, b) = name.split_once('/').unwrap_or((name, ""));
            remote.push(json!({ "name": name, "remote": r, "branch": b, "hash": f[1], "time": time, "subject": f[6] }));
        } else if let Some(name) = f[0].strip_prefix("refs/tags/") {
            tags.push(json!({ "name": name, "hash": f[1], "time": time, "subject": f[6] }));
        }
    }
    // Recent: branches checked out lately, from HEAD's reflog.
    let reflog = repo.run(Git::read(["reflog", "show", "-n", "300", "--format=%gs", "HEAD"]))?.stdout_text();
    let names: HashSet<&str> = local.iter().filter_map(|b| b["name"].as_str()).collect();
    let mut recent = Vec::new();
    for l in reflog.lines() {
        if let Some(rest) = l.strip_prefix("checkout: moving from ")
            && let Some((_, to)) = rest.split_once(" to ")
            && names.contains(to)
            && Some(to) != current.as_deref()
            && !recent.contains(&to.to_owned())
        {
            recent.push(to.to_owned());
            if recent.len() == 8 {
                break;
            }
        }
    }
    tags.sort_by(|a, b| b["time"].as_i64().cmp(&a["time"].as_i64()));
    let head = repo.run(Git::read(["rev-parse", "-q", "--verify", "HEAD"]))?;
    Ok(json!({ "current": current, "head": head.ok().then(|| head.stdout_text().trim().to_owned()), "local": local, "remote": remote, "tags": tags, "recent": recent }))
}

/// Whether git refused because local changes would be overwritten.
fn blocked_by_changes(stderr: &str) -> bool {
    stderr.contains("would be overwritten") || stderr.contains("Please commit your changes or stash them")
}

/// Checks out a branch, remote branch (creating a tracking branch), tag or
/// commit. With `smart`, local changes in the way are stashed and restored
/// (IntelliJ's smart checkout).
pub fn checkout(repo: &Repo, target: &str, smart: bool, force: bool) -> GitResult<Value> {
    let locals = repo.text(Git::read(["for-each-ref", "--format=%(refname:short)", "refs/heads"]))?;
    let is_local = locals.lines().any(|l| l == target);
    let is_remote = !is_local && repo.run(Git::read(["show-ref", "--verify", "-q", &format!("refs/remotes/{target}")]))?.ok();
    let mut cmd = if is_local {
        Git::write(["switch", target])
    } else if is_remote {
        let local = target.split_once('/').map_or(target, |(_, b)| b);
        if locals.lines().any(|l| l == local) { Git::write(["switch", local]) } else { Git::write(["switch", "-c", local, "--track", target]) }
    } else {
        Git::write(["switch", "--detach", target])
    };
    if force {
        cmd = cmd.arg("--discard-changes");
    }
    let out = repo.run(cmd.clone())?;
    if out.ok() || !smart || !blocked_by_changes(&out.stderr_text()) {
        done(repo, true);
        let mut v = outcome(&out);
        v["blocked"] = json!(!out.ok() && blocked_by_changes(&out.stderr_text()));
        return Ok(v);
    }
    let top = |repo: &Repo| -> GitResult<String> { Ok(repo.run(Git::read(["rev-parse", "-q", "--verify", "refs/stash"]))?.stdout_text().trim().to_owned()) };
    let before = top(repo)?;
    let stash = repo.run(Git::write(["stash", "push", "--include-untracked", "-m", &format!("yuzora: smart checkout of {target}")]))?;
    if !stash.ok() {
        return Err(GitError::Failed { code: stash.code, message: stash.stderr_text() });
    }
    // "No local changes to save" (what blocked git isn't stashable, such as
    // an assume-unchanged file): no stash of ours, so nothing to pop.
    let ours = top(repo)?;
    if ours.is_empty() || ours == before {
        done(repo, true);
        let mut v = outcome(&out);
        v["blocked"] = json!(true);
        return Ok(v);
    }
    let switched = repo.run(cmd)?;
    // Pop ours only: if something else stashed meanwhile, apply it and keep it.
    let pop_args: &[&str] = if top(repo)? == ours { &["stash", "pop"] } else { &["stash", "apply"] };
    let with_index = |index: bool| {
        let mut c = Git::write(pop_args.iter().copied());
        if index {
            c = c.arg("--index");
        }
        if pop_args[1] == "apply" { c.arg(ours.clone()) } else { c }
    };
    let pop = repo.run(with_index(true))?;
    let pop = if !pop.ok() && pop.stderr_text().contains("--index") { repo.run(with_index(false))? } else { pop };
    let mut v = outcome(&switched);
    v["stashed"] = json!(true);
    v["restored"] = json!(pop.ok());
    v["conflicts"] = json!(if pop.ok() { Vec::new() } else { conflicted(repo) });
    if !pop.ok() {
        v["output"] = json!(format!("{}\n{}", v["output"].as_str().unwrap_or(""), pop.stderr_text().trim()));
    }
    done(repo, true);
    Ok(v)
}

pub fn branch_create(repo: &Repo, name: &str, start: Option<&str>, checkout: bool) -> GitResult<Value> {
    let mut cmd = if checkout { Git::write(["switch", "-c", name]) } else { Git::write(["branch", name]) };
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        cmd = cmd.arg(s);
    }
    let out = repo.run(cmd)?;
    done(repo, true);
    out.into_result().map(|_| json!({ "ok": true }))
}

pub fn branch_rename(repo: &Repo, old: &str, new: &str) -> GitResult<Value> {
    repo.run(Git::write(["branch", "-m", old, new]))?.into_result()?;
    done(repo, true);
    Ok(json!({ "ok": true }))
}

/// Deletes a local branch (`notMerged` when it needs `force`), or a remote one.
pub fn branch_delete(repo: &Repo, name: &str, force: bool, remote: bool, cancel: Option<Cancel>) -> GitResult<Value> {
    if remote {
        let (r, b) = name.split_once('/').ok_or_else(|| GitError::other("a remote branch is <remote>/<branch>"))?;
        let out = repo.run(Git::write(["push", r, "--delete", b]).cancel(cancel).timeout(LONG_TIMEOUT))?;
        done(repo, true);
        return Ok(outcome(&out));
    }
    let out = repo.run(Git::write(["branch", if force { "-D" } else { "-d" }, name]))?;
    done(repo, true);
    let mut v = outcome(&out);
    v["notMerged"] = json!(!out.ok() && out.stderr_text().contains("not fully merged"));
    Ok(v)
}

pub fn branch_track(repo: &Repo, branch: &str, upstream: Option<&str>) -> GitResult<Value> {
    let cmd = match upstream {
        Some(u) => Git::write(["branch", &format!("--set-upstream-to={u}"), branch]),
        None => Git::write(["branch", "--unset-upstream", branch]),
    };
    repo.run(cmd)?.into_result()?;
    done(repo, true);
    Ok(json!({ "ok": true }))
}

/// Commits on each side and the files that differ (`a...b`).
pub fn compare(repo: &Repo, a: &str, b: &str) -> GitResult<Value> {
    let list = |range: &str| -> GitResult<Vec<Value>> {
        let out = repo.text(Git::read(["log", "-n", "500", "--encoding=UTF-8", META_FORMAT_LIST, range, "--"]))?;
        Ok(parse_list(&out))
    };
    let files = repo.text(Git::read(["diff", "--name-status", "-z", "-M", &format!("{a}...{b}"), "--"]))?;
    // `files` is what b changed since the merge base: a file's diff is base → b.
    let base = repo.run(Git::read(["merge-base", a, b]))?;
    let base = base.ok().then(|| base.stdout_text().trim().to_owned()).filter(|h| !h.is_empty());
    Ok(json!({ "a": a, "b": b, "base": base, "onlyB": list(&format!("{a}..{b}"))?, "onlyA": list(&format!("{b}..{a}"))?, "files": crate::log::parse_name_status(&files) }))
}

/// `META_FORMAT_LIST` records as `{hash, subject, author, time}`.
pub fn parse_list(text: &str) -> Vec<Value> {
    text.split(crate::log::RECORD_START)
        .filter_map(|r| r.split(crate::log::RECORD_END).next())
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.split(crate::log::FIELD).collect();
            (f.len() >= 5).then(|| json!({ "hash": f[0], "short": &f[0][..f[0].len().min(8)], "subject": f[1], "author": f[2], "email": f[3], "time": f[4].parse::<i64>().unwrap_or(0) }))
        })
        .collect()
}

pub fn merge(repo: &Repo, target: &str, o: &Value) -> GitResult<Value> {
    let b = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut cmd = Git::write(["merge"]).timeout(LONG_TIMEOUT);
    for (k, flag) in [("noFf", "--no-ff"), ("ffOnly", "--ff-only"), ("squash", "--squash"), ("noCommit", "--no-commit"), ("noVerify", "--no-verify"), ("allowUnrelated", "--allow-unrelated-histories")] {
        if b(k) {
            cmd = cmd.arg(flag);
        }
    }
    if let Some(m) = o.get("message").and_then(Value::as_str).filter(|m| !m.trim().is_empty()) {
        cmd = cmd.args(["-m", m]);
    }
    let out = repo.run(cmd.arg(target))?;
    Ok(with_conflicts(repo, out))
}

pub fn rebase(repo: &Repo, upstream: &str, o: &Value) -> GitResult<Value> {
    let b = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut cmd = Git::write(["rebase"]).timeout(LONG_TIMEOUT);
    for (k, flag) in [("rebaseMerges", "--rebase-merges"), ("updateRefs", "--update-refs"), ("keepEmpty", "--keep-empty"), ("root", "--root")] {
        if b(k) {
            cmd = cmd.arg(flag);
        }
    }
    if b("autosquash") {
        cmd = cmd.arg("--autosquash");
    }
    if let Some(onto) = o.get("onto").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        cmd = cmd.args(["--onto", onto]);
    }
    if !b("root") {
        cmd = cmd.arg(upstream);
    }
    if let Some(branch) = o.get("branch").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        cmd = cmd.arg(branch);
    }
    let out = repo.run(cmd)?;
    Ok(with_conflicts(repo, out))
}

/// `continue`, `abort`, `skip` (or `quit`) for whatever is in progress.
pub fn sequencer(repo: &Repo, action: &str) -> GitResult<Value> {
    let op = crate::repo::operation(repo);
    let verb = match op.kind.as_str() {
        "rebasing" => "rebase",
        "applying" => "am",
        "merging" => "merge",
        "cherry-picking" => "cherry-pick",
        "reverting" => "revert",
        "" => return Err(GitError::other("nothing is in progress")),
        k => return Err(GitError::Other(format!("cannot {action} while {k}"))),
    };
    if !matches!(action, "continue" | "abort" | "skip" | "quit") {
        return Err(GitError::Other(format!("unknown action {action}")));
    }
    let mut cmd = Git::write([verb, &format!("--{action}")]).timeout(LONG_TIMEOUT);
    if verb == "rebase" {
        // Pending rewords and squash messages from Yuzora's interactive rebase.
        cmd = crate::rebase::with_editor(cmd);
    }
    let out = repo.run(cmd)?;
    if verb == "rebase" && crate::repo::operation(repo).kind.is_empty() {
        crate::rebase::forget(repo);
    }
    Ok(with_conflicts(repo, out))
}

// ---- fetch, pull, push ----

pub fn fetch(repo: &Repo, o: &Value, cancel: Option<Cancel>) -> GitResult<Value> {
    // `auto`: a fetch nobody asked for (Settings › Git › auto fetch). It waits
    // for a trusted folder and runs no hooks (fetch runs reference-transaction);
    // a fetch the person starts runs as it would in a terminal.
    let auto = o.get("auto").and_then(Value::as_bool).unwrap_or(false);
    if auto {
        let pending = crate::trust::pending(repo)?;
        if !pending.is_empty() {
            return Err(GitError::Other(format!("the folder isn't trusted ({})", pending.join(", "))));
        }
    }
    let mut cmd = if auto {
        // Hooks from an empty folder of ours (it doesn't exist on another
        // machine either), and submodules (their own configs) left alone.
        Git::read(["-c".to_owned(), format!("core.hooksPath={}", crate::trust::no_hooks_dir()), "fetch".to_owned(), "--no-recurse-submodules".to_owned()])
    } else {
        Git::read(["fetch"])
    }
    .timeout(LONG_TIMEOUT)
    .cancel(cancel);
    match o.get("remote").and_then(Value::as_str).filter(|r| !r.is_empty()) {
        Some(r) => cmd = cmd.arg(r),
        None => cmd = cmd.arg("--all"),
    }
    if o.get("prune").and_then(Value::as_bool).unwrap_or_else(|| crate::settings::flag("fetchPrune")) {
        cmd = cmd.arg("--prune");
    }
    match crate::settings::text("fetchTags").as_str() {
        "always" => cmd = cmd.arg("--tags"),
        "never" => cmd = cmd.arg("--no-tags"),
        _ => {}
    }
    // fetch writes refs, but must not wait behind our own writes: it is a READ
    // in IntelliJ too (its lock is git's own).
    let out = repo.run(cmd)?;
    done(repo, true);
    Ok(outcome(&out))
}

pub fn pull(repo: &Repo, o: &Value, cancel: Option<Cancel>) -> GitResult<Value> {
    let b = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut cmd = Git::write(["pull"]).timeout(LONG_TIMEOUT).cancel(cancel);
    let method = o.get("method").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| crate::settings::text("updateMethod"));
    match method.as_str() {
        "rebase" => cmd = cmd.arg("--rebase"),
        "ff-only" => cmd = cmd.arg("--ff-only"),
        "merge" => cmd = cmd.arg("--no-rebase"),
        _ => {}
    }
    for (k, flag) in [("noFf", "--no-ff"), ("squash", "--squash"), ("noCommit", "--no-commit"), ("noVerify", "--no-verify")] {
        if b(k) {
            cmd = cmd.arg(flag);
        }
    }
    if o.get("autoStash").and_then(Value::as_bool).unwrap_or_else(|| crate::settings::flag("autoStash")) {
        cmd = cmd.arg("--autostash");
    }
    if let Some(r) = o.get("remote").and_then(Value::as_str).filter(|r| !r.is_empty()) {
        cmd = cmd.arg(r);
        if let Some(br) = o.get("branch").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            cmd = cmd.arg(br);
        }
    }
    let out = repo.run(cmd)?;
    Ok(with_conflicts(repo, out))
}

/// Where a push of the current branch goes, and the commits it would send.
pub fn push_preview(repo: &Repo) -> GitResult<Value> {
    let branch = repo.run(Git::read(["symbolic-ref", "-q", "--short", "HEAD"]))?;
    if !branch.ok() {
        return Err(GitError::other("HEAD is detached: check out a branch to push"));
    }
    let branch = branch.stdout_text().trim().to_owned();
    let upstream = repo.run(Git::read(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}"]))?;
    let upstream = upstream.ok().then(|| upstream.stdout_text().trim().to_owned()).filter(|u| !u.is_empty());
    let remotes: Vec<String> = repo.text(Git::read(["remote"]))?.lines().map(str::to_owned).collect();
    let (remote, target) = match &upstream {
        Some(u) => {
            let (r, t) = u.split_once('/').unwrap_or((u, &branch));
            (r.to_owned(), t.to_owned())
        }
        None => (remotes.first().cloned().unwrap_or_else(|| "origin".into()), branch.clone()),
    };
    let range = match &upstream {
        Some(u) => format!("{u}..HEAD"),
        None => "HEAD".into(),
    };
    let mut cmd = Git::read(["log", "-n", "300", "--encoding=UTF-8", META_FORMAT_LIST, &range]);
    if upstream.is_none() {
        cmd = cmd.args(["--not", "--remotes"]);
    }
    let commits = parse_list(&repo.text(cmd.arg("--"))?);
    let tags = repo.run(Git::read(["log", "--simplify-by-decoration", "--decorate-refs=refs/tags", "--format=%D", &range]))?.stdout_text();
    let tags: Vec<&str> = tags.lines().flat_map(|l| l.split(", ")).filter_map(|t| t.strip_prefix("tag: ")).collect();
    Ok(json!({ "branch": branch, "remote": remote, "target": target, "remotes": remotes, "upstream": upstream, "commits": commits, "newBranch": upstream.is_none(), "protected": crate::settings::is_protected(&target), "tags": tags }))
}

pub fn push(repo: &Repo, o: &Value, cancel: Option<Cancel>) -> GitResult<Value> {
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty());
    let preview = push_preview(repo)?;
    let remote = s("remote").unwrap_or_else(|| preview["remote"].as_str().unwrap_or("origin").to_owned());
    let branch = s("branch").unwrap_or_else(|| preview["branch"].as_str().unwrap_or_default().to_owned());
    let target = s("target").unwrap_or_else(|| preview["target"].as_str().unwrap_or(&branch).to_owned());
    let force = s("force").unwrap_or_default();
    if !force.is_empty() && crate::settings::is_protected(&target) {
        return Err(GitError::Other(format!("{target} is protected: force push is not allowed (Settings → Git)")));
    }
    let build = || {
        let mut cmd = Git::read(["push", "--porcelain"]).timeout(LONG_TIMEOUT).cancel(cancel.clone());
        match force.as_str() {
            "lease" => cmd = cmd.arg("--force-with-lease"),
            "force" => cmd = cmd.arg(if crate::settings::flag("forceWithLease") { "--force-with-lease" } else { "--force" }),
            _ => {}
        }
        match s("tags").unwrap_or_else(|| crate::settings::text("pushTags")).as_str() {
            "follow" => cmd = cmd.arg("--follow-tags"),
            "all" => cmd = cmd.arg("--tags"),
            _ => {}
        }
        if o.get("noVerify").and_then(Value::as_bool).unwrap_or(false) {
            cmd = cmd.arg("--no-verify");
        }
        if o.get("setUpstream").and_then(Value::as_bool).unwrap_or(preview["newBranch"].as_bool().unwrap_or(false)) {
            cmd = cmd.arg("--set-upstream");
        }
        cmd.args([remote.clone(), format!("refs/heads/{branch}:refs/heads/{target}")])
    };
    let out = repo.run(build())?;
    let text = [out.stdout_text(), out.stderr_text()].join("\n");
    let rejected = !out.ok() && (text.contains("[rejected]") || text.contains("non-fast-forward") || text.contains("fetch first"));
    let auto = o.get("autoUpdate").and_then(Value::as_bool).unwrap_or_else(|| crate::settings::flag("autoUpdateOnReject"));
    if rejected && auto && force.is_empty() {
        let pulled = pull(repo, &json!({ "remote": remote, "branch": target }), cancel.clone())?;
        if !pulled["ok"].as_bool().unwrap_or(false) {
            let mut v = pulled;
            v["rejected"] = json!(true);
            v["updated"] = json!(false);
            return Ok(v);
        }
        let again = repo.run(build())?;
        done(repo, true);
        let mut v = outcome(&again);
        v["updated"] = json!(true);
        return Ok(v);
    }
    done(repo, true);
    let mut v = outcome(&out);
    v["rejected"] = json!(rejected);
    Ok(v)
}

pub fn remotes(repo: &Repo) -> GitResult<Value> {
    let out = repo.text(Git::read(["remote", "-v"]))?;
    let mut m: BTreeMap<String, (String, String)> = BTreeMap::new();
    for l in out.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() >= 3 {
            let e = m.entry(f[0].to_owned()).or_default();
            if f[2] == "(push)" { e.1 = f[1].to_owned() } else { e.0 = f[1].to_owned() }
        }
    }
    Ok(Value::Array(m.into_iter().map(|(name, (fetch, push))| json!({ "name": name, "url": fetch, "pushUrl": push })).collect()))
}

pub fn remote_edit(repo: &Repo, action: &str, name: &str, value: Option<&str>) -> GitResult<Value> {
    let cmd = match (action, value) {
        ("add", Some(url)) => Git::write(["remote", "add", name, url]),
        ("set-url", Some(url)) => Git::write(["remote", "set-url", name, url]),
        ("rename", Some(new)) => Git::write(["remote", "rename", name, new]),
        ("remove", _) => Git::write(["remote", "remove", name]),
        _ => return Err(GitError::Other(format!("remote {action} needs a value"))),
    };
    repo.run(cmd)?.into_result()?;
    done(repo, true);
    Ok(json!({ "ok": true }))
}

// ---- stash ----

pub fn stashes(repo: &Repo) -> GitResult<Value> {
    let out = repo.text(Git::read(["stash", "list", "--format=%gd%x00%H%x00%ct%x00%gs"]))?;
    Ok(Value::Array(
        out.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\0').collect();
                (f.len() >= 4).then(|| json!({ "ref": f[0], "hash": f[1], "time": f[2].parse::<i64>().unwrap_or(0), "message": f[3] }))
            })
            .collect(),
    ))
}

pub fn stash_push(repo: &Repo, o: &Value) -> GitResult<Value> {
    let mut cmd = Git::write(["stash", "push"]);
    if o.get("keepIndex").and_then(Value::as_bool).unwrap_or(false) {
        cmd = cmd.arg("--keep-index");
    }
    if o.get("untracked").and_then(Value::as_bool).unwrap_or(false) {
        cmd = cmd.arg("--include-untracked");
    }
    if let Some(m) = o.get("message").and_then(Value::as_str).filter(|m| !m.trim().is_empty()) {
        cmd = cmd.args(["-m", m]);
    }
    let paths: Vec<String> = o.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    let out = repo.run(cmd.pathspec(&paths))?;
    done(repo, true);
    Ok(outcome(&out))
}

/// `apply` / `pop` (with `index` to restore the index too), `drop`, `branch`.
pub fn stash_action(repo: &Repo, action: &str, stash: &str, o: &Value) -> GitResult<Value> {
    let cmd = match action {
        "apply" | "pop" => {
            let mut c = Git::write(["stash", action]);
            if o.get("index").and_then(Value::as_bool).unwrap_or(false) {
                c = c.arg("--index");
            }
            c.arg(stash)
        }
        "drop" => Git::write(["stash", "drop", stash]),
        "branch" => Git::write(["stash", "branch", o.get("branch").and_then(Value::as_str).ok_or_else(|| GitError::other("stash branch needs a name"))?, stash]),
        "clear" => Git::write(["stash", "clear"]),
        _ => return Err(GitError::Other(format!("unknown stash action {action}"))),
    };
    let out = repo.run(cmd)?;
    Ok(with_conflicts(repo, out))
}

// ---- history ----

pub fn cherry_pick(repo: &Repo, hashes: &[String], o: &Value) -> GitResult<Value> {
    let mut cmd = Git::write(["cherry-pick"]).timeout(LONG_TIMEOUT);
    if o.get("suffix").and_then(Value::as_bool).unwrap_or_else(|| crate::settings::flag("cherryPickSuffix")) {
        cmd = cmd.arg("-x");
    }
    if o.get("noCommit").and_then(Value::as_bool).unwrap_or(false) {
        cmd = cmd.arg("--no-commit");
    }
    // Oldest first, as they would be applied.
    let ordered = oldest_first(repo, hashes)?;
    let out = repo.run(cmd.args(ordered))?;
    Ok(with_conflicts(repo, out))
}

fn oldest_first(repo: &Repo, hashes: &[String]) -> GitResult<Vec<String>> {
    if hashes.len() < 2 {
        return Ok(hashes.to_vec());
    }
    let out = repo.text(Git::read(["rev-list", "--no-walk=sorted", "--reverse", "--topo-order"]).args(hashes.iter().cloned()))?;
    Ok(out.lines().map(str::to_owned).collect())
}

pub fn revert(repo: &Repo, hashes: &[String], o: &Value) -> GitResult<Value> {
    let mut cmd = Git::write(["revert", "--no-edit"]).timeout(LONG_TIMEOUT);
    if o.get("noCommit").and_then(Value::as_bool).unwrap_or(false) {
        cmd = cmd.arg("--no-commit");
    }
    // Newest first, so each revert applies cleanly on the last.
    let mut ordered = oldest_first(repo, hashes)?;
    ordered.reverse();
    let out = repo.run(cmd.args(ordered))?;
    Ok(with_conflicts(repo, out))
}

pub fn reset(repo: &Repo, rev: &str, mode: &str) -> GitResult<Value> {
    if !matches!(mode, "soft" | "mixed" | "hard" | "keep") {
        return Err(GitError::Other(format!("unknown reset mode {mode}")));
    }
    let out = repo.run(Git::write(["reset", &format!("--{mode}"), rev]))?;
    done(repo, true);
    Ok(outcome(&out))
}

/// Undo the last commit: its changes go back to the index (`reset --soft HEAD~`).
pub fn undo_commit(repo: &Repo) -> GitResult<Value> {
    let parents = repo.text(Git::read(["rev-list", "--parents", "-n", "1", "HEAD"]))?;
    let n = parents.split_whitespace().count();
    if n < 2 {
        return Err(GitError::other("the first commit can't be undone"));
    }
    if n > 2 {
        return Err(GitError::other("a merge commit can't be undone this way; use reset"));
    }
    let pushed = repo.run(Git::read(["branch", "-r", "--contains", "HEAD"]))?.stdout_text();
    let out = repo.run(Git::write(["reset", "--soft", "HEAD~1"]))?;
    done(repo, true);
    let mut v = outcome(&out);
    v["wasPushed"] = json!(!pushed.trim().is_empty());
    Ok(v)
}

pub fn tag_create(repo: &Repo, name: &str, rev: Option<&str>, message: Option<&str>) -> GitResult<Value> {
    let mut cmd = Git::write(["tag"]);
    if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
        cmd = cmd.args(["-a", "-m", m]);
    }
    cmd = cmd.arg(name);
    if let Some(r) = rev.filter(|r| !r.is_empty()) {
        cmd = cmd.arg(r);
    }
    repo.run(cmd)?.into_result()?;
    done(repo, true);
    Ok(json!({ "ok": true }))
}

pub fn tag_delete(repo: &Repo, name: &str, remote: Option<&str>, cancel: Option<Cancel>) -> GitResult<Value> {
    let out = match remote.filter(|r| !r.is_empty()) {
        Some(r) => repo.run(Git::read(["push", r, &format!(":refs/tags/{name}")]).timeout(LONG_TIMEOUT).cancel(cancel))?,
        None => repo.run(Git::write(["tag", "-d", name]))?,
    };
    done(repo, true);
    Ok(outcome(&out))
}

pub fn tag_push(repo: &Repo, remote: &str, name: Option<&str>, cancel: Option<Cancel>) -> GitResult<Value> {
    let cmd = match name {
        Some(n) => Git::read(["push", remote, &format!("refs/tags/{n}")]),
        None => Git::read(["push", remote, "--tags"]),
    };
    let out = repo.run(cmd.timeout(LONG_TIMEOUT).cancel(cancel))?;
    done(repo, true);
    Ok(outcome(&out))
}

// ---- patches ----

/// A patch: commits (`format-patch`), or the staged / unstaged changes of some paths.
pub fn patch_create(repo: &Repo, o: &Value) -> GitResult<String> {
    if let Some(hashes) = o.get("hashes").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let hs: Vec<String> = hashes.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        let mut text = String::new();
        for h in oldest_first(repo, &hs)? {
            text.push_str(&repo.text(Git::read(["format-patch", "--stdout", "--binary", "-1", &h]))?);
        }
        return Ok(text);
    }
    let paths: Vec<String> = o.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    let mut cmd = Git::read(["diff", "--binary", "--no-ext-diff"]);
    if o.get("staged").and_then(Value::as_bool).unwrap_or(false) {
        cmd = cmd.arg("--cached");
    }
    let (cmd, _) = cmd.paths_or_all(&paths);
    repo.text(cmd)
}

/// `git apply` (checked first): to the work tree, the index (`cached`), both
/// (`index`), with `threeWay` or `reverse`.
pub fn patch_apply(repo: &Repo, text: &str, o: &Value) -> GitResult<Value> {
    let b = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mut flags = Vec::new();
    for (k, f) in [("cached", "--cached"), ("index", "--index"), ("threeWay", "--3way"), ("reverse", "--reverse")] {
        if b(k) {
            flags.push(f.to_string());
        }
    }
    let check = repo.run(Git::read(["apply", "--check", "--stat"]).args(flags.clone()).stdin(text.as_bytes().to_vec()))?;
    if b("check") || (!check.ok() && !b("threeWay")) {
        return Ok(outcome(&check));
    }
    let out = repo.run(Git::write(["apply"]).args(flags).stdin(text.as_bytes().to_vec()))?;
    Ok(with_conflicts(repo, out))
}

// ---- worktrees, submodules ----

pub fn worktrees(repo: &Repo) -> GitResult<Value> {
    let out = repo.text(Git::read(["worktree", "list", "--porcelain", "-z"]))?;
    let mut list = Vec::new();
    let mut cur = serde_json::Map::new();
    for f in out.split('\0') {
        if f.is_empty() {
            if !cur.is_empty() {
                list.push(Value::Object(std::mem::take(&mut cur)));
            }
            continue;
        }
        let (k, v) = f.split_once(' ').unwrap_or((f, ""));
        match k {
            "worktree" => {
                cur.insert("path".into(), json!(v));
                cur.insert("current".into(), json!(v == repo.place.root));
            }
            "HEAD" => {
                cur.insert("head".into(), json!(v));
            }
            "branch" => {
                cur.insert("branch".into(), json!(v.trim_start_matches("refs/heads/")));
            }
            "detached" | "bare" | "locked" | "prunable" => {
                cur.insert(k.into(), if v.is_empty() { json!(true) } else { json!(v) });
            }
            _ => {}
        }
    }
    if !cur.is_empty() {
        list.push(Value::Object(cur));
    }
    Ok(Value::Array(list))
}

pub fn worktree_action(repo: &Repo, action: &str, path: Option<&str>, force: bool) -> GitResult<Value> {
    let cmd = match (action, path) {
        ("lock", Some(p)) => Git::write(["worktree", "lock", p]),
        ("unlock", Some(p)) => Git::write(["worktree", "unlock", p]),
        ("remove", Some(p)) if force => Git::write(["worktree", "remove", "--force", p]),
        ("remove", Some(p)) => Git::write(["worktree", "remove", p]),
        ("prune", _) => Git::write(["worktree", "prune"]),
        _ => return Err(GitError::Other(format!("worktree {action} needs a path"))),
    };
    let out = repo.run(cmd)?;
    done(repo, true);
    Ok(outcome(&out))
}

pub fn submodules(repo: &Repo) -> GitResult<Value> {
    let out = repo.run(Git::read(["submodule", "status", "--recursive"]))?;
    if !out.ok() {
        return Ok(json!([]));
    }
    Ok(Value::Array(
        out.stdout_text()
            .lines()
            .filter_map(|l| {
                let state = match l.chars().next()? {
                    '-' => "uninitialized",
                    '+' => "modified",
                    'U' => "conflict",
                    _ => "ok",
                };
                let mut f = l[1..].split_whitespace();
                let hash = f.next()?;
                let path = f.next()?;
                Some(json!({ "path": path, "hash": hash, "state": state, "describe": f.next().map(|d| d.trim_matches(|c| c == '(' || c == ')')) }))
            })
            .collect(),
    ))
}

pub fn submodule_update(repo: &Repo, cancel: Option<Cancel>) -> GitResult<Value> {
    let out = repo.run(Git::write(["submodule", "update", "--init", "--recursive"]).timeout(LONG_TIMEOUT).cancel(cancel))?;
    done(repo, true);
    Ok(outcome(&out))
}

// ---- config ----

/// The values the settings page shows for this repo (identity, signing).
pub fn config(repo: &Repo) -> GitResult<Value> {
    let get = |k: &str| -> Option<String> { repo.run(Git::read(["config", "--get", k])).ok().filter(|o| o.ok()).map(|o| o.stdout_text().trim().to_owned()) };
    let version = repo.text(Git::read(["--version"])).unwrap_or_default();
    Ok(json!({
        "userName": get("user.name"), "userEmail": get("user.email"),
        "gpgSign": get("commit.gpgSign").is_some_and(|v| v == "true"), "signingKey": get("user.signingKey"), "gpgFormat": get("gpg.format"),
        "autocrlf": get("core.autocrlf"), "hooksPath": get("core.hooksPath"),
        "version": version.trim(), "executable": crate::settings::executable(),
        // What each file says, for editing one of them (the values above are
        // what git uses: the repo's, else the user's).
        "local": config_layer(repo, "--local"), "global": config_layer(repo, "--global"),
    }))
}

/// The identity keys one config file sets (`--local` or `--global`).
fn config_layer(repo: &Repo, scope: &str) -> Value {
    let text = repo.run(Git::read(["config", scope, "--list", "-z"])).ok().filter(|o| o.ok()).map(|o| o.stdout_text()).unwrap_or_default();
    // `-z`: `key\nvalue\0`; a key alone is a boolean true. Keys come lowercased.
    let set: HashMap<String, String> = text.split('\0').filter(|e| !e.is_empty()).map(|e| e.split_once('\n').map_or((e.to_lowercase(), "true".to_owned()), |(k, v)| (k.to_lowercase(), v.to_owned()))).collect();
    let g = |k: &str| set.get(k).cloned();
    json!({
        "userName": g("user.name"), "userEmail": g("user.email"), "gpgSign": g("commit.gpgsign").map(|v| v == "true"),
        "signingKey": g("user.signingkey"), "gpgFormat": g("gpg.format"), "autocrlf": g("core.autocrlf"),
    })
}

/// Sets (or with null, unsets) one key: `local` to this repo, `global` for the user.
pub fn set_config(repo: &Repo, key: &str, value: Option<&str>, global: bool) -> GitResult<Value> {
    const ALLOWED: [&str; 6] = ["user.name", "user.email", "commit.gpgSign", "user.signingKey", "gpg.format", "core.autocrlf"];
    if !ALLOWED.contains(&key) {
        return Err(GitError::Other(format!("{key} isn't set from here")));
    }
    let scope = if global { "--global" } else { "--local" };
    let cmd = match value {
        Some(v) => Git::write(["config", scope, key, v]),
        None => Git::write(["config", scope, "--unset", key]),
    };
    let out = repo.run(cmd)?;
    // 5: unsetting a key that isn't there.
    if !out.ok() && !(value.is_none() && out.code == 5) {
        return Err(GitError::Failed { code: out.code, message: out.stderr_text() });
    }
    Ok(json!({ "ok": true }))
}

// ---- blame ----

/// `git blame --porcelain` (IntelliJ's annotate), cached per (path, rev, options).
pub fn blame(repo: &Repo, path: &str, rev: Option<&str>, o: &Value) -> GitResult<Arc<Value>> {
    let ignore_ws = o.get("ignoreWhitespace").and_then(Value::as_bool).unwrap_or_else(|| crate::settings::flag("annotateIgnoreSpaces"));
    let movement = o.get("movement").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| crate::settings::text("annotateDetectMovement"));
    let key = format!("{path}\0{}\0{ignore_ws}\0{movement}", rev.unwrap_or(""));
    // The work tree version changes under us; only revisions are cached.
    if rev.is_some()
        && let Some(v) = repo.blame.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
    {
        return Ok(v);
    }
    // The file's own lines: no textconv program from the repo's config.
    let mut cmd = Git::read(["blame", "--porcelain", "-l", "-t", "--encoding=UTF-8", "--no-textconv"]);
    if ignore_ws {
        cmd = cmd.arg("-w");
    }
    match movement.as_str() {
        "inner" => cmd = cmd.arg("-M"),
        "outer" => cmd = cmd.arg("-C"),
        _ => {}
    }
    if let Some(r) = rev {
        cmd = cmd.arg(r);
    }
    let out = repo.run(cmd.args(["--", path]))?.into_result()?;
    let v = Arc::new(parse_blame(&String::from_utf8_lossy(&out)));
    if rev.is_some() {
        repo.blame.lock().unwrap_or_else(|e| e.into_inner()).put(key, v.clone());
    }
    Ok(v)
}

pub fn parse_blame(text: &str) -> Value {
    let mut commits: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut lines = Vec::new();
    let mut cur: Option<(String, usize)> = None;
    for l in text.lines() {
        if let Some(content) = l.strip_prefix('\t') {
            if let Some((hash, final_line)) = cur.take() {
                lines.push(json!({ "line": final_line, "hash": hash, "text": content.replace('\t', "    ") }));
            }
            continue;
        }
        let mut f = l.split(' ');
        let first = f.next().unwrap_or("");
        if first.len() >= 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let _orig = f.next();
            let fin = f.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            commits.entry(first.to_owned()).or_default();
            cur = Some((first.to_owned(), fin));
            continue;
        }
        if let Some((hash, _)) = &cur {
            let rest = l.split_once(' ').map(|x| x.1).unwrap_or("");
            let key = match first {
                "author" => "author",
                "author-mail" => "email",
                "author-time" => "time",
                "summary" => "summary",
                "previous" => "previous",
                "filename" => "filename",
                "boundary" => "boundary",
                _ => continue,
            };
            let v = match key {
                "time" => json!(rest.parse::<i64>().unwrap_or(0)),
                "email" => json!(rest.trim_matches(|c| c == '<' || c == '>')),
                "boundary" => json!(true),
                _ => json!(rest),
            };
            commits.entry(hash.clone()).or_default().insert(key.into(), v);
        }
    }
    let commits: serde_json::Map<String, Value> = commits.into_iter().map(|(k, v)| (k, Value::Object(v))).collect();
    json!({ "lines": lines, "commits": commits })
}

/// The commit's tree as names (Show Repository at Revision), one level.
pub fn tree(repo: &Repo, rev: &str, dir: &str) -> GitResult<Value> {
    let spec = if dir.is_empty() { rev.to_owned() } else { format!("{rev}:{dir}") };
    let out = repo.run(Git::read(["ls-tree", "-z", "--long", &spec]))?.into_result()?;
    Ok(Value::Array(
        split_z(&out)
            .filter_map(|e| {
                let (meta, name) = e.split_once('\t')?;
                let f: Vec<&str> = meta.split_whitespace().collect();
                (f.len() >= 4).then(|| json!({ "name": name, "kind": f[1], "hash": f[2], "size": f[3].parse::<u64>().ok() }))
            })
            .collect(),
    ))
}

/// Short hashes for display.
pub fn short(hash: &[u8]) -> String {
    to_hex(hash)[..8].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;

    #[test]
    fn commit_remembers_messages_and_branches_list_tracking() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.write("a.txt", "2\n");
        stage(&s.repo, &["a.txt".into()]).unwrap();
        let checks = commit_checks(&s.repo, false).unwrap();
        assert!(checks.as_array().unwrap().iter().all(|w| w["kind"] != "nothing"));
        let r = commit(&s.repo, &CommitOptions { message: "second\n\nbody".into(), sign_off: true, ..Default::default() }).unwrap();
        assert_eq!(r["ok"], true);
        let body = s.git(&["log", "-1", "--format=%B"]);
        assert!(body.contains("Signed-off-by: Yuzora Test"), "{body}");
        let m = messages(&s.repo).unwrap();
        assert!(m["last"].as_str().unwrap().starts_with("second"));
        assert!(m["authors"][0].as_str().unwrap().starts_with("Yuzora Test"));
        assert!(commit(&s.repo, &CommitOptions::default()).is_err());
        branch_create(&s.repo, "feature", None, false).unwrap();
        let b = branches(&s.repo).unwrap();
        assert_eq!(b["current"], "main");
        assert_eq!(b["local"].as_array().unwrap().len(), 2);
        branch_rename(&s.repo, "feature", "feat").unwrap();
        assert!(branches(&s.repo).unwrap()["local"].as_array().unwrap().iter().any(|x| x["name"] == "feat"));
    }

    #[test]
    fn smart_checkout_carries_local_changes_across() {
        let s = scratch();
        s.commit("a.txt", "base\n", "base");
        s.git(&["branch", "other"]);
        s.commit("a.txt", "main\n", "on main");
        s.write("a.txt", "edited\n");
        let plain = checkout(&s.repo, "other", false, false).unwrap();
        assert_eq!(plain["ok"], false);
        assert_eq!(plain["blocked"], true);
        let smart = checkout(&s.repo, "other", true, false).unwrap();
        assert_eq!(smart["stashed"], true);
        assert_eq!(s.git(&["branch", "--show-current"]).trim(), "other");
        // The edit conflicts with other's a.txt, so the stash stays and says so.
        assert_eq!(smart["restored"], false);
        assert_eq!(smart["conflicts"], json!(["a.txt"]));
    }

    #[test]
    fn stash_reset_undo_cherry_pick_and_revert() {
        let s = scratch();
        s.commit("a.txt", "1\n", "one");
        let two = s.commit("b.txt", "1\n", "two");
        s.write("a.txt", "dirty\n");
        stash_push(&s.repo, &json!({ "message": "wip" })).unwrap();
        let list = stashes(&s.repo).unwrap();
        assert_eq!(list[0]["message"].as_str().unwrap(), "On main: wip");
        assert_eq!(s.read("a.txt"), "1\n");
        stash_action(&s.repo, "pop", "stash@{0}", &json!({})).unwrap();
        assert_eq!(s.read("a.txt"), "dirty\n");
        discard(&s.repo, &["a.txt".into()], false).unwrap();
        undo_commit(&s.repo).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]).trim(), "b.txt");
        s.git(&["commit", "-q", "-m", "two again"]);
        reset(&s.repo, "HEAD~1", "hard").unwrap();
        assert!(!std::path::Path::new(&s.repo.abs("b.txt")).exists());
        let picked = cherry_pick(&s.repo, &[two.clone()], &json!({ "suffix": true })).unwrap();
        assert_eq!(picked["ok"], true);
        assert!(s.git(&["log", "-1", "--format=%B"]).contains(&format!("cherry picked from commit {two}")));
        let reverted = revert(&s.repo, &[s.git(&["rev-parse", "HEAD"]).trim().to_owned()], &json!({})).unwrap();
        assert_eq!(reverted["ok"], true);
        assert!(!std::path::Path::new(&s.repo.abs("b.txt")).exists());
    }

    #[test]
    fn merge_conflicts_are_reported_and_blame_is_parsed() {
        let s = scratch();
        s.commit("f.txt", "base\n", "base");
        s.git(&["switch", "-q", "-c", "other"]);
        s.commit("f.txt", "theirs\n", "theirs");
        s.git(&["switch", "-q", "main"]);
        s.commit("f.txt", "ours\n", "ours");
        let m = merge(&s.repo, "other", &json!({})).unwrap();
        assert_eq!(m["ok"], false);
        assert_eq!(m["conflicts"], json!(["f.txt"]));
        assert_eq!(m["operation"], "merging");
        sequencer(&s.repo, "abort").unwrap();
        let b = blame(&s.repo, "f.txt", Some("HEAD"), &json!({})).unwrap();
        assert_eq!(b["lines"][0]["text"], "ours");
        let h = b["lines"][0]["hash"].as_str().unwrap();
        assert_eq!(b["commits"][h]["summary"], "ours");
        assert_eq!(b["commits"][h]["author"], "Yuzora Test");
    }

    #[test]
    fn patches_round_trip_and_tags_list() {
        let s = scratch();
        s.commit("a.txt", "1\n", "one");
        s.write("a.txt", "2\n");
        let p = patch_create(&s.repo, &json!({ "paths": ["a.txt"] })).unwrap();
        assert!(p.contains("-1\n+2"));
        discard(&s.repo, &["a.txt".into()], false).unwrap();
        let checked = patch_apply(&s.repo, &p, &json!({ "check": true })).unwrap();
        assert_eq!(checked["ok"], true);
        patch_apply(&s.repo, &p, &json!({})).unwrap();
        assert_eq!(s.read("a.txt"), "2\n");
        tag_create(&s.repo, "v1", None, Some("release")).unwrap();
        let b = branches(&s.repo).unwrap();
        assert_eq!(b["tags"][0]["name"], "v1");
        let w = worktrees(&s.repo).unwrap();
        assert_eq!(w[0]["branch"], "main");
        assert_eq!(w[0]["current"], true);
    }

    #[test]
    fn config_reads_each_file_apart() {
        let s = scratch();
        let c = config(&s.repo).unwrap();
        assert_eq!(c["local"]["userName"], c["userName"], "the scratch repo sets its own identity");
        set_config(&s.repo, "user.signingKey", Some("ABC123"), false).unwrap();
        let c = config(&s.repo).unwrap();
        assert_eq!(c["local"]["signingKey"], "ABC123");
        assert_eq!(c["signingKey"], "ABC123");
        set_config(&s.repo, "user.signingKey", None, false).unwrap();
        assert!(config(&s.repo).unwrap()["local"]["signingKey"].is_null());
        assert!(set_config(&s.repo, "core.editor", Some("vi"), false).is_err(), "only identity keys");
    }

    #[test]
    fn compare_lists_both_sides_and_the_merge_base() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let base = s.repo.text(Git::read(["rev-parse", "HEAD"])).unwrap().trim().to_owned();
        s.git(&["switch", "-q", "-c", "feature"]);
        s.commit("b.txt", "b\n", "on feature");
        s.git(&["switch", "-q", "main"]);
        s.commit("a.txt", "2\n", "on main");
        let c = compare(&s.repo, "main", "feature").unwrap();
        assert_eq!(c["base"], base.as_str());
        assert_eq!(c["onlyB"][0]["subject"], "on feature");
        assert_eq!(c["onlyA"][0]["subject"], "on main");
        let files = c["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "only b's side: {files:?}");
        assert_eq!(files[0]["path"], "b.txt");
    }

    #[test]
    fn discard_takes_paths_literally_and_cleans_long_lists() {
        let s = scratch();
        s.commit("pages/[id].tsx", "a\n", "first");
        s.commit("pages/i.tsx", "b\n", "second");
        s.write("pages/[id].tsx", "changed\n");
        s.write("pages/i.tsx", "kept\n");
        s.write("pages/[ab].txt", "new\n");
        s.write("pages/a.txt", "mine\n");
        discard(&s.repo, &["pages/[id].tsx".into(), "pages/[ab].txt".into()], false).unwrap();
        assert_eq!(std::fs::read_to_string(format!("{}/pages/i.tsx", s.root())).unwrap(), "kept\n", "a glob match was restored");
        assert!(std::path::Path::new(&format!("{}/pages/a.txt", s.root())).exists(), "a glob match was deleted");
        assert!(!std::path::Path::new(&format!("{}/pages/[ab].txt", s.root())).exists());
        // More untracked files than one command line holds.
        let many: Vec<String> = (0..400).map(|i| format!("dir/a-rather-long-untracked-file-name-number-{i:04}.txt")).collect();
        for p in &many {
            s.write(p, "x\n");
        }
        assert!(crate::exec::pathspec_runs(&many).len() > 1);
        s.repo.invalidate(Scope::All); // the watcher's job in the app
        discard(&s.repo, &many, false).unwrap();
        assert!(many.iter().all(|p| !std::path::Path::new(&format!("{}/{p}", s.root())).exists()));
    }

    #[test]
    fn smart_checkout_never_pops_a_stash_it_did_not_make() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.git(&["switch", "-q", "-c", "other"]);
        s.commit("a.txt", "2\n", "on other");
        s.git(&["switch", "-q", "main"]);
        // The person's own stash, older.
        s.write("b.txt", "mine\n");
        s.git(&["add", "b.txt"]);
        s.git(&["stash", "push", "-q", "-m", "keep me"]);
        // A change git won't stash: assume-unchanged, yet modified.
        s.git(&["update-index", "--assume-unchanged", "a.txt"]);
        s.write("a.txt", "local\n");
        let v = checkout(&s.repo, "other", true, false).unwrap();
        assert_eq!(v["blocked"], true, "{v}");
        assert!(s.git(&["stash", "list"]).contains("keep me"), "the person's stash was popped");
        assert!(!std::path::Path::new(&format!("{}/b.txt", s.root())).exists());
    }
}
