//! Trusted folders. Focusing a folder makes the sidebar read its status on
//! its own, and auto fetch runs unasked. A repository's own config
//! (`.git/config`, what it includes, the worktree's) can name programs those
//! would run: a clean filter during status, an ssh command or credential
//! helper during fetch. Until the person trusts the folder, nothing runs on
//! its own there (IntelliJ's Safe mode and VS Code's Restricted Mode stop
//! Git the same way). The person's own (global, system) config is theirs and
//! needs no trust. What reads override anyway (fsmonitor, signature
//! programs, textconv) doesn't count, and neither do hooks: status runs
//! none, and auto fetch runs with them off (husky and the like set
//! `core.hooksPath` in many repos; they run when the person commits).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

use crate::exec::{Git, GitError, GitResult};
use crate::repo::Repo;

/// Config keys (lowercased by git) that name a program run without asking.
const PROGRAMS: &str = r"^(filter\..+\.(clean|smudge|process)|core\.(sshcommand|gitproxy|askpass)|credential(\..+)?\.helper|remote\..+\.(uploadpack|receivepack|vcs)|protocol(\..+)?\.allow)$";

/// Stands for the keys when the config couldn't be read: the gate stays shut.
const UNREADABLE: &str = "(unreadable config)";

/// The program keys the repository's own config sets.
pub fn repo_programs(repo: &Repo) -> GitResult<Vec<String>> {
    let out = repo.run(Git::read(["config", "--show-scope", "-z", "--name-only", "--get-regexp", PROGRAMS]))?;
    if !out.ok() {
        // 1 with nothing printed: no key matched. Anything else: unknown.
        return Ok(if out.code == 1 && out.stdout.is_empty() { Vec::new() } else { vec![UNREADABLE.into()] });
    }
    let out = out.stdout;
    // `scope\0key\0` per entry; an include counts as the file that includes it.
    let text = String::from_utf8_lossy(&out);
    let parts: Vec<&str> = text.split('\0').collect();
    let keys: BTreeSet<String> = parts.chunks(2).filter(|c| c.len() == 2 && matches!(c[0], "local" | "worktree")).map(|c| c[1].to_owned()).collect();
    Ok(keys.into_iter().collect())
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
        // A clean filter in the repo's own config, used by its attributes.
        let marker = format!("{}/ran", s.dir.path().display());
        let program = format!("{}/clean.sh", s.dir.path().display());
        std::fs::write(&program, format!("#!/bin/sh\ntouch '{marker}'\ncat\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["config", "filter.evil.clean", &program]);
        std::fs::write(s.repo.git_path("info/attributes"), "*.txt filter=evil\n").unwrap();
        s.write("a.txt", "2\n");
        assert_eq!(pending(&s.repo).unwrap(), vec!["filter.evil.clean".to_string()]);
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
}
