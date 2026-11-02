//! Trusted folders. Focusing a folder makes the sidebar read its status on
//! its own, and auto fetch runs unasked. A repository's own config
//! (`.git/config`, what it includes, the worktree's) can name programs those
//! would run: a clean filter during status, an ssh command or credential
//! helper during fetch. Until the person trusts the folder, nothing runs on
//! its own there (IntelliJ's Safe mode and VS Code's Restricted Mode stop
//! Git the same way), and every other call on the repo is refused. The
//! person's own (global, system) config is theirs and needs no trust.
//! Reads override some of these anyway (fsmonitor, signature programs,
//! textconv), but writes would run them, so they count. Editors don't (the
//! engine always sets GIT_EDITOR and GIT_SEQUENCE_EDITOR), nor does a pager
//! (output never goes to a terminal), nor hooks: status runs none, auto
//! fetch runs with them off, and husky and the like set `core.hooksPath` in
//! many repos for the commits the person makes.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

use crate::exec::{Git, GitError, GitResult};
use crate::repo::Repo;

/// Config keys (lowercased by git) that name a program run without asking.
const PROGRAMS: &str = r"^(filter\..+\.(clean|smudge|process)|core\.(fsmonitor|sshcommand|gitproxy|askpass|alternaterefscommand|alternaterefsprefixes)|credential(\..+)?\.helper|remote\..+\.(uploadpack|receivepack|vcs)|protocol(\..+)?\.allow|merge\..+\.driver|diff\..+\.(command|textconv)|gpg(\..+)?\.program|gpg\.ssh\.defaultkeycommand|submodule\..+\.update)$";

/// Whether a matched key's value runs something: fsmonitor `true` is git's
/// own daemon, and a submodule's update mode is a command only after `!`.
fn names_program(key: &str, value: &str) -> bool {
    if key == "core.fsmonitor" {
        return !matches!(value.trim().to_ascii_lowercase().as_str(), "" | "true" | "false" | "yes" | "no" | "on" | "off" | "1" | "0");
    }
    if key.starts_with("submodule.") && key.ends_with(".update") {
        return value.trim_start().starts_with('!');
    }
    true
}

/// Stands for the keys when the config couldn't be read: the gate stays shut.
const UNREADABLE: &str = "(unreadable config)";

/// An empty folder of ours for `core.hooksPath` when hooks must not run.
pub fn no_hooks_dir() -> String {
    let dir = crate::settings::own_dir().join("git").join("no-hooks");
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

/// The program keys the repository's own config sets, and its submodules'
/// (status looks inside them, with their configs), as `path: key`.
pub fn repo_programs(repo: &Repo) -> GitResult<Vec<String>> {
    let out = repo.run(Git::read(["config", "--show-scope", "-z", "--get-regexp", PROGRAMS]))?;
    if !out.ok() {
        // 1 with nothing printed: no key matched. Anything else: unknown.
        return Ok(if out.code == 1 && out.stdout.is_empty() { Vec::new() } else { vec![UNREADABLE.into()] });
    }
    let mut keys = repo_keys(&out.stdout, "");
    if repo.exists(&repo.abs(".gitmodules")) {
        // Each checked-out submodule, nested ones too: a `sub` record names it.
        // 1 is "no key"; any other failure is an `err` record (gate shut).
        let script = format!("printf 'sub\\0%s\\0' \"$displaypath\"; git config --show-scope -z --get-regexp '{PROGRAMS}'; c=$?; if [ $c -gt 1 ]; then printf 'err\\0%s\\0' $c; fi");
        let out = repo.run(Git::read(["submodule", "foreach", "--recursive", "--quiet", &script]))?;
        if !out.ok() {
            keys.insert(UNREADABLE.into());
        }
        keys.extend(repo_keys(&out.stdout, "?"));
    }
    Ok(keys.into_iter().collect())
}

/// `scope\0key\nvalue\0` records (an include counts as the file that
/// includes it; a key alone is a boolean): the repo-level keys whose values
/// run something, as `path: key` once a `sub` record named a submodule.
fn repo_keys(out: &[u8], first: &str) -> BTreeSet<String> {
    let text = String::from_utf8_lossy(out);
    let parts: Vec<&str> = text.split('\0').collect();
    let mut place = first.to_owned();
    let mut keys = BTreeSet::new();
    for c in parts.chunks(2).filter(|c| c.len() == 2) {
        match c[0] {
            "sub" => place = c[1].to_owned(),
            "err" => {
                keys.insert(format!("{place}: {UNREADABLE}"));
            }
            "local" | "worktree" => {
                let (key, value) = c[1].split_once('\n').unwrap_or((c[1], "true"));
                if names_program(key, value) {
                    keys.insert(if place.is_empty() { key.to_owned() } else { format!("{place}: {key}") });
                }
            }
            _ => {}
        }
    }
    keys
}

fn file() -> PathBuf {
    crate::settings::own_dir().join("git").join("trusted.json")
}

fn trusted() -> BTreeSet<String> {
    std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).map(|v| v.into_iter().collect()).unwrap_or_default()
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
    let mut all = trusted();
    all.insert(repo.id.clone());
    let path = file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(GitError::other)?;
    }
    let list: Vec<&String> = all.iter().collect();
    std::fs::write(&path, serde_json::to_string_pretty(&list).unwrap_or_default()).map_err(GitError::other)?;
    repo.invalidate(crate::status::Scope::All);
    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;
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
}
