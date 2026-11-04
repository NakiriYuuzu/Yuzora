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

/// How many folders below the Space's are looked at at most: as the file
/// system lists them, each folder's own right after it (as `find` walks).
/// Past that no more is read, and the list is `truncated` too.
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
/// `cwds`, here or on machine `on`. A folder that is gone has none; one that
/// can't be looked into now (no permission to it, say) is an error, so a
/// pinned repo isn't taken for gone.
pub fn find(folder: &str, cwds: &[String], on: Option<&str>) -> Result<Listing, String> {
    match on.filter(|m| !m.is_empty()) {
        Some(m) => on_machine(m, folder, cwds),
        None => here(Path::new(folder), cwds),
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
/// then the folders one and two below `folder` with a `.git` (the first
/// [`FOLDERS`] looked at), in path order (bytes). Links to folders are
/// followed (two levels at most, so a link back up ends there); [`SKIP`]
/// folders and names with a line feed in them aren't looked into, and a
/// folder counts only by its [`real`] path. A `folder` that can't be looked
/// into is none if it's [`gone`], else an error.
pub fn here(folder: &Path, cwds: &[String]) -> Result<Listing, String> {
    let base = match real(folder) {
        Some(base) => base,
        None if gone(folder) => return Ok(Listing::default()),
        None => return Err(format!("{} can't be looked into now", folder.display())),
    };
    let mut found = Collect { base: base.clone(), out: Listing::default() };
    for dir in std::iter::once(folder).chain(cwds.iter().map(Path::new)) {
        if let Some(real) = real(dir)
            && let Some(top) = real.ancestors().find(|a| has_git(a))
            && !found.add(top)
        {
            return Ok(found.out);
        }
    }
    let mut below = Vec::new();
    'walk: for dir in folders_in(&base) {
        below.push(dir.clone());
        if below.len() > FOLDERS {
            break;
        }
        for sub in folders_in(&dir) {
            below.push(sub);
            if below.len() > FOLDERS {
                break 'walk;
            }
        }
    }
    if below.len() > FOLDERS {
        below.truncate(FOLDERS);
        found.out.truncated = true;
    }
    below.sort_by(|a, b| a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
    for dir in below {
        if has_git(&dir)
            && let Some(top) = real(&dir)
            && !found.add(&top)
        {
            break;
        }
    }
    Ok(found.out)
}

/// `dir`'s real path, as [`SCRIPT`]'s `real` takes it: a folder that can be
/// looked into (`cd -P`), with no line feed in the path (the script reads
/// paths a line each).
fn real(dir: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(dir).ok().filter(|p| can_enter(p) && !p.as_os_str().as_encoded_bytes().contains(&b'\n'))
}

/// Whether `dir` is a folder that can be looked into, as `cd` takes it: on
/// unix, one it may search.
fn can_enter(dir: &Path) -> bool {
    dir.is_dir() && (cfg!(windows) || std::fs::metadata(dir.join(".")).is_ok())
}

/// Whether `folder`, which can't be looked into, is gone for sure: it, or a
/// folder on the way to it, isn't there (in a folder that can be looked
/// into), or is no folder. Else nothing is known now: no permission to it or
/// to a folder above it, or a link that doesn't lead anywhere (it may lead
/// into a folder that can't be looked into). As [`SCRIPT`]'s `gone`.
fn gone(folder: &Path) -> bool {
    let mut below = false;
    for p in folder.ancestors() {
        match std::fs::metadata(p) {
            Ok(m) => return !m.is_dir() || (below && can_enter(p)),
            Err(_) if std::fs::symlink_metadata(p).is_ok() => return false,
            Err(_) => below = true,
        }
    }
    false
}

/// The folders in `dir` (links to folders too), but [`SKIP`]'s and those
/// with a line feed in their name (as [`SCRIPT`]'s `find` prunes them), as
/// the file system lists them; each looked at only when it's asked for. Only
/// a link is looked up: the listing tells a folder from a file.
fn folders_in(dir: &Path) -> impl Iterator<Item = PathBuf> {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    entries
        .filter(|e| {
            let name = e.file_name();
            !SKIP.iter().any(|s| name == *s) && !name.as_encoded_bytes().contains(&b'\n')
        })
        .filter_map(|e| {
            #[cfg(test)]
            LOOKED.with(|n| n.set(n.get() + 1));
            let kind = e.file_type().ok()?;
            (kind.is_dir() || (kind.is_symlink() && e.path().is_dir())).then(|| e.path())
        })
}

#[cfg(test)]
thread_local! {
    /// How many folders [`folders_in`] looked at on this thread.
    static LOOKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
/// folder is gone (as [`gone`]), and nothing (exit 1) when it can't be looked
/// into now. `find -L` follows links to folders as [`here`] does
/// (two levels, so a link back up ends there). Its names come back a line
/// each, so a name with a line feed in it is pruned (`-name "*$nl*"`), as
/// [`folders_in`] passes it over: neither lists it nor looks into it, and
/// none of its parts is read as a path (a relative one would be the
/// machine's home's, where `hc exec` runs this). `real` refuses a real path
/// with one, as [`real`] does. The first 5000 as `find`
/// walks (the file system's order, as [`here`] reads them), then sorted as
/// bytes (`LC_ALL=C`), as [`here`] sorts.
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
gone() {
  p=$1 below=
  while :; do
    if [ -L "$p" ] && [ ! -e "$p" ]; then return 1; fi
    if [ -e "$p" ]; then
      [ -d "$p" ] || return 0
      [ -n "$below" ] && [ -e "$p/." ]
      return
    fi
    case "$p" in /) return 1 ;; */*) ;; *) return 1 ;; esac
    below=1 p=${p%/*}
    [ -n "$p" ] || p=/
  done
}
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
if ! real "$1"; then
  if gone "$1"; then printf 'gone\0\0end\0\0'; exit 0; fi
  printf '%s\n' "$1 can't be looked into now" >&2
  exit 1
fi
base=$r
printf 'base\0%s\0' "$base"
for c in "$@"; do up "$c"; done
all=$(find -L "$base" -mindepth 1 -maxdepth 2 \( -name .git -o -name node_modules -o -name target -o -name "*$nl*" \) -prune -o -type d -print 2>/dev/null | head -n 5001)
IFS=$nl
set -- $all
if [ "$#" -gt 5000 ]; then printf 'more\0\0'; all=$(printf '%s\n' "$all" | head -n 5000); fi
for d in $(printf '%s\n' "$all" | LC_ALL=C sort); do
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

    /// [`SCRIPT`] run here with sh (in `cwd`: a machine runs it in its home),
    /// read as a machine's answer is.
    fn by_script(folder: &Path, cwds: &[String], cwd: Option<&Path>) -> Option<Listing> {
        let mut sh = std::process::Command::new("sh");
        sh.args(["-c", SCRIPT, "sh"]).arg(folder).args(cwds);
        if let Some(cwd) = cwd {
            sh.current_dir(cwd);
        }
        listing(&sh.output().unwrap().stdout)
    }

    /// [`here`], checked against [`SCRIPT`] run in `cwd`: both must answer
    /// the same (or both have no answer).
    fn answer_in(folder: &Path, cwds: &[String], cwd: Option<&Path>) -> Result<Listing, String> {
        let got = here(folder, cwds);
        assert_eq!(by_script(folder, cwds, cwd), got.clone().ok(), "the script and here differ");
        got
    }

    fn answer(folder: &Path, cwds: &[String]) -> Result<Listing, String> {
        answer_in(folder, cwds, None)
    }

    // A name with a line feed in it is passed over, here and by the script. `find` prints a name
    // a line, so the script would read its parts as paths: relative ones in the machine's home
    // (where `hc exec` runs it), listing repos there, outside the space. Neither lists those, nor
    // a repo named so; a pane's folder or a link whose real path has one counts for nothing, and
    // a space folder with one can't be looked into.
    #[test]
    fn line_feed_names_are_passed_over() {
        let base = scratch("line-feed");
        let home = scratch("line-feed-home");
        mkdirs(&home, &[".dotfiles/.git", "src/proj/.git", "c/.git"]);
        mkdirs(&base, &["real/.git", "x\n.dotfiles", "a\nsrc/proj", "b\nc", "two\nlines/.git"]);
        std::os::unix::fs::symlink(base.join("two\nlines"), base.join("link")).unwrap();
        // Not looked into at all: not even a link in one, to a repo elsewhere.
        let elsewhere = scratch("line-feed-elsewhere");
        mkdirs(&elsewhere, &[".git"]);
        std::os::unix::fs::symlink(&elsewhere, base.join("a\nsrc/there")).unwrap();
        let cwds = [base.join("two\nlines").to_string_lossy().into_owned()];
        assert_eq!(rels(&answer_in(&base, &cwds, Some(&home)).unwrap()), ["real"]);
        mkdirs(&base, &["sp\nace/r/.git"]);
        assert!(answer_in(&base.join("sp\nace"), &[], Some(&home)).is_err(), "a space folder with a line feed");
    }

    // Names the script must take as names, never as code, options or patterns: quotes, `$(…)`,
    // backquotes, a backslash, a leading `-`, glob characters, `;`, printf directives, a tab.
    // Each is a repo in a space folder whose own name has a space; panes' folders are among them,
    // with `-` and `--` themselves. Here and the script (run in the space folder) list the same,
    // and nothing runs.
    #[test]
    fn special_names_are_names() {
        const NAMES: [&str; 16] = [
            "sp ace", "q'uote", "dq\"uote", "$(touch PWNED1)", "`touch PWNED2`", "back\\slash", "-dash", "--", "-L", "*", "[ab]",
            "a;touch PWNED3", "%s%n%d", "tab\there", "\\0nul", "é-ü",
        ];
        let base = scratch("special names");
        for name in NAMES {
            mkdirs(&base, &[&format!("{name}/.git")]);
        }
        let cwds: Vec<String> = ["sp ace", "-dash", "--", "*"].iter().map(|n| base.join(n).to_string_lossy().into_owned()).chain(["-".into(), "--".into()]).collect();
        let got = answer_in(&base, &cwds, Some(&base)).unwrap();
        assert_eq!(rels(&got)[..4], ["sp ace", "-dash", "--", "*"], "the panes' first");
        let mut listed = rels(&got);
        listed.sort();
        let mut want = NAMES.to_vec();
        want.sort();
        assert_eq!(listed, want);
        let ran: Vec<_> = std::fs::read_dir(&base).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("PWNED")).collect();
        assert!(ran.is_empty(), "{ran:?}");
    }

    fn both(folder: &Path, cwds: &[String]) -> Listing {
        answer(folder, cwds).expect("an answer")
    }

    /// Gives folders their permissions back, so a scratch folder can be removed.
    struct Unlock(Vec<PathBuf>);

    impl Drop for Unlock {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            for p in &self.0 {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    // A folder that can't be looked into now (no permission to it, or to a folder above it, or a
    // link that leads nowhere: maybe into such a folder) has no answer, both here and on a
    // machine: an error, not an empty list, so a pinned repo isn't unpinned for it. One gone for
    // sure (not there, a file, below a file) has no repositories.
    #[test]
    fn a_folder_that_cant_be_looked_into_has_no_answer() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("locked");
        mkdirs(&base, &["shut/.git", "above/r/.git"]);
        std::fs::write(base.join("file"), "").unwrap();
        std::os::unix::fs::symlink(base.join("nowhere"), base.join("dangling")).unwrap();
        std::os::unix::fs::symlink(base.join("above/r"), base.join("into")).unwrap();
        for gone in ["missing", "missing/deeper/still", "file", "file/sub"] {
            assert_eq!(answer(&base.join(gone), &[]), Ok(Listing::default()), "{gone}");
        }
        assert!(answer(&base.join("dangling"), &[]).is_err(), "a link to nothing");
        let _unlock = Unlock(vec![base.join("shut"), base.join("above")]);
        for dir in ["shut", "above"] {
            std::fs::set_permissions(base.join(dir), std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        if std::fs::metadata(base.join("shut/.")).is_ok() {
            return; // Privileged: no permission is refused.
        }
        for locked in ["shut", "above/r", "into", "into/deeper"] {
            assert!(answer(&base.join(locked), &[]).is_err(), "{locked}");
        }
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

    // The folder past FOLDERS (the last the file system lists, so `find` too) isn't looked at,
    // even with a `.git`, nor into; the first is.
    #[test]
    fn the_folder_past_the_ones_looked_at_is_left_out() {
        let base = scratch("past");
        for i in 0..=FOLDERS {
            std::fs::create_dir(base.join(format!("d{i:04}"))).unwrap();
        }
        let order: Vec<PathBuf> = std::fs::read_dir(&base).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(order.len(), FOLDERS + 1);
        mkdirs(&order[0], &[".git"]);
        mkdirs(&order[FOLDERS], &[".git", "sub"]);
        let first = order[0].file_name().unwrap().to_str().unwrap();
        LOOKED.with(|n| n.set(0));
        let got = both(&base, &[]);
        let looked = LOOKED.with(|n| n.get());
        assert_eq!((rels(&got), got.truncated), (vec![first], true));
        assert_eq!(looked, FOLDERS + 1);
    }

    // Past FOLDERS, no more is read: a space folder with tens of thousands
    // of folders lists quickly too.
    #[test]
    fn past_the_folders_looked_at_no_more_is_read() {
        let base = scratch("folders");
        // 50 folders of 99: 5000 to look at.
        for i in 0..50 {
            for j in 0..99 {
                std::fs::create_dir_all(base.join(format!("d{i:02}/c{j:02}"))).unwrap();
            }
        }
        mkdirs(&base, &["d00/c00/.git"]);
        let got = both(&base, &[]);
        assert_eq!((rels(&got), got.truncated), (vec!["d00/c00"], false), "exactly 5000");
        mkdirs(&base, &["e"]);
        assert!(both(&base, &[]).truncated, "5001");
        // 101 folders of 100: 10,201, of which only the first 5001 are looked at, in the file
        // system's order (the script too: `both`), not the first in path order. A repo in every
        // fourth folder's first: 26, about half of them in the folders looked at.
        for i in 50..101 {
            for j in 0..100 {
                std::fs::create_dir_all(base.join(format!("d{i:02}/c{j:02}"))).unwrap();
            }
        }
        for i in (0..101).step_by(4) {
            mkdirs(&base, &[&format!("d{i:02}/c00/.git")]);
        }
        LOOKED.with(|n| n.set(0));
        let got = both(&base, &[]);
        let looked = LOOKED.with(|n| n.get());
        assert!(got.truncated && !got.repos.is_empty() && got.repos.len() < 26, "{:?}", rels(&got));
        assert!(looked <= FOLDERS + 1, "looked at {looked} folders");
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
