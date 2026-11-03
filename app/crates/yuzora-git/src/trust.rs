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

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use yuzora_core::git_trust;

use crate::exec::{Git, GitError, GitResult, Output, Place};
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

/// What keeps git from running on its own in this repo, unless the folder
/// is trusted (empty: nothing does): its program keys (a partial clone's
/// too: [`git_trust::names_program`]), and the remote helpers git kept out
/// of a command there ([`heard`]).
pub fn pending(repo: &Repo) -> GitResult<Vec<String>> {
    if trusted_folder(repo) {
        return Ok(Vec::new());
    }
    let mut keys: BTreeSet<String> = repo_programs(repo)?.into_iter().collect();
    keys.extend(kept_out().lock().unwrap_or_else(|e| e.into_inner()).get(&repo.id).into_iter().flatten().cloned());
    Ok(keys.into_iter().collect())
}

/// What git kept out this session in a folder not trusted yet ([`heard`]), by repo.
fn kept_out() -> &'static Mutex<BTreeMap<String, BTreeSet<String>>> {
    static KEPT: OnceLock<Mutex<BTreeMap<String, BTreeSet<String>>>> = OnceLock::new();
    KEPT.get_or_init(Mutex::default)
}

/// git's own transports: anything else is a remote helper.
const OWN_TRANSPORTS: [&str; 5] = ["file", "git", "http", "https", "ssh"];

/// What [`guard`] decided for a command: the folder, and whether a helper
/// git kept out there waits for trust (not for a read that chose to be offline).
pub(crate) struct Guarded {
    id: String,
    listens: bool,
}

/// Every git command the engine starts passes here first (`exec`'s one way
/// to start git, here and through `hc exec`). Until the folder is trusted it
/// runs offline: `GIT_ALLOW_PROTOCOL` empty (no transport, so no remote
/// helper, however a URL names one) and `GIT_NO_LAZY_FETCH=1` (a git before
/// 2.44 ignores it; the empty list stops it there too). A command that chose
/// its own keeps it: [`reach`] lists git's own transports, [`offline`] reads
/// quietly.
pub(crate) fn guard(place: &Place, mut cmd: Git) -> (Git, Option<Guarded>) {
    let id = crate::repo::repo_id(place.on.as_deref(), &place.root);
    if trusted().contains(&id) {
        return (cmd, None);
    }
    let own = |cmd: &Git, k: &str| cmd.env.iter().rev().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    let list = own(&cmd, "GIT_ALLOW_PROTOCOL");
    if list.is_none() {
        cmd = cmd.env("GIT_ALLOW_PROTOCOL", "");
    }
    if own(&cmd, "GIT_NO_LAZY_FETCH").is_none() {
        cmd = cmd.env("GIT_NO_LAZY_FETCH", "1");
    }
    (cmd, Some(Guarded { id, listens: list.as_deref() != Some("") }))
}

/// After a guarded command: a remote helper git kept out (in its words, in
/// English here: `transport '<name>' not allowed`) waits for trust from then
/// on, so the sidebar asks. The scan can't see every URL that names one.
pub(crate) fn heard(guarded: Guarded, out: &Output) {
    if !guarded.listens {
        return;
    }
    let stderr = out.stderr_text();
    let kept = stderr.lines().filter_map(|l| l.split_once("transport '")?.1.strip_suffix("' not allowed")).filter(|t| !OWN_TRANSPORTS.contains(t));
    let kept: BTreeSet<String> = kept.map(|t| format!("transport '{t}'")).collect();
    if !kept.is_empty() {
        kept_out().lock().unwrap_or_else(|e| e.into_inner()).entry(guarded.id).or_default().extend(kept);
    }
}

/// Runs a command that reaches a remote (fetch, pull, push, submodule
/// update). Until the folder is trusted it may use git's own transports the
/// person's config allows ([`transports`]), never a remote helper, whatever
/// names one: the scan can't see them all (a tracked `.gitmodules`, a nested
/// one `--recursive` meets on the way, an unpopulated submodule's git dir,
/// the person's own `insteadOf`). It fetches nothing a partial clone lacks
/// either ([`guard`]).
pub fn reach(repo: &Repo, cmd: Git, from_user: bool) -> GitResult<Output> {
    if trusted_folder(repo) {
        return repo.run(cmd);
    }
    let config = repo.run(Git::read(["config", "--get-regexp", r"^protocol\."]))?.stdout_text();
    repo.run(cmd.env("GIT_ALLOW_PROTOCOL", transports(&config, from_user)))
}

