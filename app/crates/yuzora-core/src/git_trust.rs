//! Trusted folders for Git, for everything that runs git on its own: the Git
//! sidebar (L2), the status bar's branch (L1), the file tree's marks (L5).
//!
//! A repository's own config (`.git/config`, what it includes, the
//! worktree's, its submodules') can name programs git runs without asking:
//! a clean filter during `status` or `diff`, an ssh command or a credential
//! helper during `fetch`. Until the person trusts the folder (the Git
//! sidebar asks), nothing may run git there on its own in a way that starts
//! them. The person's own (global, system) config is theirs and needs no
//! trust.
//!
//! - Any time, with [`READ_FLAGS`] first: commands that read refs only,
//!   never the index or the work tree — `rev-parse --show-toplevel`,
//!   `symbolic-ref`, `for-each-ref` (no `:track`), `config --get`.
//! - Walking commits (`rev-list --left-right --count`, `for-each-ref`'s
//!   `%(upstream:track)`, `log`, `merge-base`) only with `--no-lazy-fetch`
//!   (git 2.44 and later) when the repo has a promisor remote: a ref may
//!   name a commit the clone lacks, and git fetches it with the repo's own
//!   ssh command. With an older git, don't walk (`yuzora-shell`'s
//!   `gitStatus` is an example).
//! - Only once [`pending`] is empty: `status`, `diff` and its kin
//!   (`diff-files`, `diff-index`, `ls-files -m`), anything that compares or
//!   writes the work tree or the index, and `fetch`, `pull`, `push`.
//!
//! Submodules count with their own config: every checked-out gitlink in
//! the index, nested ones too, found where git looks for them (a
//! submodule's own `core.worktree` may put its work tree elsewhere).
//! `status` and `diff` look into those whether or not a `.gitmodules`
//! names them.
//!
//! A folder's id is `<machine>:<top>` (machine empty here), `top` as
//! `git rev-parse --show-toplevel` prints it there. The trusted ids live in
//! `<app home>/yuzora/git/trusted.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::remote::{self, ExecRequest};

/// Config keys (lowercased by git) that name a program run without asking.
pub const PROGRAMS: &str = r"^(filter\..+\.(clean|smudge|process)|core\.(fsmonitor|sshcommand|gitproxy|askpass|alternaterefscommand|alternaterefsprefixes)|credential(\..+)?\.helper|remote\..+\.(uploadpack|receivepack|vcs)|protocol(\..+)?\.allow|merge\..+\.driver|diff\.(external|.+\.(command|textconv))|gpg(\..+)?\.program|gpg\.ssh\.defaultkeycommand|submodule\..+\.update)$";

/// Whether a matched key's value runs something: fsmonitor as a boolean is
/// git's own daemon, and a submodule's update mode is a command only after
/// `!`. Only the exact values git takes that way pass (no trimming, ASCII
/// case as git compares); anything else counts.
pub fn names_program(key: &str, value: &str) -> bool {
    let any = |list: &[&str]| list.iter().any(|v| value.eq_ignore_ascii_case(v));
    if key == "core.fsmonitor" {
        return !any(&["", "true", "false", "yes", "no", "on", "off", "1", "0"]);
    }
    if key.starts_with("submodule.") && key.ends_with(".update") {
        return !any(&["checkout", "rebase", "merge", "none"]);
    }
    true
}

/// Stands for the keys when the config couldn't be read: the gate stays shut.
pub const UNREADABLE: &str = "(unreadable config)";

/// Before every git command run on its own: no optional index refresh, and
/// no fsmonitor hook the repository's config names.
pub const READ_FLAGS: [&str; 3] = ["--no-optional-locks", "-c", "core.fsmonitor=false"];

/// How long [`programs_in`] may take, submodules included.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The most an answer to [`repo_programs`] may hold. Ask a runner for one
/// byte more and pass what it gave through [`whole`].
pub const ANSWER_CAP: usize = 8 << 20;

