//! Which repositories a Space holds (`yuzora:git` `repos`), for the Git
//! sidebar's repo picker: the one the Space's folder is in, the ones its
//! panes' folders are in, and the folders one and two below the Space's
//! folder with a `.git` (a folder or a gitfile).
//!
//! Only the file system is read: no git runs, so nothing a repository's
//! config names runs either, trusted or not. Here it is read in this
//! process ([`here`]); on a machine, [`SCRIPT`] reads it in one `hc exec`.
//! Both give the same answer, in the same order.

use std::path::{Path, PathBuf};
use std::time::Duration;

use yuzora_core::remote::{self, ExecRequest};

/// How many repositories are listed at most; past that, `truncated`.
pub const MOST: usize = 50;

/// How many folders below the Space's are looked at at most (the first, in
/// path order); past that, `truncated` too.
const FOLDERS: usize = 5000;

/// Folders never looked into: a git dir, dependencies and build output (they
/// hold other people's repositories, and many folders).
const SKIP: [&str; 3] = [".git", "node_modules", "target"];

/// How long a machine has to answer.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A repository found: its top level (the real path, as git names it) and
/// where it is from the Space's folder (`.` the folder itself, a path below
/// it, else the top level again).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub root: String,
    pub rel: String,
}

/// The repositories found, nearest first, and whether more were left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub repos: Vec<Found>,
    pub truncated: bool,
}

/// The repositories of a Space whose folder is `folder`, with panes in
/// `cwds`, here or on machine `on`. A folder that isn't there has none.
pub fn find(folder: &str, cwds: &[String], on: Option<&str>) -> Result<Listing, String> {
    match on.filter(|m| !m.is_empty()) {
        Some(m) => on_machine(m, folder, cwds),
        None => Ok(here(Path::new(folder), cwds)),
    }
}

/// Whether git takes `dir` as a work tree's top level: it has a `.git`
/// folder or file (a link to one too). git reads more before it agrees;
/// opening the repository is what tells.
fn has_git(dir: &Path) -> bool {
    std::fs::metadata(dir.join(".git")).is_ok_and(|m| m.is_dir() || m.is_file())
}

/// Collects repositories by real path, each once, up to [`MOST`].
struct Collect {
    base: PathBuf,
    out: Listing,
}

impl Collect {
    /// Adds the repository whose real top level is `top`; false once
    /// [`MOST`] are listed (and one more was found).
    fn add(&mut self, top: &Path) -> bool {
        let root = git_path(top);
        if self.out.repos.iter().any(|r| r.root == root) {
            return true;
        }
        if self.out.repos.len() == MOST {
            self.out.truncated = true;
            return false;
        }
        let rel = match top.strip_prefix(&self.base) {
            Ok(rest) if rest.as_os_str().is_empty() => ".".to_owned(),
            Ok(rest) => git_path(rest),
            Err(_) => root.clone(),
        };
        self.out.repos.push(Found { root, rel });
        true
    }
}

/// A path as git writes it: on Windows without `\\?\`, with `/` between names.
fn git_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { yuzora_core::paths::plain_str(&text).replace('\\', "/") } else { text.into_owned() }
}