/// Which of git's own transports the person's config allows, as git decides
/// (`protocol.<name>.allow`, else `protocol.allow`, else its defaults:
/// `file` only for a URL from the person, which a submodule's isn't), from
/// `git config --get-regexp '^protocol\.'`; `:`-separated.
fn transports(config: &str, from_user: bool) -> String {
    let get = |key: &str| config.lines().rev().filter_map(|l| l.split_once(' ')).find(|(k, _)| *k == key).map(|(_, v)| v.to_ascii_lowercase());
    let allowed = |t: &str| match get(&format!("protocol.{t}.allow")).or_else(|| get("protocol.allow")).as_deref() {
        Some("always") => true,
        Some("user") => from_user,
        Some(_) => false,
        None => t != "file" || from_user,
    };
    OWN_TRANSPORTS.into_iter().filter(|t| allowed(t)).collect::<Vec<_>>().join(":")
}

/// Whether the person trusted the folder (not only that nothing waits).
pub fn trusted_folder(repo: &Repo) -> bool {
    trusted().contains(&repo.id)
}

/// A read that runs on its own (opening, refreshing, following refs): offline
/// until the folder is trusted, as [`guard`] makes every command, and quiet
/// about a helper git kept out (the person didn't start it).
pub fn own_read(repo: &Repo, cmd: Git) -> Git {
    if trusted_folder(repo) { cmd } else { offline(cmd) }
}