/// A runner's answer as [`repo_programs`] takes it: one past
/// [`ANSWER_CAP`] was cut short somewhere, maybe on a record's edge with the
/// key that counts unread, so it counts as git failing (the gate stays shut).
pub fn whole(code: i32, stdout: Vec<u8>) -> (i32, Vec<u8>) {
    if stdout.len() > ANSWER_CAP { (-1, Vec::new()) } else { (code, stdout) }
}

/// The program keys the repository's own config sets, and its submodules'
/// (status looks inside them, with their configs), as `path: key`. `run`
/// runs git with these arguments in the repository's top level and answers
/// its exit code and stdout, through [`whole`]. A config git can't read
/// counts as [`UNREADABLE`].
pub fn repo_programs<E>(mut run: impl FnMut(&[&str]) -> Result<(i32, Vec<u8>), E>) -> Result<Vec<String>, E> {
    let (code, out) = run(&["config", "--show-scope", "-z", "--get-regexp", PROGRAMS])?;
    // 1 with nothing printed: no key matched. Anything else: unknown.
    let mut keys = match code {
        0 => repo_keys(&out, ""),
        1 if out.is_empty() => BTreeSet::new(),
        _ => return Ok(vec![UNREADABLE.into()]),
    };
    // The submodules, whatever the repository's own config said.
    let alias = format!("alias.{SCAN}=!{}", SCAN_SCRIPT.replace("@PROGRAMS@", PROGRAMS).replace("@SCAN@", SCAN));
    let (code, out) = run(&["-c", &alias, SCAN])?;
    if code != 0 || !scanned_to_the_end(&out) {
        keys.insert(UNREADABLE.into());
    }
    keys.extend(repo_keys(&out, "?"));
    Ok(keys.into_iter().collect())
}

/// Whether the scan's answer ends with its last record (`end\0<SCAN>\0`,
/// records in pairs): one that stopped part way may lack a submodule.
fn scanned_to_the_end(out: &[u8]) -> bool {
    let parts: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    parts.len() >= 3 && parts.len() % 2 == 1 && parts[parts.len() - 3] == b"end" && parts[parts.len() - 2] == SCAN.as_bytes() && parts[parts.len() - 1].is_empty()
}

/// The command line's own alias for [`SCAN_SCRIPT`]: git runs it in the top
/// level, so any runner of git (here, `hc exec --on`) does in one call.
const SCAN: &str = "yuzora-trust-scan";

