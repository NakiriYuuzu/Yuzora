//! Trusted folders. Focusing a folder makes the sidebar read its status on
//! its own, and auto fetch runs unasked. A repository's own config
//! (`.git/config`, what it includes, the worktree's) can name programs those
//! would run: a clean filter during status, an ssh command or credential
//! helper during fetch. Until the person trusts the folder, nothing runs on
//! its own there (IntelliJ's Safe mode and VS Code's Restricted Mode stop
//! Git the same way), and every other call on the repo is refused. The
//! person's own (global, system) config is theirs and needs no trust.
//! Reads override some of these anyway (fsmonitor, signature programs,
//! textconv), but writes would run them, so they count. Editors don't (every
//! call sets GIT_EDITOR; the interactive rebase, the one command that opens
//! the sequence editor, sets GIT_SEQUENCE_EDITOR), nor does a pager
//! (output never goes to a terminal), nor hooks: status runs none, auto
//! fetch runs with them off, and husky and the like set `core.hooksPath` in
//! many repos for the commits the person makes.
//!
//! The check and the trusted list are `yuzora_core::git_trust`, shared with
//! the status bar and the file tree; this keeps the list under
//! [`crate::settings::own_dir`] and runs git through the engine.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use yuzora_core::git_trust;

use crate::exec::{Git, GitError, GitResult};
use crate::repo::Repo;