/// [`find`] here: the repository each of `folder` and `cwds` is in (itself
/// or the nearest folder above with a `.git`, by real path, as git looks),
/// then the folders one and two below `folder` with a `.git`, in path order
/// (bytes). Links to folders are followed (two levels at most, so a link
/// back up ends there); [`SKIP`] folders aren't looked into.
pub fn here(folder: &Path, cwds: &[String]) -> Listing {
    let Ok(base) = std::fs::canonicalize(folder) else { return Listing::default() };
    let mut found = Collect { base: base.clone(), out: Listing::default() };
    for dir in std::iter::once(folder).chain(cwds.iter().map(Path::new)) {
        if let Ok(real) = std::fs::canonicalize(dir)
            && let Some(top) = real.ancestors().find(|a| has_git(a))
            && !found.add(top)
        {
            return found.out;
        }
    }
    let mut below = Vec::new();
    for dir in folders_in(&base) {
        below.extend(folders_in(&dir));
        below.push(dir);
    }
    below.sort_by(|a, b| a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
    if below.len() > FOLDERS {
        below.truncate(FOLDERS);
        found.out.truncated = true;
    }
    for dir in below {
        if has_git(&dir)
            && let Ok(top) = std::fs::canonicalize(&dir)
            && !found.add(&top)
        {
            break;
        }
    }
    found.out
}

/// The folders in `dir` (links to folders too), but [`SKIP`]'s.
fn folders_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| !SKIP.iter().any(|s| e.file_name() == *s))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// [`find`] on machine `m`: [`SCRIPT`] in one `hc exec`, read by [`listing`].
fn on_machine(m: &str, folder: &str, cwds: &[String]) -> Result<Listing, String> {
    let args = ["-c", SCRIPT, "sh", folder].into_iter().map(str::to_owned).chain(cwds.iter().cloned());
    let req = ExecRequest::new("sh", args).on(Some(m)).timeout(TIMEOUT).max_output(1 << 20);
    let out = remote::run(&req).map_err(|e| e.to_string())?;
    listing(&out.stdout).ok_or_else(|| format!("listing the repositories on {m} failed ({}): {}", out.code, String::from_utf8_lossy(&out.stderr).trim()))
}

/// What [`SCRIPT`] printed, read as [`here`] answers; none unless it got to
/// its end.
fn listing(out: &[u8]) -> Option<Listing> {
    let text = std::str::from_utf8(out).ok()?;
    let parts: Vec<&str> = text.split('\0').collect();
    let mut found: Option<Collect> = None;
    let mut ended = false;
    for pair in parts.chunks(2).filter(|c| c.len() == 2) {
        match (pair[0], pair[1]) {
            ("base", path) if found.is_none() => found = Some(Collect { base: PathBuf::from(path), out: Listing::default() }),
            ("repo", path) => {
                if let Some(f) = found.as_mut() {
                    f.add(Path::new(path));
                }
            }
            ("more", "") => found.as_mut()?.out.truncated = true,
            ("gone", "") if found.is_none() => found = Some(Collect { base: PathBuf::new(), out: Listing::default() }),
            ("end", "") => ended = true,
            _ => return None,
        }
    }
    ended.then_some(found?.out)
}