/// Each checked-out gitlink in the index, nested ones too, and its config's
/// program keys: `sub\0path\0` then the records, `err\0what\0` when it can't
/// be read, and `end\0<SCAN>\0` last. It follows git: a gitlink is checked
/// out when its folder has a `.git`, and git works in it with `GIT_DIR=.git`
/// (never a repo further up), so its config is read that way, and its own
/// gitlinks are where its work tree is (its `core.worktree`, from its config
/// or its gitdir's). grep picks the gitlinks out of git's list, and must pass
/// on a last line added to it: a grep that failed would print nothing, which
/// would read as no gitlinks. A quoted path (a `"`, `\` or control
/// character) can't be followed, nor more than 16 levels (a work tree
/// pointing back up): both count as unreadable. `GIT_NO_LAZY_FETCH`: listing
/// a sparse index in a partial clone never fetches with the repo's ssh.
const SCAN_SCRIPT: &str = r#"unset GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_PREFIX
GIT_NO_LAZY_FETCH=1
export GIT_NO_LAZY_FETCH
tab=$(printf '\t') nl='
'
scan() {
  [ "$2" -lt 16 ] || { printf 'sub\0%s\0err\0depth\0' "$1"; return; }
  links=$( { git -c core.quotepath=false ls-files -s || echo '#failed'; echo '160000 #end'; } | LC_ALL=C grep -e '^160000 ' -e '^#failed$') || links=
  case "$nl$links" in
    *"$nl#failed$nl"*) links= ;;
    *"${nl}160000 #end") ;;
    *) links= ;;
  esac
  [ -n "$links" ] || { printf 'sub\0%s\0err\0index\0' "${1:-.}"; return; }
  printf '%s\n' "$links" | while IFS= read -r line; do
    [ "$line" = '160000 #end' ] && continue
    p=${line#*"$tab"}
    case "$p" in \"*) printf 'sub\0%s%s\0err\0name\0' "$1" "$p"; continue ;; esac
    (
      cd "./$p" 2>/dev/null && [ -e .git ] || exit 0
      GIT_DIR=.git
      export GIT_DIR
      unset GIT_WORK_TREE
      printf 'sub\0%s%s\0' "$1" "$p"
      git config --show-scope -z --get-regexp '@PROGRAMS@'
      c=$?
      if [ $c -gt 1 ]; then printf 'err\0%s\0' $c; exit 0; fi
      w=$(git rev-parse --absolute-git-dir --show-toplevel) && gd=${w%%"$nl"*} && top=${w#*"$nl"} && cd "$top" || { printf 'err\0worktree\0'; exit 0; }
      GIT_DIR=$gd GIT_WORK_TREE=$top
      export GIT_DIR GIT_WORK_TREE
      scan "$1$p/" $(($2 + 1))
    )
  done
}
scan "" 0
printf 'end\0@SCAN@\0'
"#;

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

/// [`repo_programs`] for the repository at `top`, here or on `machine`
/// (through `hc exec --on`), with `git` from PATH there. Err: git couldn't
/// run or answer; count the folder as not safe.
pub fn programs_in(machine: Option<&str>, top: &str) -> Result<Vec<String>, String> {
    let machine = machine.filter(|m| !m.is_empty());
    repo_programs(|args| {
        let req = ExecRequest::new("git", READ_FLAGS.iter().copied().chain(args.iter().copied())).on(machine).cwd(Some(top)).timeout(TIMEOUT).max_output(ANSWER_CAP as u64 + 1);
        remote::run(&req).map(|o| whole(o.code, o.stdout)).map_err(|e| e.to_string())
    })
}

/// A folder's id: `<machine>:<top>`, machine empty here.
pub fn folder_id(machine: Option<&str>, top: &str) -> String {
    format!("{}:{top}", machine.unwrap_or(""))
}

/// `<app home>/yuzora/git/trusted.json`.
pub fn trusted_file() -> PathBuf {
    crate::paths::yuzora_dir().join("git").join("trusted.json")
}

/// The trusted folder ids in `file` (none when it can't be read).
pub fn trusted_from(file: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(file).ok().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).map(|v| v.into_iter().collect()).unwrap_or_default()
}

/// Adds a folder id to `file` (the Git sidebar's "Trust").
pub fn trust_at(file: &Path, id: &str) -> std::io::Result<()> {
    let mut all = trusted_from(file);
    all.insert(id.to_owned());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let list: Vec<&String> = all.iter().collect();
    std::fs::write(file, serde_json::to_string_pretty(&list).unwrap_or_default())
}

/// Whether the person trusted the folder at `top` (here or on `machine`).
pub fn is_trusted(machine: Option<&str>, top: &str) -> bool {
    trusted_from(&trusted_file()).contains(&folder_id(machine.filter(|m| !m.is_empty()), top))
}

/// What keeps git from running on its own in the folder at `top`: its
/// repository's program keys, unless the person trusted it. Empty: `status`,
/// `diff` and the like may run. Err: count it as not safe.
pub fn pending(machine: Option<&str>, top: &str) -> Result<Vec<String>, String> {
    pending_from(&trusted_file(), machine, top)
}