/// A read that never fetches what the repository lacks ([`own_read`]).
pub fn offline(cmd: Git) -> Git {
    cmd.env("GIT_NO_LAZY_FETCH", "1").env("GIT_ALLOW_PROTOCOL", "")
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

    // An unpopulated submodule's git dir (`.git/modules/<name>`) is beyond
    // the scan, yet `fetch` (on demand) and `submodule update` use its
    // remote; a tracked `.gitmodules`, a nested one met on the way, likewise.
    #[test]
    fn an_untrusted_folder_reaches_no_remote_helper() {
        let s = scratch();
        let mark = s.dir.path().join("ran");
        let hit = s.dir.path().join("hit");
        std::fs::write(&hit, format!("#!/bin/sh\ntouch '{}'\nexit 1\n", mark.display())).unwrap();
        std::fs::set_permissions(&hit, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (lib, up) = (scratch(), scratch());
        lib.commit("l.txt", "1\n", "l1");
        up.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "m"]);
        up.git(&["commit", "-q", "-m", "s1"]);
        s.git(&["remote", "add", "origin", up.root()]);
        s.git(&["fetch", "-q", "origin"]);
        s.git(&["reset", "-q", "--hard", "origin/main"]);
        s.git(&["-c", "protocol.file.allow=always", "submodule", "update", "-q", "--init"]);
        s.git(&["submodule", "deinit", "-q", "-f", "m"]);
        // Upstream moves the submodule on; its git dir here names a helper, and allows it.
        lib.commit("l.txt", "2\n", "l2");
        up.git(&["-C", "m", "pull", "-q"]);
        up.git(&["commit", "-q", "-am", "s2"]);
        let config = s.repo.git_path("modules/m/config");
        s.git(&["config", "--file", &config, "remote.origin.url", &format!("ext::{}", hit.display())]);
        s.git(&["config", "--file", &config, "protocol.ext.allow", "always"]);
        let named = [".git/modules/m: protocol.ext.allow", ".git/modules/m: remote.origin.url"];
        assert_eq!(pending(&s.repo).unwrap(), named, "the scan reads the git dir of a submodule nothing checks out");
        crate::ops::fetch(&s.repo, &serde_json::json!({}), None).unwrap();
        assert!(!mark.exists(), "fetch ran the helper an unpopulated submodule names before the folder was trusted");
        // Past the gate (which shuts here): git's own transports only, so git
        // keeps the helper out however the git dir allows it, and says so.
        s.git(&["reset", "-q", "--hard", "origin/main"]);
        crate::ops::submodule_update(&s.repo, None).unwrap();
        assert!(!mark.exists(), "submodule update ran it before the folder was trusted");
        // The update put a `.git` in `m`: the same git dir, now by its gitlink's name.
        assert_eq!(pending(&s.repo).unwrap(), ["m: protocol.ext.allow", "m: remote.origin.url", "transport 'ext'"], "what git kept out waits for trust");
        trust(&s.repo).unwrap();
        crate::ops::submodule_update(&s.repo, None).unwrap();
        assert!(mark.exists(), "trusted, git didn't run it either: this proves nothing");
    }

    // The engine starts git one way (`exec::run_streaming`), here and through
    // `hc exec`: until the folder is trusted every command there runs
    // offline, whatever its caller built.
    #[test]
    fn every_git_command_in_an_untrusted_folder_runs_offline() {
        let s = scratch();
        s.commit("a.txt", "1\n", "one");
        let lines = |out: Output| {
            let mut v: Vec<String> = out.stdout_text().lines().filter(|l| l.starts_with("GIT_ALLOW_PROTOCOL=") || l.starts_with("GIT_NO_LAZY_FETCH=")).map(str::to_owned).collect();
            v.sort();
            v
        };
        let env = ["-c", "alias.yz-env=!env", "yz-env"];
        let offline = ["GIT_ALLOW_PROTOCOL=", "GIT_NO_LAZY_FETCH=1"];
        let there = crate::repo::testing::on_machine(&s);
        for repo in [&s.repo, &there] {
            assert_eq!(lines(repo.run(Git::read(env)).unwrap()), offline, "{}", repo.id);
            assert_eq!(lines(repo.run(Git::write(env)).unwrap()), offline, "{}", repo.id);
        }
        // A command that reaches a remote: git's own transports the person allows.
        let config = s.repo.run(Git::read(["config", "--get-regexp", r"^protocol\."])).unwrap().stdout_text();
        let listed = transports(&config, true);
        assert!(!listed.is_empty() && !listed.contains("ext"), "{listed}");
        assert_eq!(lines(reach(&s.repo, Git::read(env), true).unwrap()), [format!("GIT_ALLOW_PROTOCOL={listed}"), "GIT_NO_LAZY_FETCH=1".into()]);
        trust(&s.repo).unwrap();
        assert_eq!(lines(s.repo.run(Git::read(env)).unwrap()), Vec::<String>::new(), "trusted: as git in a terminal");
        assert_eq!(lines(there.run(Git::read(env)).unwrap()), offline, "another machine's folder of the same path is another folder");
    }

    // A URL whose scheme git doesn't serve itself (`zz://`, `ext://`) names a
    // remote helper too, and a partial clone fetches what it lacks through
    // it. Before trust a commit's details and its diff start none, even
    // called past the gate.
    #[test]
    fn an_untrusted_partial_clone_starts_no_helper_for_details_or_a_diff() {
        for url in ["zz://payload", "ext://x/run"] {
            let s = scratch();
            let mark = s.dir.path().join("ran");
            let bin = s.dir.path().join("bin");
            // A helper the person installed; `ext://x/run` runs `ext:/x/run`, which the repo can hold.
            for at in [bin.join("git-remote-zz"), std::path::Path::new(s.root()).join("ext:/x/run")] {
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(&at, format!("#!/bin/sh\ntouch '{}'\nexit 1\n", mark.display())).unwrap();
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
            crate::exec::TEST_ENV.with(|t| *t.borrow_mut() = vec![("PATH".into(), path)]);
            let one = s.commit("a.txt", "1\n", "one");
            let two = s.commit("a.txt", "2\n", "two");
            // `protocol.ext.allow` stands for the person's global config.
            for (k, v) in [("remote.origin.url", url), ("remote.origin.promisor", "true"), ("protocol.ext.allow", "always")] {
                s.git(&["config", k, v]);
            }
            std::fs::remove_file(s.repo.git_path(&format!("objects/{}/{}", &one[..2], &one[2..]))).unwrap();
            // The gate is shut from the start (a partial clone waits); these are called past it.
            assert!(pending(&s.repo).unwrap().contains(&"remote.origin.promisor".to_string()), "{url}: {:?}", pending(&s.repo));
            let diff = || _ = crate::diffview::DiffSession::load(s.repo.clone(), "a.txt".into(), None, crate::diffview::Mode::Commit(two.clone()));
            let details = crate::log::details(&s.repo, &one);
            assert!(!mark.exists(), "{url}: a commit's details started a helper before the folder was trusted");
            assert!(details.is_err(), "{url}: {details:?}");
            diff();
            assert!(!mark.exists(), "{url}: a diff started a helper before the folder was trusted");
            trust(&s.repo).unwrap();
            let _ = crate::log::details(&s.repo, &one);
            let details_ran = mark.exists();
            let _ = std::fs::remove_file(&mark);
            diff();
            crate::exec::TEST_ENV.with(|t| t.borrow_mut().clear());
            assert!(details_ran && mark.exists(), "{url}: trusted, git didn't start the helper either ({details_ran}): this proves nothing");
        }
    }

    /// A program for a test that notes it ran in `mark`; `tail` ends it.
    fn hit(s: &crate::repo::testing::Scratch, tail: &str) -> (std::path::PathBuf, String) {
        let mark = s.dir.path().join("ran");
        let prog = s.dir.path().join("hit.sh");
        std::fs::write(&prog, format!("#!/bin/sh\necho \"$*\" >> '{}'\n{tail}\n", mark.display())).unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        (mark, prog.display().to_string())
    }

    // A submodule not checked out keeps its git dir, and `submodule update`
    // reuses it: a smudge filter for its checkout, an ssh command or git
    // proxy for the commit it lacks. They wait for trust like the top's.
    #[test]
    fn an_unpopulated_submodules_programs_wait_for_trust() {
        for (variant, key, url) in [("smudge", "filter.evil.smudge", "ssh://example.invalid/s"), ("sshcommand", "core.sshCommand", "ssh://example.invalid/s"), ("gitproxy", "core.gitProxy", "git://example.invalid/s")] {
            let (s, lib) = (scratch(), scratch());
            lib.write(".gitattributes", "*.txt filter=evil\n");
            lib.git(&["add", ".gitattributes"]);
            lib.commit("f.txt", "x\n", "l");
            s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "s"]);
            s.git(&["commit", "-q", "-m", "sub"]);
            s.git(&["submodule", "deinit", "-q", "-f", "s"]);
            s.git(&["config", "-f", ".gitmodules", "submodule.s.url", url]);
            s.git(&["commit", "-q", "-am", "url"]);
            let (mark, prog) = hit(&s, if variant == "smudge" { "cat" } else { "exit 1" });
            let config = s.repo.git_path("modules/s/config");
            if variant != "smudge" {
                // A commit the git dir lacks: update fetches it there.
                s.git(&["update-index", "--cacheinfo", "160000,1234567890123456789012345678901234567890,s"]);
                s.git(&["commit", "-q", "-m", "bump"]);
                s.git(&["config", "--file", &config, "remote.origin.url", url]);
            }
            s.git(&["config", "--file", &config, key, &prog]);
            let pending = pending(&s.repo).unwrap();
            assert_eq!(pending, [format!(".git/modules/s: {}", key.to_lowercase())], "{variant}");
            assert!(check(&s.repo).is_err(), "{variant}");
            assert!(!mark.exists(), "{variant}");
            // Trusted, update runs it: the setup proves something.
            trust(&s.repo).unwrap();
            let _ = crate::ops::submodule_update(&s.repo, None);
            assert!(mark.exists(), "{variant}: trusted, submodule update didn't run it: this proves nothing");
        }
    }

    // `submodule update` reuses whatever is at `.git/modules/<name>`, and git
    // takes it for a git dir as it opens it: `HEAD` spelled `head` (where
    // case doesn't count), only `HEAD` and a `commondir` naming where the
    // rest is, `HEAD` a link to a ref that's only packed. Each waits for
    // trust like a plain one; where git can't open it, nothing runs either.
    #[test]
    fn a_submodule_git_dir_hidden_from_a_listing_waits_for_trust() {
        for variant in ["lowerhead", "commondir", "symhead"] {
            let (s, lib) = (scratch(), scratch());
            lib.write(".gitattributes", "*.txt filter=evil\n");
            lib.git(&["add", ".gitattributes"]);
            lib.commit("f.txt", "x\n", "l");
            s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "s"]);
            s.git(&["commit", "-q", "-m", "sub"]);
            s.git(&["submodule", "deinit", "-q", "-f", "s"]);
            s.git(&["config", "-f", ".gitmodules", "submodule.s.url", "ssh://example.invalid/s"]);
            s.git(&["commit", "-q", "-am", "url"]);
            let gd = std::path::PathBuf::from(s.repo.git_path("modules/s"));
            let mut config = gd.join("config");
            match variant {
                "lowerhead" => std::fs::rename(gd.join("HEAD"), gd.join("head")).unwrap(),
                "commondir" => {
                    let common = std::path::PathBuf::from(s.repo.git_path("evil-common"));
                    std::fs::rename(&gd, &common).unwrap();
                    std::fs::create_dir(&gd).unwrap();
                    std::fs::copy(common.join("HEAD"), gd.join("HEAD")).unwrap();
                    std::fs::write(gd.join("commondir"), "../../evil-common\n").unwrap();
                    config = common.join("config");
                }
                _ => {
                    let at = gd.display().to_string();
                    let head = s.git(&["--git-dir", &at, "rev-parse", "HEAD"]);
                    s.git(&["--git-dir", &at, "update-ref", "refs/heads/keep", head.trim()]);
                    s.git(&["--git-dir", &at, "pack-refs", "--all"]);
                    std::fs::remove_file(gd.join("HEAD")).unwrap();
                    std::os::unix::fs::symlink("refs/heads/keep", gd.join("HEAD")).unwrap();
                }
            }
            let (mark, prog) = hit(&s, "cat");
            s.git(&["config", "--file", &config.display().to_string(), "filter.evil.smudge", &prog]);
            // Where case counts, git can't open `head` as `HEAD`.
            let opens = gd.join("HEAD").exists() || gd.join("HEAD").is_symlink();
            let pending = pending(&s.repo).unwrap();
            if opens {
                assert_eq!(pending, [".git/modules/s: filter.evil.smudge"], "{variant}");
                assert!(check(&s.repo).is_err(), "{variant}");
            } else {
                assert_eq!(pending, Vec::<String>::new(), "{variant}");
            }
            assert!(!mark.exists(), "{variant}");
            trust(&s.repo).unwrap();
            let _ = crate::ops::submodule_update(&s.repo, None);
            assert_eq!(mark.exists(), opens, "{variant}: trusted, submodule update ran it as git opens it: this proves the scan reads what git does");
        }
    }

    // A linked worktree keeps its submodules' git dirs in its own git dir
    // (`.git/worktrees/<id>/modules`), and its `submodule update` reuses them.
    #[test]
    fn a_linked_worktree_s_unpopulated_submodule_waits_for_trust() {
        let (s, lib) = (scratch(), scratch());
        lib.write(".gitattributes", "*.txt filter=evil\n");
        lib.git(&["add", ".gitattributes"]);
        lib.commit("f.txt", "x\n", "l");
        s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "s"]);
        s.git(&["commit", "-q", "-m", "sub"]);
        let away = tempfile::tempdir().unwrap();
        let wt = std::fs::canonicalize(away.path()).unwrap().join("wt").display().to_string();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "protocol.file.allow=always", "-C", &wt]).args(args).output().unwrap().status.success(), "{args:?}");
        s.git(&["worktree", "add", "-q", &wt]);
        git(&["submodule", "update", "-q", "--init"]);
        git(&["submodule", "deinit", "-q", "-f", "s"]);
        let (mark, prog) = hit(&s, "cat");
        s.git(&["config", "--file", &format!("{}/.git/worktrees/wt/modules/s/config", s.root()), "filter.evil.smudge", &prog]);
        let w = crate::repo::open(&wt, None).unwrap();
        assert_eq!(pending(&w).unwrap(), [".git/worktrees/wt/modules/s: filter.evil.smudge"]);
        assert!(check(&w).is_err());
        assert_eq!(pending(&s.repo).unwrap(), Vec::<String>::new(), "the main worktree never uses it");
        trust(&w).unwrap();
        let _ = crate::ops::submodule_update(&w, None);
        assert!(mark.exists(), "trusted, submodule update didn't run it: this proves nothing");
    }

    // The person's fetch recurses on demand into a submodule whose gitlink
    // the fetched commits move, checked out or not. Before trust it fetches
    // the folder's own repo only: no submodule's ssh command runs.
    #[test]
    fn a_fetch_before_trust_leaves_submodules_alone() {
        let (s, lib) = (scratch(), scratch());
        lib.commit("f.txt", "x\n", "l");
        s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "s"]);
        s.git(&["config", "-f", ".gitmodules", "submodule.s.url", "ssh://example.invalid/s"]);
        s.git(&["config", "submodule.s.url", "ssh://example.invalid/s"]);
        s.git(&["commit", "-q", "-am", "sub"]);
        // Upstream moves the gitlink on.
        let up = s.dir.path().join("up.git");
        let work = s.dir.path().join("work");
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["clone", "-q", "--bare", "--no-local", s.root(), &up.display().to_string()]);
        git(&["clone", "-q", "--no-local", &up.display().to_string(), &work.display().to_string()]);
        let w = work.display().to_string();
        git(&["-C", &w, "update-index", "--cacheinfo", "160000,1234567890123456789012345678901234567890,s"]);
        git(&["-C", &w, "commit", "-q", "-m", "bump"]);
        git(&["-C", &w, "push", "-q", "origin", "HEAD"]);
        s.git(&["remote", "add", "origin", &format!("file://{}", up.display())]);
        // Not checked out: its work tree emptied, its git dir kept.
        std::fs::remove_dir_all(format!("{}/s", s.root())).unwrap();
        std::fs::create_dir_all(format!("{}/s", s.root())).unwrap();
        let (mark, prog) = hit(&s, "exit 1");
        let config = s.repo.git_path("modules/s/config");
        s.git(&["config", "--file", &config, "remote.origin.url", "ssh://example.invalid/s"]);
        s.git(&["config", "--file", &config, "core.sshCommand", &prog]);
        // Past the gate (which shuts here), as a fetch the person starts.
        crate::ops::fetch(&s.repo, &serde_json::json!({}), None).unwrap();
        assert!(!mark.exists(), "fetch ran a submodule's ssh command before the folder was trusted");
        assert!(!s.git(&["rev-parse", "origin/main"]).is_empty(), "the folder's own repo was fetched");
        // Pull fetches the same way. Then the commit is new to the next fetch again.
        s.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
        let head = s.git(&["rev-parse", "HEAD"]);
        let _ = crate::ops::pull(&s.repo, &serde_json::json!({"remote": "origin", "branch": "main", "method": "merge"}), None);
        assert!(!mark.exists(), "pull ran a submodule's ssh command before the folder was trusted");
        assert!(!s.git(&["rev-parse", "origin/main"]).is_empty(), "pull fetched the folder's own repo");
        s.git(&["reset", "-q", "--hard", head.trim()]);
        s.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
        trust(&s.repo).unwrap();
        let _ = crate::ops::fetch(&s.repo, &serde_json::json!({}), None);
        assert!(mark.exists(), "trusted, fetch didn't run it either: this proves nothing");
    }

    // A git before 2.44 ignores GIT_NO_LAZY_FETCH: this one drops it from
    // every git it starts, and from the ones those start (`GIT_EXEC_PATH`).
    fn old_git(at: &std::path::Path) -> Vec<(String, String)> {
        let real = std::process::Command::new("git").arg("--exec-path").output().unwrap();
        let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
        std::fs::create_dir_all(at).unwrap();
        for e in std::fs::read_dir(&real).unwrap().map(Result::unwrap).filter(|e| e.file_name() != "git") {
            std::os::unix::fs::symlink(e.path(), at.join(e.file_name())).unwrap();
        }
        std::fs::write(at.join("git"), format!("#!/bin/sh\nunset GIT_NO_LAZY_FETCH\nexec '{real}/git' \"$@\"\n")).unwrap();
        std::fs::set_permissions(at.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
        vec![("PATH".into(), format!("{}:{}", at.display(), std::env::var("PATH").unwrap_or_default())), ("GIT_EXEC_PATH".into(), at.display().to_string())]
    }

    // `.git/modules/<name>` may be a gitfile, here naming a partial clone
    // that lacks the commit a push carries: a push that checks submodules
    // (`push.recurseSubmodules`) opens it in its own process, and a git
    // before 2.44 fetches the commit there with that repo's `uploadpack`.
    // The scan reads the git dir the gitfile names; before trust a push
    // checks no submodule either.
    #[test]
    fn a_push_before_trust_opens_no_submodule_git_dir() {
        let (s, lib) = (scratch(), scratch());
        let away_dir = tempfile::tempdir().unwrap();
        let away = std::fs::canonicalize(away_dir.path()).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").args(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false"]).args(args).output().unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let (src, up) = (away.join("src.git").display().to_string(), away.join("up.git").display().to_string());
        lib.commit("f.txt", "1\n", "one");
        git(&["clone", "-q", "--bare", lib.root(), &src]);
        git(&["init", "-q", "--bare", &up]);
        s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "s"]);
        s.git(&["commit", "-q", "-m", "sub"]);
        s.git(&["remote", "add", "origin", &up]);
        s.git(&["push", "-q", "origin", "main"]);
        let sub = s.git(&["rev-parse", "HEAD"]);
        s.git(&["submodule", "deinit", "-q", "-f", "s"]);
        let (gd, evil) = (s.repo.git_path("modules/s"), s.repo.git_path("evil.git"));
        std::fs::rename(&gd, &evil).unwrap();
        std::fs::write(&gd, "gitdir: ../evil.git\n").unwrap();
        let (mark, prog) = hit(&s, "exec git-upload-pack \"$@\"");
        for (k, v) in [("core.worktree", "../../s"), ("remote.origin.url", &src), ("remote.origin.promisor", "true"), ("extensions.partialClone", "origin"), ("remote.origin.uploadpack", &prog)] {
            s.git(&["--git-dir", &evil, "config", k, v]);
        }
        // A commit the partial clone lacks; the push carries the gitlink moved to it.
        lib.commit("f.txt", "2\n", "two");
        git(&["-C", lib.root(), "push", "-q", &src, "HEAD:main"]);
        let two = lib.git(&["rev-parse", "HEAD"]);
        s.git(&["update-index", "--cacheinfo", &format!("160000,{},s", two.trim())]);
        s.git(&["commit", "-q", "-m", "bump"]);
        s.git(&["config", "push.recurseSubmodules", "check"]);
        assert_eq!(pending(&s.repo).unwrap(), [".git/modules/s: extensions.partialclone", ".git/modules/s: remote.origin.promisor", ".git/modules/s: remote.origin.uploadpack"]);
        assert!(check(&s.repo).is_err());
        // Past the gate (which shuts here), with a git that fetches lazily anyway.
        let env = old_git(&away.join("old-git"));
        crate::exec::TEST_ENV.with(|t| *t.borrow_mut() = env);
        let _ = crate::ops::push(&s.repo, &serde_json::json!({}), None);
        let before = mark.exists();
        // The same push again, trusted: it opens s and fetches there.
        git(&["--git-dir", &up, "update-ref", "refs/heads/main", sub.trim()]);
        s.git(&["update-ref", "refs/remotes/origin/main", sub.trim()]);
        trust(&s.repo).unwrap();
        let _ = crate::ops::push(&s.repo, &serde_json::json!({}), None);
        crate::exec::TEST_ENV.with(|t| t.borrow_mut().clear());
        assert!(!before, "a push before trust ran the uploadpack of a submodule's git dir");
        assert!(mark.exists(), "trusted, the push didn't run it either: this proves nothing");
    }

    #[test]
    fn a_partial_clone_waits_for_trust_by_the_keys_git_reads() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        assert_eq!(pending(&s.repo).unwrap(), Vec::<String>::new());
        s.git(&["config", "remote.origin.promisor", "false"]);
        assert_eq!(pending(&s.repo).unwrap(), Vec::<String>::new());
        s.git(&["config", "remote.origin.promisor", "yes"]);
        assert_eq!(pending(&s.repo).unwrap(), ["remote.origin.promisor"]);
        s.git(&["config", "--unset", "remote.origin.promisor"]);
        s.git(&["config", "extensions.partialClone", "origin"]);
        assert_eq!(pending(&s.repo).unwrap(), ["extensions.partialclone"]);
        // A filter alone makes a remote a promisor, as in git.
        s.git(&["config", "--unset", "extensions.partialClone"]);
        s.git(&["config", "remote.origin.partialCloneFilter", "blob:none"]);
        assert_eq!(pending(&s.repo).unwrap(), ["remote.origin.partialclonefilter"]);
        trust(&s.repo).unwrap();
        assert_eq!(pending(&s.repo).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn git_s_own_transports_follow_the_person_s_policy() {
        assert_eq!(transports("", true), "file:git:http:https:ssh");
        assert_eq!(transports("", false), "git:http:https:ssh");
        // Per transport first, then `protocol.allow`; the last value wins; values in any case.
        assert_eq!(transports("protocol.allow never\nprotocol.https.allow always", true), "https");
        assert_eq!(transports("protocol.git.allow NEVER\nprotocol.file.allow always", false), "file:http:https:ssh");
        assert_eq!(transports("protocol.ssh.allow always\nprotocol.ssh.allow user", false), "git:http:https");
        assert_eq!(transports("protocol.allow user", false), "");
        // The transport's name is a subsection: its case counts, as in git.
        assert_eq!(transports("protocol.HTTPS.allow never", true), "file:git:http:https:ssh");
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

    #[cfg(unix)]
    #[test]
    fn a_filter_behind_a_work_tree_name_ending_in_a_newline_waits_for_trust() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let id = ["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false"];
        let repo = |at: &str| {
            s.write(&format!("{at}/f"), "x\n");
            s.git(&["-C", at, "init", "-q"]);
            s.git(&["-C", at, "add", "."]);
            s.git(&[&["-C", at][..], &id[..], &["commit", "-qm", "s"][..]].concat());
            s.git(&["-C", at, "rev-parse", "HEAD"]).trim().to_owned()
        };
        let head = repo("s");
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{head},s")]);
        // `s` works in `wt<LF>`, where a repo with a clean filter is; `wt` is a decoy.
        std::fs::create_dir_all(format!("{}/wt", s.root())).unwrap();
        let mark = format!("{}/ran", s.root());
        s.write("wt\n/evil/.gitattributes", "* filter=evil\n");
        let evil = repo("wt\n/evil");
        s.git(&["-C", "wt\n/evil", "config", "filter.evil.clean", &format!("touch '{mark}'; cat")]);
        s.git(&["-C", "s", "update-index", "--add", "--cacheinfo", &format!("160000,{evil},evil")]);
        s.git(&["-C", "s", "config", "core.worktree", "../../wt\n"]);
        std::fs::File::options().write(true).open(format!("{}/wt\n/evil/f", s.root())).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800)).unwrap();
        let v = crate::status::read(&s.repo, false).unwrap();
        assert!(!std::path::Path::new(&mark).exists(), "status ran the nested repo's clean filter before the folder was trusted");
        assert_eq!(v["trust"], serde_json::json!(["s: (unreadable config)"]));
        // git status itself goes there and runs it: otherwise this proves nothing.
        std::process::Command::new("git").args(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--untracked-files=no"]).current_dir(s.root()).output().unwrap();
        assert!(std::path::Path::new(&mark).exists(), "git status didn't run the filter, so this proves nothing");
    }
}