/// An empty folder of ours for `core.hooksPath` when hooks must not run.
pub fn no_hooks_dir() -> String {
    let dir = crate::settings::own_dir().join("git").join("no-hooks");
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

/// The program keys the repository's own config sets, and its submodules'
/// (status looks inside them, with their configs), as `path: key`.
pub fn repo_programs(repo: &Repo) -> GitResult<Vec<String>> {
    within(repo, crate::exec::READ_TIMEOUT)
}

/// [`repo_programs`] within `timeout`.
fn within(repo: &Repo, timeout: std::time::Duration) -> GitResult<Vec<String>> {
    git_trust::repo_programs(|args| match repo.run(Git::read(args.iter().copied()).max_output(git_trust::ANSWER_CAP + 1).timeout(timeout)) {
        Ok(out) => Ok(git_trust::whole(out.code, out.stdout)),
        // Out of time: unreadable, which the person may still trust.
        Err(GitError::TimedOut { .. }) => Ok((-1, Vec::new())),
        Err(e) => Err(e),
    })
}

/// `trusted.json` (`git_trust::trusted_file` outside tests).
fn file() -> PathBuf {
    crate::settings::own_dir().join("git").join("trusted.json")
}

fn trusted() -> BTreeSet<String> {
    git_trust::trusted_from(&file())
}

/// What keeps git from running on its own in this repo: its program keys,
/// unless the folder is trusted (empty: nothing does).
pub fn pending(repo: &Repo) -> GitResult<Vec<String>> {
    if trusted().contains(&repo.id) { Ok(Vec::new()) } else { repo_programs(repo) }
}

/// Every other call on the repo goes through here first: while the folder
/// waits for trust, the panes (log, diff, merge...) stay off as the sidebar
/// does, so no filter or other program of the repo's runs through them.
pub fn check(repo: &Repo) -> GitResult<()> {
    let pending = pending(repo)?;
    if pending.is_empty() {
        return Ok(());
    }
    let keys = pending.join(", ");
    // In the shown language (the sidebar's strings); tests keep to English
    // and away from the app home.
    let text = if cfg!(test) { String::new() } else { yuzora_core::i18n::translate("yuzora-sidebar-git", "trust.refused", &[("keys", keys.clone())]) };
    Err(GitError::Other(if text.is_empty() || text == "trust.refused" { format!("{UNTRUSTED} ({keys})") } else { text }))
}

/// The start of [`check`]'s error, for the panes to recognise.
pub const UNTRUSTED: &str = "untrusted folder: trust it in the Git sidebar first";

/// Trusts the repo's folder (here or on its machine) from now on.
pub fn trust(repo: &Repo) -> GitResult<Value> {
    git_trust::trust_at(&file(), &repo.id).map_err(GitError::other)?;
    repo.invalidate(crate::status::Scope::All);
    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;
    use yuzora_core::git_trust::{UNREADABLE, names_program};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_repo_whose_config_runs_programs_waits_for_trust() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        assert!(pending(&s.repo).unwrap().is_empty(), "a plain repo needs no trust");
        assert!(check(&s.repo).is_ok());
        s.git(&["config", "core.alternateRefsCommand", "true"]);
        assert_eq!(pending(&s.repo).unwrap(), vec!["core.alternaterefscommand".to_string()]);
        s.git(&["config", "--unset", "core.alternateRefsCommand"]);
        // Git's own fsmonitor daemon and a plain update mode run nothing of the repo's.
        s.git(&["config", "core.fsmonitor", "true"]);
        s.git(&["config", "submodule.x.update", "rebase"]);
        assert!(pending(&s.repo).unwrap().is_empty());
        s.git(&["config", "core.fsmonitor", "/tmp/hook"]);
        s.git(&["config", "submodule.x.update", "!touch /tmp/x"]);
        s.git(&["config", "merge.lock.driver", "true"]);
        // What git would read as a hook path, though it looks like a boolean.
        assert!(names_program("core.fsmonitor", "true\u{a0}") && names_program("core.fsmonitor", " true") && names_program("core.fsmonitor", "2"));
        assert!(!names_program("core.fsmonitor", "TRUE") && names_program("submodule.x.update", " rebase"));
        assert_eq!(pending(&s.repo).unwrap(), vec!["core.fsmonitor".to_string(), "merge.lock.driver".into(), "submodule.x.update".into()]);
        for k in ["core.fsmonitor", "submodule.x.update", "merge.lock.driver"] {
            s.git(&["config", "--unset", k]);
        }
        // A clean filter in the repo's own config, used by its attributes.
        let marker = format!("{}/ran", s.dir.path().display());
        let program = format!("{}/clean.sh", s.dir.path().display());
        std::fs::write(&program, format!("#!/bin/sh\ntouch '{marker}'\ncat\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["config", "filter.evil.clean", &program]);
        std::fs::write(s.repo.git_path("info/attributes"), "*.txt filter=evil\n").unwrap();
        s.write("a.txt", "2\n");
        assert_eq!(pending(&s.repo).unwrap(), vec!["filter.evil.clean".to_string()]);
        // Every other call refuses until then, not just status.
        assert!(check(&s.repo).unwrap_err().to_string().contains(UNTRUSTED));
        assert!(crate::ops::fetch(&s.repo, &serde_json::json!({ "auto": true }), None).is_err(), "auto fetch waits for trust");
        let v = crate::status::read(&s.repo, false).unwrap();
        assert_eq!(v["trust"], serde_json::json!(["filter.evil.clean"]));
        assert!(!std::path::Path::new(&marker).exists(), "status ran the repo's filter before trust");
        trust(&s.repo).unwrap();
        assert!(pending(&s.repo).unwrap().is_empty());
        let v = crate::status::read(&s.repo, false).unwrap();
        assert!(v.get("trust").is_none());
        assert_eq!(v["counts"]["unstaged"], 1);
        assert!(std::path::Path::new(&marker).exists(), "trusted: git runs the filter as it would in a terminal");
    }

    #[test]
    fn a_check_out_of_time_still_lets_the_folder_be_trusted() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.write("sub/f", "x\n");
        s.git(&["-C", "sub", "init", "-q"]);
        s.git(&["-C", "sub", "add", "f"]);
        s.git(&["-C", "sub", "-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "-qm", "s"]);
        let head = s.git(&["-C", "sub", "rev-parse", "HEAD"]);
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{},sub", head.trim())]);
        // A submodule config git waits on forever.
        let config = format!("{}/sub/.git/config", s.root());
        std::fs::remove_file(&config).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(&config).status().unwrap().success());
        // Pending, not an error: the sidebar shows what waits for trust only then.
        assert_eq!(within(&s.repo, std::time::Duration::from_secs(2)).map_err(|e| e.to_string()), Ok(vec![UNREADABLE.to_string()]));
    }

    #[test]
    fn auto_fetch_runs_no_hooks_but_a_fetch_the_person_starts_does() {
        let origin = scratch();
        origin.commit("a.txt", "1\n", "first");
        let s = scratch();
        s.git(&["remote", "add", "origin", origin.root()]);
        let marker = format!("{}/ran", s.dir.path().display());
        let hook = s.repo.git_path("hooks/reference-transaction");
        std::fs::create_dir_all(std::path::Path::new(&hook).parent().unwrap()).unwrap();
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{marker}'\ncat >/dev/null\n")).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let v = crate::ops::fetch(&s.repo, &serde_json::json!({ "auto": true }), None).unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert!(!std::path::Path::new(&marker).exists(), "auto fetch ran a hook");
        origin.commit("a.txt", "2\n", "second");
        crate::ops::fetch(&s.repo, &serde_json::json!({}), None).unwrap();
        assert!(std::path::Path::new(&marker).exists(), "the person's fetch runs hooks as git does");
    }

    #[test]
    fn an_unreadable_config_keeps_the_gate_shut() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let config = s.repo.git_path("config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[broken\n");
        std::fs::write(&config, text).unwrap();
        let keys = pending(&s.repo).unwrap_or_else(|_| vec![UNREADABLE.into()]);
        assert_eq!(keys, vec![UNREADABLE.to_string()]);
    }

    #[test]
    fn a_submodules_own_config_counts_too() {
        let lib = scratch();
        lib.commit("f.txt", "1\n", "lib");
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "vendor/lib"]);
        s.git(&["commit", "-qm", "sub"]);
        assert!(pending(&s.repo).unwrap().is_empty());
        let sub = crate::repo::open(&format!("{}/vendor/lib", s.root()), None).unwrap();
        sub.run(Git::write(["config", "filter.evil.clean", "cat"])).unwrap();
        assert_eq!(pending(&s.repo).unwrap(), vec!["vendor/lib: filter.evil.clean".to_string()]);
        // A submodule config git can't read keeps the gate shut too.
        let config = format!("{}/config", sub.git_dir);
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[broken\n");
        std::fs::write(&config, text).unwrap();
        let keys = pending(&s.repo).unwrap_or_else(|_| vec![UNREADABLE.into()]);
        assert!(keys.iter().any(|k| k.contains(UNREADABLE)), "{keys:?}");
    }

    #[test]
    fn a_checked_out_gitlink_counts_without_gitmodules() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        // A repo inside, linked in the index only: no `submodule add`, no .gitmodules.
        s.git(&["init", "-q", "sub"]);
        s.write("sub/f", "x\n");
        s.git(&["-C", "sub", "add", "f"]);
        s.git(&["-C", "sub", "-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "-qm", "s"]);
        let head = s.git(&["-C", "sub", "rev-parse", "HEAD"]).trim().to_owned();
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{head},sub")]);
        assert!(pending(&s.repo).unwrap().is_empty());
        // Status above looks into it, with its config: its filter counts.
        s.git(&["-C", "sub", "config", "filter.evil.clean", "cat"]);
        assert_eq!(pending(&s.repo).unwrap(), vec!["sub: filter.evil.clean".to_string()]);
        // The same on a machine (through `hc exec`).
        assert_eq!(pending(&crate::repo::testing::on_machine(&s)).unwrap(), vec!["sub: filter.evil.clean".to_string()]);
    }
}