/// [`pending`] with the trusted ids in `file`.
pub fn pending_from(file: &Path, machine: Option<&str>, top: &str) -> Result<Vec<String>, String> {
    let machine = machine.filter(|m| !m.is_empty());
    if trusted_from(file).contains(&folder_id(machine, top)) { Ok(Vec::new()) } else { programs_in(machine, top) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_values_that_run_something_count() {
        assert!(!names_program("core.fsmonitor", "true") && !names_program("core.fsmonitor", "TRUE") && !names_program("core.fsmonitor", ""));
        // What git would read as a hook path, though it looks like a boolean.
        assert!(names_program("core.fsmonitor", "true\u{a0}") && names_program("core.fsmonitor", " true") && names_program("core.fsmonitor", "2"));
        assert!(!names_program("submodule.x.update", "rebase") && names_program("submodule.x.update", " rebase") && names_program("submodule.x.update", "!touch x"));
        assert!(names_program("filter.x.clean", "cat") && names_program("core.sshcommand", "ssh"));
    }

    #[test]
    fn records_and_exit_codes_read_as_the_gate_needs() {
        let answer = |config: (i32, &str), subs: (i32, &str)| {
            let mut calls = Vec::new();
            let keys = repo_programs(|args| {
                calls.push(args[0].to_owned());
                Ok::<_, ()>(if args[0] == "config" { (config.0, config.1.as_bytes().to_vec()) } else { (subs.0, subs.1.as_bytes().to_vec()) })
            })
            .unwrap();
            (keys, calls)
        };
        // The scan's last record: an answer without it was cut short.
        const END: &str = "end\0yuzora-trust-scan\0";
        // No key: nothing pending.
        assert_eq!(answer((1, ""), (0, END)).0, Vec::<String>::new());
        // An unreadable config shuts the gate and stops there.
        assert_eq!(answer((128, ""), (0, "")), (vec![UNREADABLE.to_string()], vec!["config".to_string()]));
        // Only the repository's own scopes count, and only values that run something.
        let own = "local\0filter.x.clean\nprog\0global\0core.sshcommand\nssh\0worktree\0core.fsmonitor\ntrue\0local\0diff.x.textconv\0";
        assert_eq!(answer((0, own), (0, END)).0, ["diff.x.textconv", "filter.x.clean"]);
        // Submodules name themselves; one git can't read keeps the gate shut.
        let subs = &format!("sub\0vendor/a\0local\0filter.y.clean\ncat\0sub\0vendor/b\0err\0128\0{END}");
        assert_eq!(answer((1, ""), (0, subs)).0, ["vendor/a: filter.y.clean", "vendor/b: (unreadable config)"], "no key above: the submodules still count");
        assert_eq!(answer((0, "global\0core.askpass\nx\0"), (0, subs)).0, ["vendor/a: filter.y.clean", "vendor/b: (unreadable config)"]);
        assert!(answer((0, ""), (2, "")).0.contains(&UNREADABLE.to_string()));
        // A scan that stopped part way (exit 0, records missing) keeps it shut.
        assert_eq!(answer((1, ""), (0, "")).0, [UNREADABLE]);
        assert_eq!(answer((1, ""), (0, "sub\0vendor/a\0local\0filter.y.clean\ncat\0")).0, [UNREADABLE, "vendor/a: filter.y.clean"]);
    }

    #[test]
    fn trusted_ids_are_kept_in_their_file() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        assert!(trusted_from(&file).is_empty());
        assert_eq!(folder_id(None, "/r"), ":/r");
        assert_eq!(folder_id(Some("lab-01"), "/srv/r"), "lab-01:/srv/r");
        trust_at(&file, &folder_id(Some("lab-01"), "/srv/r")).unwrap();
        trust_at(&file, ":/r").unwrap();
        assert_eq!(trusted_from(&file).into_iter().collect::<Vec<_>>(), [":/r", "lab-01:/srv/r"]);
        std::fs::write(&file, "not json").unwrap();
        assert!(trusted_from(&file).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_repo_here_waits_for_trust_until_its_folder_is_trusted() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&dir).status().unwrap().success(), "git {args:?}");
        git(&["init", "-q"]);
        let top = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
        let file = dir.join("trusted.json");
        assert_eq!(pending_from(&file, None, &top), Ok(Vec::new()), "a plain repo needs no trust");
        git(&["config", "filter.evil.clean", "touch ran"]);
        assert_eq!(pending_from(&file, None, &top), Ok(vec!["filter.evil.clean".to_string()]));
        assert!(!dir.join("ran").exists());
        // `git diff` without `--no-ext-diff` runs it.
        git(&["config", "diff.external", "touch ran"]);
        assert_eq!(pending_from(&file, None, &top), Ok(vec!["diff.external".to_string(), "filter.evil.clean".into()]));
        trust_at(&file, &folder_id(None, &top)).unwrap();
        assert_eq!(pending_from(&file, Some(""), &top), Ok(Vec::new()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_checked_out_gitlink_counts_whether_or_not_gitmodules_names_it() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let top = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
        let git = |at: &str, args: &[&str]| {
            let out = std::process::Command::new("git").args(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false"]).args(args).current_dir(Path::new(&top).join(at)).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let repo = |at: &str| {
            std::fs::create_dir_all(Path::new(&top).join(at)).unwrap();
            git(at, &["init", "-q"]);
            std::fs::write(Path::new(&top).join(at).join("f"), "x\n").unwrap();
            git(at, &["add", "f"]);
            git(at, &["commit", "-qm", "s"]);
        };
        // Gitlinks in the index, none in a .gitmodules: `sub`, and `deep` inside it.
        repo("");
        repo("sub");
        repo("sub/deep");
        let link = |at: &str, path: &str, to: &str| git(at, &["update-index", "--add", "--cacheinfo", &format!("160000,{},{path}", git(to, &["rev-parse", "HEAD"]))]);
        link("sub", "deep", "sub/deep");
        link("", "sub", "sub");
        // Not checked out: git doesn't look inside, nor does the check.
        std::fs::create_dir_all(Path::new(&top).join("empty")).unwrap();
        link("", "empty", "sub");
        let marker = Path::new(&top).join("ran");
        git("sub", &["config", "filter.evil.clean", &format!("touch '{}'; cat", marker.display())]);
        std::fs::write(Path::new(&top).join("sub/.gitattributes"), "* filter=evil\n").unwrap();
        git("sub/deep", &["config", "core.sshCommand", "true"]);
        let file = dir.join("trusted.json");
        assert_eq!(pending_from(&file, None, &top), Ok(vec!["sub/deep: core.sshcommand".to_string(), "sub: filter.evil.clean".into()]));
        assert!(!marker.exists());
        // What it guards: status above looks into `sub` and, with the file's
        // time changed, runs its clean filter.
        std::fs::File::options().write(true).open(Path::new(&top).join("sub/f")).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800)).unwrap();
        git("", &["--no-optional-locks", "status", "--porcelain=v2", "--untracked-files=no"]);
        assert!(marker.exists(), "status didn't run the submodule's filter, so this test proves nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A scratch folder for a test, with `git` run in a folder inside it.
    #[cfg(unix)]
    fn scratch_dir(name: &str) -> (PathBuf, String, impl Fn(&str, &[&str]) -> String) {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let top = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
        let at = top.clone();
        let git = move |path: &str, args: &[&str]| {
            let here = Path::new(&at).join(path);
            std::fs::create_dir_all(&here).unwrap();
            let out = std::process::Command::new("git").args(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false"]).args(args).current_dir(here).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        (dir, top, git)
    }

    #[cfg(unix)]
    #[test]
    fn a_submodule_that_works_elsewhere_is_followed_there() {
        let (dir, top, git) = scratch_dir("worktree");
        let repo = |at: &str| {
            git(at, &["init", "-q"]);
            std::fs::write(Path::new(&top).join(at).join("f"), "x\n").unwrap();
            git(at, &["add", "f"]);
            git(at, &["commit", "-qm", "s"]);
        };
        let link = |at: &str, path: &str, to: &str| git(at, &["update-index", "--add", "--cacheinfo", &format!("160000,{},{path}", git(to, &["rev-parse", "HEAD"]))]);
        repo("");
        repo("other/deep");
        repo("sub");
        // `sub` links `deep`, and works in `other` (its core.worktree): git
        // looks for `deep` there, not in `sub`.
        link("sub", "deep", "other/deep");
        git("sub", &["config", "core.worktree", "../../other"]);
        link("", "sub", "sub");
        let marker = Path::new(&top).join("ran");
        git("other/deep", &["config", "filter.evil.clean", &format!("touch '{}'; cat", marker.display())]);
        std::fs::write(Path::new(&top).join("other/deep/.gitattributes"), "* filter=evil\n").unwrap();
        let file = dir.join("trusted.json");
        assert_eq!(pending_from(&file, None, &top), Ok(vec!["sub/deep: filter.evil.clean".to_string()]));
        assert!(!marker.exists());
        // What it guards: status above goes through `sub` into `other/deep`
        // and, with the file's time changed, runs its clean filter.
        std::fs::File::options().write(true).open(Path::new(&top).join("other/deep/f")).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800)).unwrap();
        git("", &["--no-optional-locks", "status", "--porcelain=v2", "--untracked-files=no"]);
        assert!(marker.exists(), "status didn't run the filter there, so this test proves nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_failing_part_way_keeps_the_gate_shut() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, top, git) = scratch_dir("tools");
        git("", &["init", "-q"]);
        git("sub", &["init", "-q"]);
        std::fs::write(Path::new(&top).join("sub/f"), "x\n").unwrap();
        git("sub", &["add", "f"]);
        git("sub", &["commit", "-qm", "s"]);
        git("sub", &["config", "core.sshCommand", "true"]);
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{},sub", git("sub", &["rev-parse", "HEAD"]))]);
        let scan = |path: &str| {
            repo_programs(|args| {
                let out = std::process::Command::new("git").args(READ_FLAGS).args(args).current_dir(&top).env("PATH", path).output()?;
                Ok::<_, std::io::Error>(whole(out.status.code().unwrap_or(-1), out.stdout))
            })
            .unwrap()
        };
        assert_eq!(scan(&std::env::var("PATH").unwrap_or_default()), ["sub: core.sshcommand"]);
        // Tools a shell script might pipe through, broken: failing, or
        // printing nothing as if all went well. Never "no key".
        for (i, body) in ["exit 1", "exit 0"].iter().enumerate() {
            let stub = dir.join(format!("stub-{i}"));
            std::fs::create_dir_all(&stub).unwrap();
            for tool in ["sort", "grep", "cut", "sed", "awk", "tr", "uniq", "head"] {
                std::fs::write(stub.join(tool), format!("#!/bin/sh\ncat >/dev/null\n{body}\n")).unwrap();
                std::fs::set_permissions(stub.join(tool), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path = format!("{}:{}", stub.display(), std::env::var("PATH").unwrap_or_default());
            assert_eq!(scan(&path), [format!(".: {UNREADABLE}")], "tools that {body}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_answer_cut_short_keeps_the_gate_shut() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&dir).status().unwrap().success());
        // Keys that match but run nothing, more than any answer may carry,
        // then a filter: cut short, the filter would go unseen.
        let config = dir.join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[core]\n");
        text.push_str(&"\tfsmonitor = true\n".repeat(400_000));
        text.push_str("[filter \"evil\"]\n\tclean = cat\n");
        std::fs::write(&config, text).unwrap();
        let top = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
        let got = pending_from(&dir.join("trusted.json"), None, &top);
        std::fs::remove_dir_all(&dir).unwrap();
        // Cut anywhere, a record may end whole and the filter go unseen: a
        // cut answer is never read as a whole one.
        assert_eq!(got, Ok(vec![UNREADABLE.to_string()]));
    }
}