/// [`here`] in POSIX sh, for a machine: `$1` the Space's folder, then the
/// panes' folders. Prints `base\0<real path>\0`, a `repo\0<real top
/// level>\0` for each repository (at most [`MOST`]), `more\0\0` when there
/// were more, and `end\0\0` last; `gone\0\0` instead of `base` when the
/// folder isn't there. `find -L` follows links to folders as [`here`] does
/// (two levels, so a link back up ends there); its names come back a line
/// each, so one with a line feed in it can't be read and is passed over
/// (`real` refuses it; a part of one is no folder). Sorted as bytes
/// (`LC_ALL=C`), as [`here`] sorts.
const SCRIPT: &str = r#"nl='
'
set -f
real() {
  r=$(cd -P -- "$1" 2>/dev/null && pwd -P && echo .) || return 1
  r=${r%"$nl."}
  case "$r" in *"$nl"*) return 1 ;; esac
}
seen=$nl n=0
add() {
  case "$seen" in *"$nl$1$nl"*) return 0 ;; esac
  seen=$seen$1$nl n=$((n + 1))
  if [ "$n" -gt 50 ]; then printf 'more\0\0end\0\0'; exit 0; fi
  printf 'repo\0%s\0' "$1"
}
has() { [ -d "$1/.git" ] || [ -f "$1/.git" ]; }
up() {
  real "$1" || return 0
  d=$r
  while :; do
    if has "$d"; then add "$d"; return 0; fi
    case "$d" in /) return 0 ;; esac
    d=${d%/*}
    [ -n "$d" ] || d=/
  done
}
real "$1" || { printf 'gone\0\0end\0\0'; exit 0; }
base=$r
printf 'base\0%s\0' "$base"
for c in "$@"; do up "$c"; done
dirs=$(find -L "$base" -mindepth 1 -maxdepth 2 \( -name .git -o -name node_modules -o -name target \) -prune -o -type d -print 2>/dev/null | LC_ALL=C sort | head -n 5001)
IFS=$nl
k=0
for d in $dirs; do
  k=$((k + 1))
  if [ "$k" -gt 5000 ]; then printf 'more\0\0'; break; fi
  if has "$d" && real "$d"; then add "$r"; fi
done
printf 'end\0\0'
"#;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A scratch folder (its real path) for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yuzora-git-repos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn mkdirs(base: &Path, dirs: &[&str]) {
        for d in dirs {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
    }

    fn rels(l: &Listing) -> Vec<&str> {
        l.repos.iter().map(|r| r.rel.as_str()).collect()
    }

    /// [`SCRIPT`] run here with sh, read as a machine's answer is.
    fn by_script(folder: &Path, cwds: &[String]) -> Option<Listing> {
        let out = std::process::Command::new("sh").args(["-c", SCRIPT, "sh"]).arg(folder).args(cwds).output().unwrap();
        listing(&out.stdout)
    }

    /// [`here`], checked against [`SCRIPT`]: both must answer the same.
    fn both(folder: &Path, cwds: &[String]) -> Listing {
        let got = here(folder, cwds);
        assert_eq!(by_script(folder, cwds).as_ref(), Some(&got), "the script and here differ");
        got
    }

    #[test]
    fn the_space_and_two_levels_below_it_are_looked_at() {
        let base = scratch("levels");
        // `a-b` before `a/b`: in byte order, as the script's `sort` has it.
        mkdirs(&base, &[".git", "a/.git", "a/b/.git", "a-b/.git", "b/c/.git", "b/d", "e/f/g/.git", "h"]);
        // A gitfile (a submodule, a linked worktree) counts too.
        std::fs::write(base.join("h/.git"), "gitdir: elsewhere\n").unwrap();
        let got = both(&base, &[]);
        assert_eq!(rels(&got), [".", "a", "a-b", "a/b", "b/c", "h"], "three levels down isn't looked at");
        assert_eq!(got.repos[1].root, base.join("a").to_string_lossy());
        assert!(!got.truncated);
    }

    #[test]
    fn dependencies_build_output_and_git_dirs_are_passed_over() {
        let base = scratch("skip");
        mkdirs(&base, &["node_modules/x/.git", "target/y/.git", "app/node_modules/z/.git", "app/.git", "sub/.git/modules/w/.git", "sub/.git/inner/.git"]);
        assert_eq!(rels(&both(&base, &[])), ["app", "sub"]);
        // A `.git` that is neither a folder nor a file (a FIFO) is no repository.
        mkdirs(&base, &["fifo"]);
        assert!(std::process::Command::new("mkfifo").arg(base.join("fifo/.git")).status().unwrap().success());
        assert_eq!(rels(&both(&base, &[])), ["app", "sub"]);
    }

    #[test]
    fn the_repos_the_folder_and_the_panes_are_in_come_first() {
        let base = scratch("up");
        let outer = scratch("up-outer");
        mkdirs(&base, &[".git", "space", "space/z/.git", "space/in/deep"]);
        mkdirs(&outer, &[".git", "x"]);
        let space = base.join("space");
        let cwds = [space.join("in/deep").to_string_lossy().into_owned(), outer.join("x").to_string_lossy().into_owned(), "/no/such/folder".into()];
        let got = both(&space, &cwds);
        // The folder is in `base`'s repository (above it), so is a pane; another pane is in `outer`'s.
        assert_eq!(got.repos.iter().map(|r| (r.root.as_str(), r.rel.as_str())).collect::<Vec<_>>(), [
            (&*base.to_string_lossy(), &*base.to_string_lossy()),
            (&*outer.to_string_lossy(), &*outer.to_string_lossy()),
            (&*space.join("z").to_string_lossy(), "z"),
        ]);
        assert_eq!(both(&base.join("nothing"), &[]), Listing::default(), "a folder that isn't there");
    }

    #[test]
    fn links_are_followed_two_levels_and_each_repo_listed_once() {
        let base = scratch("links");
        let away = scratch("links-away");
        mkdirs(&base, &["r/.git"]);
        mkdirs(&away, &["s/.git"]);
        std::os::unix::fs::symlink(&away, base.join("l")).unwrap();
        std::os::unix::fs::symlink(base.join("r"), base.join("r2")).unwrap();
        // Back up to the Space's folder: two levels, then it ends.
        std::os::unix::fs::symlink(&base, base.join("loop")).unwrap();
        let got = both(&base, &[]);
        // `l/s` is listed by its real path (outside the folder); `r2` and `loop/r` are `r`.
        assert_eq!(rels(&got), [&*away.join("s").to_string_lossy(), "r"]);
    }

    #[test]
    fn past_fifty_repos_the_list_is_cut() {
        let base = scratch("most");
        for i in 0..(MOST + 3) {
            mkdirs(&base, &[&format!("r{i:02}/.git")]);
        }
        let got = both(&base, &[]);
        assert_eq!(got.repos.len(), MOST);
        assert_eq!((got.repos[0].rel.as_str(), got.repos[MOST - 1].rel.as_str()), ("r00", "r49"));
        assert!(got.truncated);
        for i in MOST..(MOST + 3) {
            std::fs::remove_dir_all(base.join(format!("r{i:02}"))).unwrap();
        }
        assert!(!both(&base, &[]).truncated, "exactly fifty");
    }

    #[test]
    fn past_the_folders_looked_at_the_list_is_cut() {
        let base = scratch("folders");
        for i in 0..(FOLDERS + 1) {
            std::fs::create_dir(base.join(format!("d{i:05}"))).unwrap();
        }
        mkdirs(&base, &["d00000/.git"]);
        let got = both(&base, &[]);
        assert_eq!((rels(&got), got.truncated), (vec!["d00000"], true));
    }

    #[test]
    fn a_machines_answer_reads_only_as_the_script_writes_it() {
        assert_eq!(listing(b"base\0/b\0repo\0/b\0repo\0/b/x\0repo\0/elsewhere\0end\0\0").map(|l| l.repos.into_iter().map(|r| r.rel).collect::<Vec<_>>()), Some(vec![".".into(), "x".into(), "/elsewhere".into()]));
        assert_eq!(listing(b"base\0/b\0more\0\0end\0\0").map(|l| l.truncated), Some(true));
        assert_eq!(listing(b"gone\0\0end\0\0"), Some(Listing::default()));
        for cut in [&b"base\0/b\0repo\0/b\0"[..], b"", b"base\0/b\0what\0x\0end\0\0", b"repo\0/b\0end\0\0"] {
            assert_eq!(listing(cut).map(|l| l.repos.len()).filter(|n| *n > 0), None, "{:?}", String::from_utf8_lossy(cut));
        }
    }

    // On a machine: one `hc exec` (the test's stand-in runs it here).
    #[test]
    fn a_machine_is_asked_once() {
        let s = crate::repo::testing::scratch();
        crate::repo::testing::on_machine(&s);
        let base = scratch("machine");
        mkdirs(&base, &["a/.git", "b/.git"]);
        let before = crate::repo::testing::hc_calls_in(".");
        let got = find(&base.to_string_lossy(), &[], Some("test-machine")).unwrap();
        assert_eq!(rels(&got), ["a", "b"]);
        assert_eq!(crate::repo::testing::hc_calls_in(".") - before, 1);
    }
}
