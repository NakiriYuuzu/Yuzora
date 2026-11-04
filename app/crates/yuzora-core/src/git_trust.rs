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

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::remote::{self, ExecRequest};

/// Config keys (lowercased by git) that name a program run without asking.
pub const PROGRAMS: &str = r"^(filter\..+\.(clean|smudge|process)|core\.(fsmonitor|sshcommand|gitproxy|askpass|alternaterefscommand|alternaterefsprefixes)|credential(\..+)?\.helper|remotes?\..+|branch\..+\.(remote|pushremote)|url\..+\.(insteadof|pushinsteadof)|extensions\.partialclone|protocol(\..+)?\.allow|merge\..+\.driver|diff\.(external|.+\.(command|textconv))|gpg(\..+)?\.program|gpg\.ssh\.defaultkeycommand|submodule\..+\.(update|url|gitdir))$";

/// Whether a matched key's value runs something: fsmonitor as a boolean is
/// git's own daemon, and a submodule's update mode is a command only after
/// `!`. Only the exact values git takes that way pass (no trimming, ASCII
/// case as git compares); anything else counts. A URL counts when it names a
/// remote helper (`ext::` runs its command): a remote's or a submodule's,
/// and a remote's name too, which git takes for the URL when the remote has
/// none. So do the remote names that stand where a remote is wanted (a
/// branch's remote or push remote, the push default, a group's members),
/// and an `insteadOf` whose replacement a URL's rest can make into one
/// (`ext:` and `:sh …`). A partial clone counts whatever its URL
/// (`extensions.partialClone`, a remote's `promisor` or
/// `partialCloneFilter`): it fetches from its promisor remote as it reads.
pub fn names_program(key: &str, value: &str) -> bool {
    let any = |list: &[&str]| list.iter().any(|v| value.eq_ignore_ascii_case(v));
    if key == "core.fsmonitor" {
        return !any(&["", "true", "false", "yes", "no", "on", "off", "1", "0"]);
    }
    if key.starts_with("submodule.") && key.ends_with(".update") {
        return !any(&["checkout", "rebase", "merge", "none"]);
    }
    if key.starts_with("submodule.") && key.ends_with(".url") {
        return names_helper(value);
    }
    // Since git 2.54 (`extensions.submodulePathConfig`) a submodule's git dir is where this says.
    if key.starts_with("submodule.") && key.ends_with(".gitdir") {
        return true;
    }
    let branch_remote = key.starts_with("branch.") && (key.ends_with(".remote") || key.ends_with(".pushremote"));
    if branch_remote || key == "remote.pushdefault" {
        return names_helper(value);
    }
    // A partial clone: it fetches what it lacks from its promisor remote as it reads.
    if key == "extensions.partialclone" {
        return true;
    }
    if key.starts_with("remotes.") {
        // A group's members, split as git splits them.
        return value.split([' ', '\t', '\n']).any(names_helper);
    }
    // `remote.<name>.<key>`: the name may stand for the URL whatever the key.
    if let Some((name, var)) = key.strip_prefix("remote.").and_then(|k| k.rsplit_once('.')) {
        return names_helper(name) || matches!(var, "uploadpack" | "receivepack" | "vcs" | "partialclonefilter") || (var == "promisor" && git_true(value)) || (matches!(var, "url" | "pushurl") && names_helper(value));
    }
    if let Some(to) = key.strip_prefix("url.").and_then(|k| k.strip_suffix(".insteadof").or_else(|| k.strip_suffix(".pushinsteadof"))) {
        return may_name_helper(to);
    }
    true
}

/// A boolean as git reads it: false is `false`, `no`, `off`, empty or a
/// number that is 0 (a unit after it too); anything else (a key alone, a
/// value git can't read) is true.
fn git_true(value: &str) -> bool {
    let v = value.to_ascii_lowercase();
    !(matches!(v.as_str(), "false" | "no" | "off" | "") || v.trim_end_matches(['k', 'm', 'g']).parse::<i64>() == Ok(0))
}

/// A URL git hands to a remote helper, as git's `transport_get` reads it:
/// `<scheme>::…` (letters and digits, then `+`, `-` or `.` too; even none),
/// or `<scheme>://…` with a scheme git doesn't serve itself, which goes to
/// `git-remote-<scheme>`.
fn names_helper(url: &str) -> bool {
    let (scheme, rest) = url.split_at(scheme_len(url));
    rest.starts_with("::") || (!scheme.is_empty() && rest.starts_with("://") && !OWN_SCHEMES.contains(&scheme))
}

/// The `<scheme>://` git serves itself, compared as git does (case and all):
/// `HTTPS://` goes to `git-remote-HTTPS`.
const OWN_SCHEMES: [&str; 9] = ["file", "git", "ssh", "git+ssh", "ssh+git", "http", "https", "ftp", "ftps"];

/// Whether a URL starting with `start` may name a remote helper, whatever
/// follows: it does already, or its scheme (maybe empty) has at most one `:`
/// after it so far, or `:/` after one git doesn't serve itself.
fn may_name_helper(start: &str) -> bool {
    let (scheme, rest) = start.split_at(scheme_len(start));
    matches!(rest, "" | ":") || (rest == ":/" && !OWN_SCHEMES.contains(&scheme)) || names_helper(start)
}

fn scheme_len(url: &str) -> usize {
    url.char_indices().take_while(|&(i, c)| c.is_ascii_alphanumeric() || (i > 0 && "+-.".contains(c))).count()
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
/// its exit code and stdout, through [`whole`]; it's called once (on a
/// machine, one `hc exec`): [`SCAN_SCRIPT`] reads the repository's own
/// config first. A config git can't read counts as [`UNREADABLE`]. A runner
/// that ran out of time answers `(-1, [])`, as git failing: the gate stays
/// shut and the person can still trust the folder. Err only when git
/// couldn't run at all.
pub fn repo_programs<E>(run: impl FnMut(&[&str]) -> Result<(i32, Vec<u8>), E>) -> Result<Vec<String>, E> {
    scan_answer(run).map(|(keys, _)| keys)
}

/// [`repo_programs`], and the repository's git dir and common dir when the
/// scan found it plain ([`plain_layout`]): what [`programs_here_at`] needs
/// to keep the answer.
fn scan_answer<E>(mut run: impl FnMut(&[&str]) -> Result<(i32, Vec<u8>), E>) -> Result<(Vec<String>, Option<Dirs>), E> {
    let alias = format!("alias.{SCAN}=!{}", SCAN_SCRIPT.replace("@PROGRAMS@", PROGRAMS).replace("@SCAN@", SCAN));
    let (code, out) = run(&["-c", &alias, SCAN])?;
    // The config's answer, then the submodules', whatever the config said.
    let Some((config, config_code, subs)) = config_part(&out) else {
        return Ok((vec![UNREADABLE.into()], None));
    };
    // 1 with nothing printed: no key matched. Anything else: unknown.
    let mut keys = match config_code {
        0 => repo_keys(config, ""),
        1 if config.is_empty() => BTreeSet::new(),
        _ => return Ok((vec![UNREADABLE.into()], None)),
    };
    let whole = code == 0 && scanned_to_the_end(subs);
    if !whole {
        keys.insert(UNREADABLE.into());
    }
    keys.extend(repo_keys(subs, "?"));
    let layout = if whole { plain_layout(subs) } else { None };
    Ok((keys.into_iter().collect(), layout))
}

/// The top's git dir and common dir from the scan's `dirs` record, when its
/// records say the repository is plain: the top's own records only (its
/// `sub`, its remote files, `dirs`) and the end. A submodule, a git dir under
/// `modules/`, a gitlink in the index (`links`, checked out or not: one may
/// be checked out later) or anything git couldn't read isn't.
fn plain_layout(subs: &[u8]) -> Option<Dirs> {
    let text = std::str::from_utf8(subs).ok()?;
    let parts: Vec<&str> = text.split('\0').collect();
    let mut top = false;
    let mut dirs = None;
    for c in parts.chunks(2).filter(|c| c.len() == 2) {
        match (c[0], c[1]) {
            ("sub", "") if !top => top = true,
            ("legacy", _) if top => {}
            ("dirs", d) if top && dirs.is_none() => dirs = Some(d.split_once('\n')?),
            ("end", SCAN) => {}
            _ => return None,
        }
    }
    dirs.map(|(git_dir, common)| (git_dir.to_owned(), common.to_owned()))
}

/// A git dir and its common dir.
type Dirs = (String, String);

/// [`repo_programs`] for a repository on this computer at `top`, kept while
/// nothing it was read from changes: a later call looks at those files'
/// metadata ([`read_from`]; no git runs) and scans again only when one of
/// them changed or can't be looked at. `key` tells apart callers, and the
/// git they run. Kept only for a plain repository ([`plain_layout`],
/// [`read_from`]), and only when none of those files changed within
/// [`RACY`] before the scan began: a coarse file system clock could give a
/// later change the same time.
pub fn repo_programs_here<E>(key: &str, top: &str, run: impl FnMut(&[&str]) -> Result<(i32, Vec<u8>), E>) -> Result<Vec<String>, E> {
    programs_here_at(key, top, run, SystemTime::now())
}

/// [`repo_programs_here`] with the scan beginning at `began`.
fn programs_here_at<E>(key: &str, top: &str, run: impl FnMut(&[&str]) -> Result<(i32, Vec<u8>), E>, began: SystemTime) -> Result<Vec<String>, E> {
    let id = format!("{key}\0{top}");
    if let Some(keys) = kept_answer(&id, top) {
        return Ok(keys);
    }
    // The files as they were before the scan: the answer is kept only if
    // they still are after it. A `.git` swapped in meanwhile, its files'
    // times as old as they were, would otherwise pass for the one read.
    let before = found_dirs(Path::new(top)).and_then(|dirs| stamps_of(&dirs));
    let (keys, layout) = scan_answer(run)?;
    if let Some((git_dir, common)) = layout
        && let Some(before) = before
        && let Some(read) = read_from(&git_dir, &common)
        && (&read.dirs, &read.stamps) == (&before.dirs, &before.stamps)
        && read.latest.is_none_or(|t| began.duration_since(t).is_ok_and(|d| d >= RACY))
    {
        let mut all = kept().lock().unwrap_or_else(|e| e.into_inner());
        if all.len() >= KEEP_AT_MOST {
            all.clear();
        }
        all.insert(id, Kept { dirs: read.dirs, stamps: read.stamps, keys: keys.clone() });
    }
    Ok(keys)
}

/// How long before a scan the files it read must have last changed for its
/// answer to be kept: more than a coarse clock's step (FAT's is 2 s).
const RACY: Duration = Duration::from_secs(3);

/// How many folders' answers are kept at most; past that, all are dropped.
const KEEP_AT_MOST: usize = 64;

/// A folder's kept answer: the git dir and common dir git found from the
/// top, and the files it was read from, as they were.
struct Kept {
    dirs: (PathBuf, PathBuf),
    stamps: Vec<(PathBuf, Look, Stamp)>,
    keys: Vec<String>,
}

fn kept() -> &'static Mutex<HashMap<String, Kept>> {
    static KEPT: OnceLock<Mutex<HashMap<String, Kept>>> = OnceLock::new();
    KEPT.get_or_init(Mutex::default)
}

/// The kept answer for `id` if the top still leads to the same git dir and
/// common dir, and every file it was read from is as it was; else none,
/// and it's dropped.
fn kept_answer(id: &str, top: &str) -> Option<Vec<String>> {
    let mut all = kept().lock().unwrap_or_else(|e| e.into_inner());
    let entry = all.get(id)?;
    if found_dirs(Path::new(top)).as_ref() == Some(&entry.dirs) && entry.stamps.iter().all(|(path, look, was)| stamp(path, *look).as_ref() == Some(was)) {
        return Some(entry.keys.clone());
    }
    all.remove(id);
    None
}

/// The files a plain repository's answer came from, as they are now, and the
/// latest time one of them changed; none when the answer can't be kept.
///
/// - Which repository git finds from the top: a later look reads the top's
///   `.git` (a folder, a gitfile, a link) and the git dir's `commondir`
///   again ([`found_dirs`]; a link on the way may change while both files
///   stay as they were), which must lead to these dirs; and git takes that
///   git dir only with its `HEAD`, and the common dir's `objects/` and
///   `refs/` (`is_git_directory`), else it looks further up.
/// - Its own config, the scopes the gate counts: the common dir's `config`
///   and the git dir's `config.worktree`. Neither may name an `include` (any
///   `include` in them, as bytes, whatever case): an included file could be
///   anywhere and change unseen.
/// - The index (`ls-files -s`; a gitlink in it already isn't plain), which
///   must hold all its entries itself ([`index_stands_alone`]), the
///   common dir's `remotes/` and `branches/` (their listing too), and no
///   `modules/` in either dir.
///
/// The person's own config (global, system, from the environment) isn't
/// here: the gate counts only the repository's scopes, which those can't
/// add to; at most they make git fail to read the repository, and then
/// every git command there fails too.
fn read_from(git_dir: &str, common: &str) -> Option<Read> {
    let dirs = (std::fs::canonicalize(git_dir).ok()?, std::fs::canonicalize(common).ok()?);
    let read = stamps_of(&dirs)?;
    let (git_dir, common) = (dirs.0.as_path(), dirs.1.as_path());
    let missing = |path: PathBuf| read.stamps.iter().any(|(p, _, s)| *p == path && *s == Stamp::Missing);
    if !missing(git_dir.join("modules")) || !missing(common.join("modules")) {
        return None;
    }
    for config in [common.join("config"), git_dir.join("config.worktree")] {
        if read_plain(&config, ANSWER_CAP as u64)?.is_some_and(|bytes| bytes.windows(7).any(|w| w.eq_ignore_ascii_case(b"include"))) {
            return None;
        }
    }
    if let Some(bytes) = read_plain(&git_dir.join("index"), INDEX_CAP)?
        && !index_stands_alone(&bytes)?
    {
        return None;
    }
    Some(read)
}

/// The largest index [`read_from`] reads (a bigger one isn't kept).
const INDEX_CAP: u64 = 256 << 20;

/// The bytes of `path` when it is a regular file (or a link to one) of at
/// most `cap` bytes; `Some(None)` when it isn't there. None for anything
/// else: a FIFO would wait for a writer, a device might never end. Opened
/// without waiting, then checked.
fn read_plain(path: &Path, cap: u64) -> Option<Option<Vec<u8>>> {
    let mut options = std::fs::File::options();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(None),
        Err(_) => return None,
    };
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, cap + 1), &mut bytes).ok()?;
    (bytes.len() as u64 <= cap).then_some(Some(bytes))
}

/// The fingerprint files of a git dir and common dir ([`read_from`] says
/// which) as they are now, and the latest time one of them changed.
fn stamps_of(dirs: &(PathBuf, PathBuf)) -> Option<Read> {
    let (git_dir, common) = (dirs.0.as_path(), dirs.1.as_path());
    let files = [
        (git_dir.join("HEAD"), Look::Whole),
        (git_dir.join("config.worktree"), Look::Whole),
        (git_dir.join("index"), Look::Whole),
        (git_dir.join("modules"), Look::Whole),
        (common.join("config"), Look::Whole),
        (common.join("objects"), Look::Kind),
        (common.join("refs"), Look::Kind),
        (common.join("remotes"), Look::Whole),
        (common.join("branches"), Look::Whole),
        (common.join("modules"), Look::Whole),
    ];
    let mut read = Read { dirs: dirs.clone(), stamps: Vec::new(), latest: None };
    for (path, look) in files {
        let (stamp, changed) = stamp_and_time(&path, look)?;
        read.latest = read.latest.max(changed);
        read.stamps.push((path, look, stamp));
    }
    Some(read)
}

/// Whether an index file holds all its entries itself: no split index (a
/// `link` extension; entries in a `sharedindex.*` file) and no sparse
/// directories (`sdir`; git expands them from trees). Either reads other
/// files, which may change while the index doesn't (git doesn't check them
/// against their names). None when it doesn't read as an index (with SHA-1
/// or SHA-256 ids, version 2 to 4), or reads as one either way: which one
/// git takes is the repository's, and the two may differ in extensions.
fn index_stands_alone(bytes: &[u8]) -> Option<bool> {
    let mut reads = [20, 32].into_iter().filter_map(|id_len| index_extensions(bytes, id_len));
    let ext = reads.next()?;
    if reads.next().is_some() {
        return None;
    }
    Some(!ext.iter().any(|e| e == b"link" || e == b"sdir"))
}

/// The extensions of an index whose ids are `id_len` bytes long, when it
/// reads as one to its trailing checksum.
fn index_extensions(bytes: &[u8], id_len: usize) -> Option<Vec<[u8; 4]>> {
    let be32 = |at: usize| bytes.get(at..at.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(..4)? != b"DIRC" {
        return None;
    }
    let version = be32(4)?;
    if !(2..=4).contains(&version) {
        return None;
    }
    let mut at = 12usize;
    // Version 4: the last path's length (each path drops some of the last one's end).
    let mut last = 0usize;
    for _ in 0..be32(8)? {
        // Times, device, inode, mode, owner, size; the id; the flags.
        let flags_at = at.checked_add(40 + id_len)?;
        let flags = u16::from_be_bytes(bytes.get(flags_at..flags_at + 2)?.try_into().ok()?);
        let mut path = flags_at + 2;
        if version >= 3 && flags & 0x4000 != 0 {
            path += 2;
        }
        let mut drop = 0usize;
        if version == 4 {
            // How much of the last path to drop, in 7-bit groups as git writes it.
            let mut byte = *bytes.get(path)?;
            drop = usize::from(byte & 127);
            while byte & 128 != 0 {
                path += 1;
                byte = *bytes.get(path)?;
                drop = drop.checked_add(1)?.checked_mul(128)?.checked_add(usize::from(byte & 127))?;
            }
            path += 1;
        }
        let end = path + bytes.get(path..)?.iter().position(|b| *b == 0)?;
        let len = if version == 4 { last.checked_sub(drop)? + (end - path) } else { end - path };
        // The flags hold the path's length (up to 0xfff): an entry read from
        // the wrong place doesn't match it.
        if usize::from(flags & 0xfff) != len.min(0xfff) {
            return None;
        }
        last = len;
        // Versions 2 and 3 pad each entry with 1 to 8 NULs to a multiple of 8.
        at = if version == 4 { end + 1 } else { at + ((end - at + 8) & !7) };
    }
    let checksum = bytes.len().checked_sub(id_len)?;
    let mut ext = Vec::new();
    while at < checksum {
        ext.push(bytes.get(at..at + 4)?.try_into().ok()?);
        at = at.checked_add(8)?.checked_add(be32(at + 4)? as usize)?;
    }
    (at == checksum).then_some(ext)
}

/// The git dir and common dir the top's `.git` leads to, as git reads them
/// (real paths): the folder (or what a link leads to), or the git dir a
/// gitfile names (`gitdir: <path>`); then that git dir's `commondir` (a
/// path), or the git dir itself without one. A path in either file counts
/// from that file's folder unless absolute, line ends dropped. None when
/// they aren't there or can't be read.
fn found_dirs(top: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot = top.join(".git");
    let git_dir = if std::fs::metadata(&dot).ok()?.is_dir() { std::fs::canonicalize(&dot).ok()? } else { named_in(&dot, b"gitdir: ", top)? };
    let common = match std::fs::symlink_metadata(git_dir.join("commondir")) {
        Ok(_) => named_in(&git_dir.join("commondir"), b"", &git_dir)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(_) => return None,
    };
    Some((git_dir, common))
}

/// The real path named in `file` after `prefix`, from `from` unless absolute.
fn named_in(file: &Path, prefix: &[u8], from: &Path) -> Option<PathBuf> {
    // git reads no gitfile past 1 MiB.
    let text = read_plain(file, 1 << 20)??;
    let named = text.strip_prefix(prefix)?;
    let end = named.iter().rposition(|b| *b != b'\n' && *b != b'\r')? + 1;
    let named = Path::new(std::str::from_utf8(&named[..end]).ok()?);
    std::fs::canonicalize(if named.is_absolute() { named.to_owned() } else { from.join(named) }).ok()
}

struct Read {
    dirs: (PathBuf, PathBuf),
    stamps: Vec<(PathBuf, Look, Stamp)>,
    latest: Option<SystemTime>,
}

/// How much of a file [`stamp`] looks at.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Look {
    /// Which file it is, its size and its times: any change shows.
    Whole,
    /// A folder: which one it is (it's there, and the same one); else as `Whole`.
    Kind,
}

/// What a file is, as far as a later look can tell it changed: missing, or
/// its metadata (a link's, and what it leads to).
#[derive(Clone, PartialEq, Debug)]
enum Stamp {
    Missing,
    Seen(Vec<i128>),
}

/// The file's [`Stamp`], or none when it can't be looked at.
fn stamp(path: &Path, look: Look) -> Option<Stamp> {
    stamp_and_time(path, look).map(|(stamp, _)| stamp)
}

/// [`stamp`], and the latest time the file (or what a link leads to)
/// changed, as far as the stamp covers it.
fn stamp_and_time(path: &Path, look: Look) -> Option<(Stamp, Option<SystemTime>)> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some((Stamp::Missing, None)),
        Err(_) => return None,
    };
    let (mut facts, mut latest) = metadata_facts(&meta, look)?;
    if meta.file_type().is_symlink() {
        match std::fs::metadata(path) {
            Ok(to) => {
                let (more, changed) = metadata_facts(&to, look)?;
                facts.extend(more);
                latest = latest.max(changed);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => facts.push(-1),
            Err(_) => return None,
        }
    }
    Some((Stamp::Seen(facts), latest))
}

/// The numbers that tell a file apart from what it was: which file it is
/// (type, mode, owner, device, inode) and, unless `look` is `Kind` and it's
/// a folder, its size and change times (`mtime`, `ctime`, to the
/// nanosecond); with the latest of those times.
#[cfg(unix)]
fn metadata_facts(meta: &std::fs::Metadata, look: Look) -> Option<(Vec<i128>, Option<SystemTime>)> {
    use std::os::unix::fs::MetadataExt;
    let mut facts = vec![i128::from(meta.mode()), i128::from(meta.uid()), i128::from(meta.dev()), i128::from(meta.ino())];
    if look == Look::Kind && meta.is_dir() {
        return Some((facts, None));
    }
    let (modified, changed) = (since_1970(meta.mtime(), meta.mtime_nsec())?, since_1970(meta.ctime(), meta.ctime_nsec())?);
    facts.extend([i128::from(meta.size()), i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()), i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec())]);
    Some((facts, Some(modified.max(changed))))
}

/// A time as `SystemTime`, from seconds and nanoseconds since 1970.
#[cfg(unix)]
fn since_1970(s: i64, ns: i64) -> Option<SystemTime> {
    let ns = Duration::from_nanos(u64::try_from(ns).ok()?);
    let secs = Duration::from_secs(s.unsigned_abs());
    if s >= 0 { SystemTime::UNIX_EPOCH.checked_add(secs)?.checked_add(ns) } else { SystemTime::UNIX_EPOCH.checked_sub(secs)?.checked_add(ns) }
}

/// Elsewhere nothing is kept. On Windows a file rewritten in place, its
/// size kept and its times set back, looks as it was: every time it has
/// (its change time too) is one a program may set.
#[cfg(not(unix))]
fn metadata_facts(_: &std::fs::Metadata, _: Look) -> Option<(Vec<i128>, Option<SystemTime>)> {
    None
}

/// The scan's answer cut where its record of the config's exit code
/// (`config\0<code>\0`) starts a pair: what `git config` printed before it
/// (its records start with a scope, never `config`), the code, and the
/// scan's records after it. None without one: the scan never got that far,
/// or the config's answer stopped part way (count it unreadable).
fn config_part(out: &[u8]) -> Option<(&[u8], i32, &[u8])> {
    let mut at = 0;
    loop {
        let first = at + out[at..].iter().position(|b| *b == 0)?;
        let second = first + 1 + out[first + 1..].iter().position(|b| *b == 0)?;
        if &out[at..first] == b"config" {
            let code = std::str::from_utf8(&out[first + 1..second]).ok()?.parse().ok()?;
            return Some((&out[..at], code, &out[second + 1..]));
        }
        at = second + 1;
    }
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

/// First the repository's config read from the top level, as the gate always
/// read it: its program keys in all scopes as `git config --show-scope -z`
/// prints them, then `config\0<its exit code>\0` ([`config_part`]). Then each checked-out
/// gitlink in the index, nested ones too, and its config's
/// program keys: `sub\0path\0` then the records, `err\0what\0` when it can't
/// be read, and `end\0<SCAN>\0` last. The top's own records also say
/// where its repository is, `dirs\0<git dir>\n<common dir>\0`, and
/// `links\0\0` when its index has a gitlink ([`plain_layout`]). git sets
/// `GIT_DIR` for the alias when the top's `.git` is a gitfile (a linked
/// worktree, a submodule as the top): the config is read through it, the
/// repository git found; past the top the script unsets it. It follows git: a gitlink is checked
/// out when its folder has a `.git`, and git works in it with `GIT_DIR=.git`
/// (never a repo further up), so its config is read that way, and its own
/// gitlinks are where its work tree is (its `core.worktree`, from its config
/// or its gitdir's). grep picks the gitlinks out of git's list, and must pass
/// on a last line added to it: a grep that failed would print nothing, which
/// would read as no gitlinks. Each repo (git dir and work tree) is read once,
/// in one shell that remembers them: met again below itself, it is a way
/// back up that git would follow without end, and counts as unreadable; met
/// again elsewhere, its keys are in already. Like git, it looks from inside
/// each work tree, by the gitlink's name as it is (one path from the root
/// could pass PATH_MAX where git's doesn't), and a `.git` it can't see is
/// passed over only when the gitlink is surely not there: a folder on its way
/// can be entered and lacks the next part, or is a file (`gone` goes into
/// that folder; each `enter` starts with its own `cd`). A quoted path (a
/// `"`, `\` or control character), an absolute one (git goes there; the scan
/// would look under the top), one git never writes (a `.`, `..` or empty
/// part), on a Windows host one Win32 reads otherwise (a `:`, a drive or a
/// stream; a part ending in `.` or a space, which it drops), a gitlink that
/// may be there but can't be looked at, a work tree whose name has a line
/// feed (`$(…)` drops one at the end,
/// so the scan would read another folder: `rev-parse`'s answer is taken with
/// a `.` after it, and must be two lines then), and more than 16 levels can't
/// be followed: all count as unreadable too. `GIT_NO_LAZY_FETCH` (and an
/// empty `GIT_ALLOW_PROTOCOL` for a git before 2.44, which ignores it):
/// listing a sparse index in a partial clone never fetches with the repo's
/// ssh. Each repo's remote files (`remotes/`, `branches/` in its common
/// dir) count too: git reads remotes from them besides the config. So does
/// each git dir under `modules/` (nested ones too) in its git dir and its
/// common dir, as `.git/modules/<name>` (a linked worktree keeps its own,
/// `.git/worktrees/<id>/modules/<name>`), with the remote files of its
/// common dir: a submodule not checked out keeps one there, which `submodule
/// update` reuses and a fetch recurses into. git says which folders are git
/// dirs and reads their config (`GIT_DIR`, with a work tree of its own so a
/// missing one doesn't stop it): any folder there with a `HEAD` (as the file
/// system opens it: `head` where case doesn't count, a link that leads
/// nowhere) and a `commondir` or `objects/` and `refs/` is asked, which is
/// all git's own test (`is_git_directory`) can take. So does any file there
/// that starts `gitdir: ` (a gitfile, or a link to one, up to 2 MiB: git
/// reads none past 1 MiB), which git follows when it opens a submodule in
/// its own process (a push that checks them does); the git dir it names
/// counts, and that git dir's own `modules/` in turn. A link to a gitfile
/// names two: `GIT_DIR` reads the path in it from the link's folder, and git
/// opening a submodule in its own process takes the real path first, so from
/// the file's own folder; both count. One git won't take, and a path whose
/// real one can't be had (no `readlink`, links 40 deep), count as
/// unreadable. They're read last, and one the scan already read as a
/// checked-out submodule keeps that name. A link there the kernel can't
/// follow (so neither can `find -L`), and a `modules` that isn't a folder it
/// can, count as unreadable too: the real path git takes may still get
/// through one (`x/..` past a file, a trailing slash, more links). A path
/// there `find` gives that can't be read back (a line feed in it) and a
/// `find` that fails count as unreadable.
const SCAN_SCRIPT: &str = r#"unset CDPATH GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_PREFIX
GIT_NO_LAZY_FETCH=1 GIT_ALLOW_PROTOCOL=
export GIT_NO_LAZY_FETCH GIT_ALLOW_PROTOCOL
git config --show-scope -z --get-regexp '@PROGRAMS@'
printf 'config\0%s\0' $?
tab=$(printf '\t') nl='
'
IFS=$nl
mseen=$nl mods=
set -f
case ${OS-}:$(uname -s 2>/dev/null) in Windows_NT:*|*:CYGWIN*|*:MINGW*|*:MSYS*) win=1 ;; *) win= ;; esac
where() {
  case "$1" in *"$nl"*"$nl"*"$nl"*) return 1 ;; ?*"$nl"?*"$nl.") ;; *) return 1 ;; esac
  set -- "${1%"$nl."}"
  gd=${1%%"$nl"*}
  top=${1#*"$nl"}
  node="${nl}g:$gd${nl}t:$top$nl"
  [ -d "$top" ]
}
scan() {
  [ "$2" -lt 16 ] || { printf 'sub\0%s\0err\0depth\0' "$1"; return; }
  links=$( { GIT_DIR=$3 GIT_WORK_TREE=$4 git -C "$4" -c core.quotepath=false ls-files -s || echo '#failed'; echo '160000 #end'; } | LC_ALL=C grep -e '^160000 ' -e '^#failed$') || links=
  case "$nl$links" in
    *"$nl#failed$nl"*) links= ;;
    *"${nl}160000 #end") ;;
    *) links= ;;
  esac
  [ -n "$links" ] || { printf 'sub\0%s\0err\0index\0' "${1:-.}"; return; }
  [ "$2" -gt 0 ] || [ "$links" = '160000 #end' ] || printf 'links\0\0'
  for line in $links; do
    [ "$line" = '160000 #end' ] || enter "$1" "$2" "${4%/}" "${line#*"$tab"}" "$5"
  done
}
files() {
  for d in remotes branches; do
    if [ -d "$1/$d" ] && [ ! -L "$1/$d" ]; then
      l=$(ls -A "$1/$d" && echo .) || l=x
      [ "$l" = . ] || printf 'legacy\0.git/%s\0' "$d"
    elif [ -e "$1/$d" ] || [ -L "$1/$d" ]; then
      printf 'legacy\0.git/%s\0' "$d"
    fi
  done
}
has() { [ -e "$1" ] || [ -L "$1" ]; }
real() {
  rp=$1 rk=0
  while [ -L "$rp" ]; do
    [ $rk -lt 40 ] || return 1
    rl=$(readlink "$rp" && echo .) || return 1
    rl=${rl%"$nl."} rk=$((rk+1))
    case "$rl" in /*) rp=$rl ;; *) rp=${rp%/*}/$rl ;; esac
  done
  rd=$(cd -P "${rp%/*}/" && pwd -P && echo .) || return 1
  rd=${rd%"$nl."}
  rp=${rd%/}/${rp##*/}
}
queue() {
  mods=$mods+$1$nl$2$nl$2$nl
  [ "$3" = "$2" ] || mods=$mods+$1$nl$2$nl$3$nl
}
gitdir() {
  w=$(GIT_DIR=$3 GIT_WORK_TREE=$4 git rev-parse --path-format=absolute --git-common-dir --absolute-git-dir && echo .) || w=
  case "$w" in *"$nl"*"$nl"*"$nl"*) w= ;; ?*"$nl"?*"$nl.") w=${w%"$nl."} ;; *) w= ;; esac
  [ -n "$w" ] || { printf 'sub\0%s%s\0err\0gitdir\0' "$1" "$2"; return 1; }
  mg=${w#*"$nl"} w=${w%%"$nl"*}
  case "$mseen" in *"$nl$mg$nl"*) return 1 ;; esac
  mseen=$mseen$mg$nl
  printf 'sub\0%s%s\0' "$1" "$2"
  GIT_DIR=$3 GIT_WORK_TREE=$4 git config --show-scope -z --get-regexp '@PROGRAMS@'
  r=$?
  if [ $r -gt 1 ]; then printf 'err\0%s\0' $r; fi
  files "$w"
}
modules() {
  case "$3" in "$2") n=.git ;; "$2"/*) n=.git/${3#"$2/"} ;; *) n=$3 ;; esac
  [ -d "$3/modules" ] || { has "$3/modules" && printf 'sub\0%s%s/modules\0err\0modules\0' "$1" "$n"; return; }
  dirs=$(find -L "$3/modules" -type d && echo .) || dirs=
  gfs=$(find -L "$3/modules" -type f -size -4097 -exec sh -c 'grep -l "^gitdir: " "$@"; [ $? -le 1 ]' sh {} + && echo .) || gfs=
  lns=$(find -L "$3/modules" -type l && echo .) || lns=
  case "$dirs" in .|*"$nl.") ;; *) dirs= ;; esac
  case "$gfs" in .|*"$nl.") ;; *) gfs= ;; esac
  case "$lns" in .|*"$nl.") ;; *) lns= ;; esac
  [ -n "$dirs" ] && [ -n "$gfs" ] && [ -n "$lns" ] || { printf 'sub\0%s%s/modules\0err\0modules\0' "$1" "$n"; return; }
  for m in $lns; do
    [ "$m" != . ] || continue
    case "$m" in "$3/modules/"*) [ -L "$m" ] ;; *) false ;; esac || { printf 'sub\0%s%s/modules\0err\0name\0' "$1" "$n"; continue; }
    printf 'sub\0%s%s\0err\0gitdir\0' "$1" "$n/${m#"$3/"}"
  done
  for m in $dirs; do
    [ "$m" != . ] || continue
    case "$m" in "$3/modules"|"$3/modules/"*) [ -d "$m" ] ;; *) false ;; esac || { printf 'sub\0%s%s/modules\0err\0name\0' "$1" "$n"; continue; }
    has "$m/HEAD" && { has "$m/commondir" || { has "$m/objects" && has "$m/refs"; }; } || continue
    gitdir "$1" "$n/${m#"$3/"}" "$m" "$m"
  done
  for m in $gfs; do
    [ "$m" != . ] || continue
    case "$m" in "$3/modules/"*) [ -f "$m" ] ;; *) false ;; esac || { printf 'sub\0%s%s/modules\0err\0name\0' "$1" "$n"; continue; }
    first=
    IFS= read -r first < "$m"
    case "$first" in "gitdir: "*) ;; *) continue ;; esac
    ml=$n/${m#"$3/"}
    real "$m" || { printf 'sub\0%s%s\0err\0gitdir\0' "$1" "$ml"; continue; }
    gitdir "$1" "$ml" "$rp" "${rp%/*}" && queue "$1$ml/" "$w" "$mg"
    [ "$rp" = "$m" ] || { gitdir "$1" "$ml" "$m" "${m%/*}" && queue "$1$ml/" "$w" "$mg"; }
  done
}
legacy() {
  p=$1
  shift
  c=$("$@" rev-parse --path-format=absolute --git-common-dir --absolute-git-dir && echo .) || c=
  case "$c" in *"$nl"*"$nl"*"$nl"*) printf 'err\0commondir\0'; return ;; ?*"$nl"?*"$nl.") c=${c%"$nl."} ;; *) printf 'err\0commondir\0'; return ;; esac
  g=${c#*"$nl"} c=${c%%"$nl"*}
  [ -n "$p" ] || printf 'dirs\0%s\0' "$g$nl$c"
  files "$c"
  queue "$p" "$c" "$g"
}
gone() {
  case "$1" in */*) up=${1%/*} ;; *) up=. ;; esac
  if [ -d "$up" ]; then
    cd "./$up" 2>/dev/null && [ ! -e "./${1##*/}" ] && [ ! -L "./${1##*/}" ]
  elif [ -e "$up" ]; then
    :
  else
    [ ! -L "$up" ] && gone "$up"
  fi
}
enter() {
  case "/$4/" in /\"*|*/./*|*/../*|*//*) printf 'sub\0%s%s\0err\0name\0' "$1" "$4"; return ;; esac
  if [ -n "$win" ]; then
    case "/$4/" in *:*|*./*|*" /"*) printf 'sub\0%s%s\0err\0name\0' "$1" "$4"; return ;; esac
  fi
  cd "$3/" || { printf 'sub\0%s%s\0err\0path\0' "$1" "$4"; return; }
  if [ ! -e "$4/.git" ]; then
    gone "$4/.git" || printf 'sub\0%s%s\0err\0path\0' "$1" "$4"
    return
  fi
  printf 'sub\0%s%s\0' "$1" "$4"
  GIT_DIR=.git git -C "$4" config --show-scope -z --get-regexp '@PROGRAMS@'
  c=$?
  if [ $c -gt 1 ]; then printf 'err\0%s\0' $c; return; fi
  w=$(GIT_DIR=.git git -C "$4" rev-parse --absolute-git-dir --show-toplevel && echo .) && where "$w" || { printf 'err\0worktree\0'; return; }
  mseen=$mseen$gd$nl
  case "$5" in *"$node"*) printf 'err\0cycle\0'; return ;; esac
  case "$seen" in *"$node"*) return ;; esac
  seen=$seen$node
  legacy "$1$4/" env GIT_DIR=.git git -C "$4"
  scan "$1$4/" $(($2 + 1)) "$gd" "$top" "$5$node"
}
if w=$(git rev-parse --absolute-git-dir --show-toplevel && echo .) && where "$w"; then
  unset GIT_DIR GIT_WORK_TREE
  seen=$node
  printf 'sub\0\0'
  legacy "" git
  scan "" 0 "$gd" "$top" "$node"
  while [ -n "$mods" ]; do
    mp=${mods%%"$nl"*}
    mods=${mods#*"$nl"}
    mc=${mods%%"$nl"*}
    mods=${mods#*"$nl"}
    mm=${mods%%"$nl"*}
    mods=${mods#*"$nl"}
    modules "${mp#+}" "$mc" "$mm"
  done
else
  printf 'sub\0.\0err\0worktree\0'
fi
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
            // Files git reads remotes from besides the config (any there counts).
            "legacy" => {
                keys.insert(if place.is_empty() { c[1].to_owned() } else { format!("{place}: {}", c[1]) });
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
/// run or answer (out of time is [`UNREADABLE`]); count the folder as not safe.
pub fn programs_in(machine: Option<&str>, top: &str) -> Result<Vec<String>, String> {
    programs_within(machine, top, TIMEOUT)
}

/// [`programs_in`] within `timeout`; here, kept ([`repo_programs_here`]).
fn programs_within(machine: Option<&str>, top: &str, timeout: Duration) -> Result<Vec<String>, String> {
    let machine = machine.filter(|m| !m.is_empty());
    let run = |args: &[&str]| {
        let req = ExecRequest::new("git", READ_FLAGS.iter().copied().chain(args.iter().copied())).on(machine).cwd(Some(top)).timeout(timeout).max_output(ANSWER_CAP as u64 + 1);
        match remote::run(&req) {
            Ok(o) => Ok(whole(o.code, o.stdout)),
            // Out of time: unreadable, which the person may still trust.
            Err(remote::ExecError::TimedOut { .. }) => Ok((-1, Vec::new())),
            Err(e) => Err(e.to_string()),
        }
    };
    if machine.is_some() { repo_programs(run) } else { repo_programs_here("yuzora-core:git", top, run) }
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

/// Adds a folder id to `file` (the Git sidebar's "Trust"). One at a time,
/// threads and processes alike (in this process first, then under a lock on
/// [`lock_file`]), so no folder trusted at the same moment is lost; and the
/// new list takes the old one's place whole ([`replace`]), so a reader (every
/// git command reads it) never meets one cut short. A file system without
/// locks changes it with this process's lock only. A list there that can't
/// be read whole (unreadable, or not a list) is left as it is and the trust
/// fails: writing would drop every folder in it.
pub fn trust_at(file: &Path, id: &str) -> std::io::Result<()> {
    add_to_list(file, id, std::fs::File::lock)
}

/// [`trust_at`], with `lock` taking the lock file's lock.
fn add_to_list(file: &Path, id: &str, lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    static HERE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _here = HERE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let held = std::fs::File::options().create(true).write(true).truncate(false).open(lock_file(file))?;
    if let Err(e) = lock(&held)
        && e.kind() != std::io::ErrorKind::Unsupported
    {
        return Err(e);
    }
    // No list only when there's no file yet ([`trusted_from`] takes any
    // trouble for none, the gate shut: right for a reader, not for this).
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", file.display());
    let mut all: BTreeSet<String> = match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str::<Vec<String>>(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, at(&e)))?.into_iter().collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => return Err(std::io::Error::new(e.kind(), at(&e))),
    };
    all.insert(id.to_owned());
    let list: Vec<&String> = all.iter().collect();
    replace(file, &serde_json::to_string_pretty(&list).unwrap_or_default())
}

/// Writes `text` beside `file` (`<file>.<pid>.tmp`, on disk before it moves),
/// then puts it in `file`'s place whole. A step that fails leaves nothing
/// beside it.
fn replace(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut next = file.as_os_str().to_owned();
    next.push(format!(".{}.tmp", std::process::id()));
    let next = PathBuf::from(next);
    let done = std::fs::File::create(&next).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all())).and_then(|()| std::fs::rename(&next, file));
    if done.is_err() {
        let _ = std::fs::remove_file(&next);
    }
    done
}

/// What [`trust_at`] locks while it changes `file`: `<file>.lock` beside it.
fn lock_file(file: &Path) -> PathBuf {
    let mut lock = file.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
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
        // A remote helper (`<scheme>::…`; `ext::` runs its command) as a remote's URL, or as what insteadOf makes of one.
        assert!(names_program("remote.origin.url", "ext::sh -c touch% x") && names_program("remote.o.pushurl", "Ext::x") && names_program("remote.o.url", "fd::0") && names_program("remote.o.url", "1x+y::z") && names_program("remote.o.url", "::z"));
        assert!(!names_program("remote.o.url", "https://example.com/r.git") && !names_program("remote.o.url", "git@host:r.git") && !names_program("remote.o.url", "/srv/r") && !names_program("remote.o.url", "c:/srv/r") && !names_program("remote.o.url", ".x::y") && !names_program("remote.o.url", "a b::y"));
        assert!(names_program("url.ext::sh -c x.insteadof", "https://") && names_program("url.ext::x.pushinsteadof", "git@") && !names_program("url.https://mirror/.insteadof", "https://"));
        // An insteadOf whose replacement a URL's rest can make one (`ext:` + `:sh …`).
        assert!(names_program("url.ext:.insteadof", "P") && names_program("url.ext.insteadof", "P") && names_program("url..insteadof", "P") && names_program("url.x+y:.pushinsteadof", "P"));
        assert!(!names_program("url.git@github.com:.insteadof", "gh:") && !names_program("url.ssh://host/.insteadof", "h:") && !names_program("url.a:b.insteadof", "P"));
        // The promisor remote of a partial clone, by name or as a URL.
        assert!(names_program("extensions.partialclone", "ext::sh -c x") && names_program("extensions.partialclone", "origin"));
        // A partial clone fetches what it lacks from its promisor remote as it reads.
        assert!(names_program("remote.origin.promisor", "true") && names_program("remote.origin.promisor", "Yes") && names_program("remote.origin.promisor", "1"));
        assert!(!names_program("remote.origin.promisor", "false") && !names_program("remote.origin.promisor", "off") && !names_program("remote.origin.promisor", "") && !names_program("remote.origin.promisor", "0"));
        assert!(names_program("remote.origin.partialclonefilter", "blob:none") && !names_program("remote.origin.fetch", "+refs/*:refs/*"));
        // Since git 2.54 a submodule's git dir may be anywhere config says.
        assert!(names_program("submodule.x.gitdir", ".git/modules/x"));
        // A remote's name, which stands for its URL when it has none, whatever the key.
        assert!(names_program("remote.ext::sh -c x.fetch", "+refs/heads/*:refs/remotes/x/*") && names_program("remote.zz::p.promisor", "true") && names_program("remote.a.b.vcs", "hg"));
        assert!(!names_program("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*") && !names_program("remote.team/a.b.prune", "true") && !names_program("remote.origin.tagopt", "--no-tags"));
        // Remote names where a remote is wanted, and a submodule's URL.
        assert!(names_program("branch.main.remote", "zz::p") && names_program("branch.a.b.pushremote", "Ext::x") && names_program("remote.pushdefault", "ext::x"));
        assert!(!names_program("branch.main.remote", "origin") && !names_program("branch.main.pushremote", ".") && !names_program("remote.pushdefault", "fork"));
        assert!(names_program("remotes.all", "origin\text::x") && !names_program("remotes.all", "origin  upstream"));
        assert!(names_program("submodule.s.url", "ext::sh -c x") && !names_program("submodule.s.url", "../s.git"));
        // `<scheme>://` with a scheme git doesn't serve itself goes to `git-remote-<scheme>` (case counts, as in git).
        assert!(names_program("remote.o.url", "zz://payload") && names_program("remote.o.url", "ext://x/run") && names_program("remote.o.url", "HTTPS://h/r") && names_program("remote.zz://p.fetch", "+refs/heads/*:refs/remotes/x/*"));
        assert!(["https://h/r", "http://h/r", "ssh://h/r", "git://h/r", "git+ssh://h/r", "ssh+git://h/r", "file:///srv/r", "ftp://h/r", "ftps://h/r"].iter().all(|u| !names_program("remote.o.url", u)));
        assert!(names_program("extensions.partialclone", "zz://payload") && names_program("url.zz://p/.insteadof", "https://") && names_program("url.zz:/.insteadof", "P"));
        assert!(!names_program("url.https:/.insteadof", "P") && !names_program("url.https://mirror/.insteadof", "https://"));
    }

    #[test]
    fn records_and_exit_codes_read_as_the_gate_needs() {
        // One call (on a machine, one `hc exec`): the scan's shell answers
        // what `git config` printed, `config\0<its code>\0`, then the scan's records.
        let raw = |code: i32, out: &str| {
            let mut calls = 0;
            let keys = repo_programs(|args| {
                calls += 1;
                assert_eq!(args[0], "-c");
                Ok::<_, ()>((code, out.as_bytes().to_vec()))
            })
            .unwrap();
            assert_eq!(calls, 1);
            keys
        };
        let answer = |config: (i32, &str), subs: (i32, &str)| raw(subs.0, &format!("{}config\0{}\0{}", config.1, config.0, subs.1));
        // The scan's last record: an answer without it was cut short.
        const END: &str = "end\0yuzora-trust-scan\0";
        // No key: nothing pending.
        assert_eq!(answer((1, ""), (0, END)), Vec::<String>::new());
        // An unreadable config shuts the gate, whatever the scan found.
        assert_eq!(answer((128, ""), (0, "")), [UNREADABLE]);
        assert_eq!(answer((128, ""), (0, &format!("sub\0vendor/a\0local\0filter.y.clean\ncat\0{END}"))), [UNREADABLE]);
        // No word of the config's: git failed before the shell ran, or it
        // stopped part way (a record cut, so the code's record doesn't start
        // a pair), or the code isn't one. Never "no key".
        assert_eq!(raw(128, ""), [UNREADABLE]);
        assert_eq!(raw(0, END), [UNREADABLE]);
        assert_eq!(raw(0, &format!("local\0filter.x.clean\ncatconfig\00\0{END}")), [UNREADABLE]);
        assert_eq!(raw(0, &format!("local\0config\00\0{END}")), [UNREADABLE]);
        assert_eq!(raw(0, &format!("config\0x\0{END}")), [UNREADABLE]);
        assert_eq!(raw(0, &format!("config\0{END}")), [UNREADABLE]);
        // Only the repository's own scopes count, and only values that run something.
        let own = "local\0filter.x.clean\nprog\0global\0core.sshcommand\nssh\0worktree\0core.fsmonitor\ntrue\0local\0diff.x.textconv\0";
        assert_eq!(answer((0, own), (0, END)), ["diff.x.textconv", "filter.x.clean"]);
        // Submodules name themselves; one git can't read keeps the gate shut.
        let subs = &format!("sub\0vendor/a\0local\0filter.y.clean\ncat\0sub\0vendor/b\0err\0128\0{END}");
        assert_eq!(answer((1, ""), (0, subs)), ["vendor/a: filter.y.clean", "vendor/b: (unreadable config)"], "no key above: the submodules still count");
        assert_eq!(answer((0, "global\0core.askpass\nx\0"), (0, subs)), ["vendor/a: filter.y.clean", "vendor/b: (unreadable config)"]);
        // Remote files beside a config, the top's and a submodule's.
        let legacy = &format!("sub\0\0legacy\0.git/remotes\0sub\0vendor/a\0legacy\0.git/branches\0{END}");
        assert_eq!(answer((1, ""), (0, legacy)), [".git/remotes", "vendor/a: .git/branches"]);
        assert!(answer((0, ""), (2, "")).contains(&UNREADABLE.to_string()));
        // A scan that stopped part way (exit 0, records missing) keeps it shut.
        assert_eq!(answer((1, ""), (0, "")), [UNREADABLE]);
        assert_eq!(answer((1, ""), (0, "sub\0vendor/a\0local\0filter.y.clean\ncat\0")), [UNREADABLE, "vendor/a: filter.y.clean"]);
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

    // Folders trusted at the same moment (two panes, or the tests of one
    // process, which share the file) all stay trusted: each adds to the list
    // the one before it wrote.
    #[test]
    fn folders_trusted_at_once_all_stay_trusted() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        for round in 0..20 {
            let start = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for i in 0..8 {
                    let (file, start) = (&file, &start);
                    s.spawn(move || {
                        start.wait();
                        trust_at(file, &format!(":/r{round}-{i}")).unwrap();
                    });
                }
            });
            let all = trusted_from(&file);
            let lost: Vec<String> = (0..8).map(|i| format!(":/r{round}-{i}")).filter(|id| !all.contains(id)).collect();
            assert!(lost.is_empty(), "round {round}: lost {lost:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Every git command reads the list, so one may read it while a folder is
    // being trusted: it gets the old list or the new one, never part of one.
    // The new list takes the old one's place whole, so a reader that had the
    // old one open still reads all of it.
    #[test]
    fn a_reader_never_meets_a_list_half_written() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        trust_at(&file, ":/a").unwrap();
        let mut reading = std::fs::File::open(&file).unwrap();
        trust_at(&file, ":/b").unwrap();
        let mut text = String::new();
        reading.read_to_string(&mut text).unwrap();
        assert_eq!(serde_json::from_str::<Vec<String>>(&text).ok(), Some(vec![":/a".to_string()]), "{text:?}");
        assert_eq!(trusted_from(&file).into_iter().collect::<Vec<_>>(), [":/a", ":/b"]);
        drop(reading);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Without locks in the file system, threads of one process still take
    // turns: none of them is lost, nor do two write the same file beside it.
    #[test]
    fn folders_trusted_at_once_without_file_locks_all_stay_trusted() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-nolock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        let no_locks = |_: &std::fs::File| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        for round in 0..20 {
            let start = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for i in 0..8 {
                    let (file, start) = (&file, &start);
                    s.spawn(move || {
                        start.wait();
                        add_to_list(file, &format!(":/r{round}-{i}"), no_locks).unwrap();
                    });
                }
            });
            let all = trusted_from(&file);
            let lost: Vec<String> = (0..8).map(|i| format!(":/r{round}-{i}")).filter(|id| !all.contains(id)).collect();
            assert!(lost.is_empty(), "round {round}: lost {lost:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A list there that can't be read whole stays as it is, and so the trust
    // fails: written again with only the new folder, every one trusted
    // before would be gone. No list yet is the one case that starts empty.
    #[test]
    fn a_list_that_cant_be_read_is_never_written_over() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(trust_at(&file, ":/new").map_err(|e| e.kind()), Err(std::io::ErrorKind::InvalidData));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::remove_file(&file).unwrap();
            trust_at(&file, ":/keep-1").unwrap();
            trust_at(&file, ":/keep-2").unwrap();
            let kept = std::fs::read_to_string(&file).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            // Root reads it anyway: nothing to see then.
            if std::fs::read_to_string(&file).is_err() {
                assert_eq!(trust_at(&file, ":/new").map_err(|e| e.kind()), Err(std::io::ErrorKind::PermissionDenied));
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
                assert_eq!(std::fs::read_to_string(&file).unwrap(), kept);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A step of the write that fails leaves nothing beside the list: the
    // file beside it can't be made (a link there leads nowhere), or it can't
    // take the list's place (a folder stands there).
    #[test]
    fn a_write_that_fails_leaves_nothing_beside_the_list() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-left-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("trusted.json");
        let left = || -> Vec<String> { std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect() };
        #[cfg(unix)]
        {
            std::fs::create_dir_all(&dir).unwrap();
            std::os::unix::fs::symlink(dir.join("no/such/folder"), dir.join(format!("trusted.json.{}.tmp", std::process::id()))).unwrap();
            assert!(replace(&file, "[]").is_err());
            assert_eq!(left(), Vec::<String>::new(), "the write failed");
        }
        std::fs::create_dir_all(file.join("in-the-way")).unwrap();
        assert!(replace(&file, "[]").is_err());
        assert_eq!(left(), ["trusted.json"], "the move failed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Another process trusting a folder in the same file (a second Yuzora)
    // waits its turn too: the list changes under a lock on its lock file,
    // which this test holds a while as that process would.
    #[test]
    fn trusting_waits_while_another_process_holds_the_list() {
        let dir = std::env::temp_dir().join(format!("yuzora-git-trust-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("git/trusted.json");
        trust_at(&file, ":/a").unwrap();
        let held = std::fs::File::options().create(true).write(true).truncate(false).open(lock_file(&file)).unwrap();
        held.lock().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            let file = &file;
            s.spawn(move || {
                trust_at(file, ":/b").unwrap();
                done.send(()).unwrap();
            });
            assert!(finished.recv_timeout(Duration::from_millis(300)).is_err(), "trusted while another process held the list");
            held.unlock().unwrap();
            finished.recv_timeout(Duration::from_secs(10)).expect("still waiting once the list was let go");
        });
        assert_eq!(trusted_from(&file).into_iter().collect::<Vec<_>>(), [":/a", ":/b"]);
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
        // Nor are these: no folder, no folder above it, a file in its place.
        link("", "missing", "sub");
        link("", "no/where", "sub");
        std::fs::write(Path::new(&top).join("plain"), "x\n").unwrap();
        link("", "plain", "sub");
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

    /// A repo at `at` whose clean filter leaves `mark` behind; its HEAD.
    fn evil_repo(git: &impl Fn(&str, &[&str]) -> String, top: &str, at: &str, mark: &Path) -> String {
        git(at, &["init", "-q"]);
        let here = Path::new(top).join(at);
        std::fs::write(here.join("f"), "x\n").unwrap();
        std::fs::write(here.join(".gitattributes"), "* filter=evil\n").unwrap();
        git(at, &["add", "f", ".gitattributes"]);
        git(at, &["commit", "-qm", "s"]);
        git(at, &["config", "filter.evil.clean", &format!("touch '{}'; cat", mark.display())]);
        git(at, &["rev-parse", "HEAD"])
    }

    /// The longest path the system takes, its NUL included.
    const PATH_MAX: usize = if cfg!(target_os = "linux") { 4096 } else { 1024 };

    /// A gitlink to `commit` in `top`'s index named `name`, which git takes
    /// from no command: a stand-in of the same length, renamed in the index
    /// file (and its checksum written again).
    fn link_by_a_name_git_never_writes(git: &impl Fn(&str, &[&str]) -> String, top: &str, commit: &str, name: &str) {
        let stand_in = "Q".repeat(name.len());
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{commit},{stand_in}")]);
        let index = Path::new(top).join(".git/index");
        let bytes = std::fs::read(&index).unwrap();
        let mut body = bytes[..bytes.len() - 20].to_vec();
        let at = body.windows(name.len()).position(|w| w == stand_in.as_bytes()).unwrap();
        body[at..at + name.len()].copy_from_slice(name.as_bytes());
        let sum = sha1(&body);
        std::fs::write(&index, [&body[..], &sum].concat()).unwrap();
    }

    /// SHA-1, for an index file's checksum (`sha1sum` and `shasum` aren't on
    /// every system).
    fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
        let mut m = data.to_vec();
        m.push(0x80);
        while m.len() % 64 != 56 {
            m.push(0);
        }
        m.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
        for block in m.chunks(64) {
            let mut w = [0u32; 80];
            for (i, word) in block.chunks(4).enumerate() {
                w[i] = u32::from_be_bytes(word.try_into().unwrap());
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let [mut a, mut b, mut c, mut d, mut e] = h;
            for (i, wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                    20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                    _ => (b ^ c ^ d, 0xCA62_C1D6),
                };
                let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
                (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
            }
            for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
                *x = x.wrapping_add(y);
            }
        }
        let mut out = [0; 20];
        for (i, x) in h.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
        }
        out
    }

    /// Whether `git status` at `cwd` runs the clean filter of the repo at
    /// `evil` (its `f` made to look changed, so status cleans it again).
    fn status_runs_the_filter(cwd: &str, evil: &Path, mark: &Path) -> bool {
        let _ = std::fs::remove_file(mark);
        std::fs::File::options().write(true).open(evil.join("f")).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800)).unwrap();
        let _ = std::process::Command::new("git").args(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--untracked-files=no"]).current_dir(cwd).output();
        mark.exists()
    }

    #[cfg(unix)]
    #[test]
    fn a_work_tree_name_ending_in_a_newline_shuts_the_gate() {
        // `$(…)` drops the newline: `wt` (a decoy) for `wt<LF>`, and the top
        // itself for `<LF>`; status goes into the real one.
        for (name, wt, decoy) in [("nl-wt", "wt\n", "wt"), ("nl-top", "\n", "")] {
            let (dir, top, git) = scratch_dir(name);
            let repo = |at: &str| {
                git(at, &["init", "-q"]);
                std::fs::write(Path::new(&top).join(at).join("f"), "x\n").unwrap();
                git(at, &["add", "f"]);
                git(at, &["commit", "-qm", "s"]);
                git(at, &["rev-parse", "HEAD"])
            };
            repo("");
            let s = repo("s");
            git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{s},s")]);
            std::fs::create_dir_all(Path::new(&top).join(decoy)).unwrap();
            let mark = dir.join("ran");
            let evil = evil_repo(&git, &top, &format!("{wt}/evil"), &mark);
            git("s", &["update-index", "--add", "--cacheinfo", &format!("160000,{evil},evil")]);
            git("s", &["config", "core.worktree", &format!("../../{wt}")]);
            let got = programs_within(None, &top, Duration::from_secs(30));
            let ran = status_runs_the_filter(&top, &Path::new(&top).join(format!("{wt}/evil")), &mark);
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(ran, "{name}: status didn't run the filter, so this proves nothing");
            assert_eq!(got, Ok(vec![format!("s: {UNREADABLE}")]), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_top_whose_work_tree_name_ends_in_a_newline_shuts_the_gate() {
        let (dir, top, git) = scratch_dir("nl-root");
        // The git dir in `repo`, its work tree `wt<LF>`; `wt` is a decoy.
        git("repo", &["init", "-q"]);
        git("repo", &["config", "core.worktree", &format!("{top}/wt\n")]);
        std::fs::create_dir_all(Path::new(&top).join("wt")).unwrap();
        let mark = dir.join("ran");
        let evil = evil_repo(&git, &top, "wt\n/s", &mark);
        git("repo", &["update-index", "--add", "--cacheinfo", &format!("160000,{evil},s")]);
        let repo = format!("{top}/repo");
        let got = programs_within(None, &repo, Duration::from_secs(30));
        let ran = status_runs_the_filter(&repo, &Path::new(&top).join("wt\n/s"), &mark);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ran, "status didn't run the filter, so this proves nothing");
        assert_eq!(got, Ok(vec![format!(".: {UNREADABLE}")]));
    }

    #[cfg(unix)]
    #[test]
    fn a_gitlink_named_by_an_absolute_path_shuts_the_gate() {
        let (dir, top, git) = scratch_dir("abs-link");
        git("", &["init", "-q"]);
        std::fs::write(Path::new(&top).join("f"), "x\n").unwrap();
        git("", &["add", "f"]);
        git("", &["commit", "-qm", "s"]);
        let (other, elsewhere, git_there) = scratch_dir("abs-link-ev");
        let mark = other.join("ran");
        let evil = evil_repo(&git_there, &elsewhere, "ev", &mark);
        let abs = format!("{elsewhere}/ev");
        link_by_a_name_git_never_writes(&git, &top, &evil, &abs);
        assert!(git("", &["ls-files", "-s"]).contains(&format!("\t{abs}")));
        let got = programs_within(None, &top, Duration::from_secs(30));
        let ran = status_runs_the_filter(&top, Path::new(&abs), &mark);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
        assert!(ran, "status didn't run the filter, so this proves nothing");
        assert_eq!(got, Ok(vec![format!("{abs}: {UNREADABLE}")]));
    }

    #[cfg(unix)]
    #[test]
    fn a_gitlink_name_with_a_dot_or_empty_part_shuts_the_gate() {
        // git goes in by the name as it is (`name/.git/commondir` the longest
        // path it reads first), and these make `top/name/.git` too long.
        let long = PATH_MAX - 1 - "/.git/commondir".len();
        for (tag, name) in [
            ("dots-pad", format!("{}ev", "./".repeat((long - 2) / 2))),
            ("dots-slash", format!("ev{}", "/".repeat(long - 2))),
            ("dots-up", format!("{}ev", "x/../".repeat((long - 2) / 5))),
        ] {
            let (dir, top, git) = scratch_dir(tag);
            git("", &["init", "-q"]);
            std::fs::create_dir_all(Path::new(&top).join("x")).unwrap();
            let mark = dir.join("ran");
            let evil = evil_repo(&git, &top, "ev", &mark);
            link_by_a_name_git_never_writes(&git, &top, &evil, &name);
            let got = programs_within(None, &top, Duration::from_secs(30));
            let ran = status_runs_the_filter(&top, &Path::new(&top).join("ev"), &mark);
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(ran, "{tag}: status didn't run the filter, so this proves nothing");
            assert_eq!(got, Ok(vec![format!("{name}: {UNREADABLE}")]), "{tag}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_gitlink_name_with_a_colon_is_read_where_git_writes_it() {
        // `git add` writes these; only a Windows host takes `a:` for a drive.
        for (tag, name) in [("colon-ab", "a:b"), ("colon-1", "1:"), ("colon-xyz", "x:y/z")] {
            let (dir, top, git) = scratch_dir(tag);
            git("", &["init", "-q"]);
            evil_repo(&git, &top, name, &dir.join("ran"));
            git("", &["add", name]);
            let got = programs_within(None, &top, Duration::from_secs(30));
            std::fs::remove_dir_all(&dir).unwrap();
            assert_eq!(got, Ok(vec![format!("{name}: filter.evil.clean")]), "{tag}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn on_a_windows_host_names_win32_reads_otherwise_shut_the_gate() {
        // Win32 takes `a:` for a drive (or `a:b` for a stream) and drops a
        // part's last `.` or space: git would go where the name doesn't say.
        let (dir, top, git) = scratch_dir("win");
        git("", &["init", "-q"]);
        let names = ["a:b", "ev.", "ev ", "d/ev./x", "ok"];
        for name in names {
            evil_repo(&git, &top, name, &dir.join("ran"));
            git("", &["add", name]);
        }
        let scan = |env: &[(&str, String)]| {
            repo_programs(|args| {
                let out = std::process::Command::new("git").args(READ_FLAGS).args(args).envs(env.iter().map(|(k, v)| (k, v))).current_dir(&top).output().map_err(|e| e.to_string())?;
                Ok::<_, String>(whole(out.status.code().unwrap_or(-1), out.stdout))
            })
        };
        let here = scan(&[]);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("uname"), "#!/bin/sh\necho MINGW64_NT-10.0-19045\n").unwrap();
        std::fs::set_permissions(bin.join("uname"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let by_uname = scan(&[("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))]);
        let by_os = scan(&[("OS", "Windows_NT".into())]);
        std::fs::remove_dir_all(&dir).unwrap();
        let keys = |shut: bool| -> Vec<String> { names.iter().map(|n| format!("{n}: {}", if shut && *n != "ok" { UNREADABLE } else { "filter.evil.clean" })).collect::<BTreeSet<_>>().into_iter().collect() };
        assert_eq!(here, Ok(keys(false)), "not Windows: all read");
        assert_eq!(by_uname, Ok(keys(true)), "uname says MINGW");
        assert_eq!(by_os, Ok(keys(true)), "OS=Windows_NT");
    }

    #[cfg(unix)]
    #[test]
    fn a_remote_that_runs_a_command_waits_for_trust() {
        let (dir, top, git) = scratch_dir("ext");
        git("", &["init", "-q"]);
        let mark = dir.join("ran");
        let ext = format!("ext::sh -c touch% {}", mark.display());
        let fetch_runs_it = || {
            let _ = std::fs::remove_file(&mark);
            // As a person's global config may allow it.
            let _ = std::process::Command::new("git").args(["-c", "protocol.ext.allow=always", "fetch", "-q", "origin"]).current_dir(&top).output();
            mark.exists()
        };
        git("", &["remote", "add", "origin", &ext]);
        let direct = programs_within(None, &top, Duration::from_secs(30));
        let direct_runs = fetch_runs_it();
        // A plain URL that the repo's insteadOf makes one.
        git("", &["remote", "set-url", "origin", "https://example.invalid/r.git"]);
        let plain = programs_within(None, &top, Duration::from_secs(30));
        git("", &["config", &format!("url.{ext}.insteadOf"), "https://example.invalid/r.git"]);
        let rewritten = programs_within(None, &top, Duration::from_secs(30));
        let rewritten_runs = fetch_runs_it();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(direct_runs && rewritten_runs, "fetch didn't run the command, so this proves nothing");
        assert_eq!(direct, Ok(vec!["remote.origin.url".to_string()]));
        assert_eq!(plain, Ok(Vec::new()));
        assert_eq!(rewritten, Ok(vec![format!("url.{ext}.insteadof")]));
    }

    // A remote helper git reaches without a remote URL saying `ext::`: the
    // URL split between it and an insteadOf, the remote files git still
    // reads (`.git/remotes/<name>`, `.git/branches/<name>`), and the promisor
    // remote a partial clone fetches from by name.
    #[cfg(unix)]
    #[test]
    fn a_remote_helper_named_any_other_way_waits_for_trust() {
        // A case's setup: the top, the command to run, and git there.
        type Setup = dyn Fn(&Path, &str, &dyn Fn(&str, &[&str]) -> String);
        let case = |name: &str, setup: &Setup, runs: &[&str]| {
            let (dir, top, git) = scratch_dir(name);
            git("", &["init", "-q"]);
            git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
            let mark = dir.join("ran");
            setup(Path::new(&top), &format!("sh -c touch% {}", mark.display()), &git);
            let keys = programs_within(None, &top, Duration::from_secs(30));
            // As a person's global config may allow it.
            let _ = std::process::Command::new("git").args(["-c", "protocol.ext.allow=always"]).args(runs).current_dir(&top).output();
            let ran = mark.exists();
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(ran, "{name}: git didn't run the command, so this proves nothing");
            keys
        };
        let split = case("ext-split", &|_, cmd, git| {
            git("", &["remote", "add", "origin", &format!("P:{cmd}")]);
            git("", &["config", "url.ext:.insteadOf", "P"]);
        }, &["fetch", "-q", "origin"]);
        let remotes = case("ext-remotes", &|top, cmd, _| {
            std::fs::create_dir_all(top.join(".git/remotes")).unwrap();
            std::fs::write(top.join(".git/remotes/origin"), format!("URL: ext::{cmd}\n")).unwrap();
        }, &["fetch", "-q", "origin"]);
        let branches = case("ext-branches", &|top, cmd, _| {
            std::fs::create_dir_all(top.join(".git/branches")).unwrap();
            std::fs::write(top.join(".git/branches/origin"), format!("ext::{cmd}\n")).unwrap();
        }, &["fetch", "-q", "origin"]);
        let promisor = case("ext-promisor", &|_, cmd, git| {
            git("", &["config", "extensions.partialClone", &format!("ext::{cmd}")]);
        }, &["cat-file", "-e", "1111111111111111111111111111111111111111"]);
        assert_eq!(split, Ok(vec!["url.ext:.insteadof".to_string()]));
        assert_eq!(remotes, Ok(vec![".git/remotes".to_string()]));
        assert_eq!(branches, Ok(vec![".git/branches".to_string()]));
        assert_eq!(promisor, Ok(vec!["extensions.partialclone".to_string()]));
    }

    // git takes a remote's name for its URL when the remote has none: a
    // remote the config names by a helper URL (whatever its keys), or such a
    // name where a remote is wanted (a branch's remote, the push default, a
    // group), runs the helper. Also with git's default policy, through a
    // helper the person installed (`zz::`).
    #[cfg(unix)]
    #[test]
    fn a_remote_named_by_a_helper_url_waits_for_trust() {
        use std::os::unix::fs::PermissionsExt;
        // A case's setup: the top, an `ext::` URL that runs one program, and git there.
        type Setup = dyn Fn(&Path, &str, &dyn Fn(&str, &[&str]) -> String);
        let case = |name: &str, setup: &Setup, runs: &[&str]| {
            let (dir, top, git) = scratch_dir(name);
            git("", &["init", "-q", "-b", "main"]);
            git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
            let mark = dir.join("ran");
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            for program in ["hit", "git-remote-zz"] {
                std::fs::write(bin.join(program), format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
                std::fs::set_permissions(bin.join(program), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let cmd = format!("ext::{}", bin.join("hit").display());
            setup(Path::new(&top), &cmd, &git);
            let keys = programs_within(None, &top, Duration::from_secs(30));
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
            // As a person's global config may allow `ext`; `zz` needs nothing.
            let _ = std::process::Command::new("git").args(["-c", "protocol.ext.allow=always"]).args(runs).current_dir(&top).env("PATH", path).output();
            let ran = mark.exists();
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(ran, "{name}: git didn't run the helper, so this proves nothing");
            keys.map(|keys| keys.into_iter().map(|k| k.replace(&cmd, "<ext>")).collect::<Vec<_>>())
        };
        let fetch_all: &[&str] = &["fetch", "-q", "--all"];
        let got = [
            case("name-fetch", &|_, cmd, git| _ = git("", &["config", &format!("remote.{cmd}.fetch"), "+refs/heads/*:refs/remotes/x/*"]), fetch_all),
            case("name-skip", &|_, cmd, git| _ = git("", &["config", &format!("remote.{cmd}.skipDefaultUpdate"), "false"]), fetch_all),
            case("name-promisor", &|_, cmd, git| _ = git("", &["config", &format!("remote.{cmd}.promisor"), "true"]), &["cat-file", "-e", "1111111111111111111111111111111111111111"]),
            case("branch-remote", &|_, cmd, git| _ = git("", &["config", "branch.main.remote", cmd]), &["fetch", "-q"]),
            // Pushing elsewhere than the branch's remote: git needs no upstream then.
            case("branch-pushremote", &|_, cmd, git| {
                git("", &["config", "branch.main.remote", "origin"]);
                git("", &["config", "branch.main.pushRemote", cmd]);
            }, &["push", "-q"]),
            case("push-default", &|_, cmd, git| {
                git("", &["config", "branch.main.remote", "origin"]);
                git("", &["config", "remote.pushDefault", cmd]);
            }, &["push", "-q"]),
            // A group of one is fetched as the remote of its own name: two.
            case("group", &|_, cmd, git| _ = git("", &["config", "remotes.all", &format!("origin {cmd}")]), &["fetch", "-q", "all"]),
            case("submodule-url", &|_, cmd, git| {
                git("", &["config", "-f", ".gitmodules", "submodule.s.path", "s"]);
                git("", &["config", "-f", ".gitmodules", "submodule.s.url", "https://example.invalid/s"]);
                let head = git("", &["rev-parse", "HEAD"]);
                git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{head},s")]);
                git("", &["config", "submodule.s.url", cmd]);
            }, &["submodule", "-q", "update", "--init"]),
            case("zz-name", &|_, _, git| _ = git("", &["config", "remote.zz::payload.fetch", "+refs/heads/*:refs/remotes/x/*"]), fetch_all),
            case("zz-branch", &|_, _, git| _ = git("", &["config", "branch.main.remote", "zz::payload"]), &["fetch", "-q"]),
            // `<scheme>://` that git doesn't serve itself: the same helper.
            case("zz-url", &|_, _, git| _ = git("", &["config", "remote.origin.url", "zz://payload"]), &["fetch", "-q", "origin"]),
            case("zz-url-name", &|_, _, git| _ = git("", &["config", "remote.zz://payload.fetch", "+refs/heads/*:refs/remotes/x/*"]), fetch_all),
            case("zz-url-insteadof", &|_, _, git| {
                git("", &["remote", "add", "origin", "https://example.invalid/r"]);
                git("", &["config", "url.zz://p/.insteadOf", "https://example.invalid/"]);
            }, &["fetch", "-q", "origin"]),
            case("zz-url-partialclone", &|_, _, git| _ = git("", &["config", "extensions.partialClone", "zz://payload"]), &["cat-file", "-e", "1111111111111111111111111111111111111111"]),
            // `ext://x/run` runs `ext:/x/run`, a program the repo itself can hold.
            case("ext-url", &|top, _, git| {
                let run = top.join("ext:/x/run");
                std::fs::create_dir_all(run.parent().unwrap()).unwrap();
                std::fs::copy(top.join("bin/hit"), &run).unwrap();
                git("", &["config", "remote.origin.url", "ext://x/run"]);
            }, &["fetch", "-q", "origin"]),
        ];
        let want = [
            "remote.<ext>.fetch",
            "remote.<ext>.skipdefaultupdate",
            "remote.<ext>.promisor",
            "branch.main.remote",
            "branch.main.pushremote",
            "remote.pushdefault",
            "remotes.all",
            "submodule.s.url",
            "remote.zz::payload.fetch",
            "branch.main.remote",
            "remote.origin.url",
            "remote.zz://payload.fetch",
            "url.zz://p/.insteadof",
            "extensions.partialclone",
            "remote.origin.url",
        ];
        assert_eq!(got.to_vec(), want.map(|k| Ok(vec![k.to_string()])));
    }

    // git takes a folder for a git dir by its `HEAD` as the file system opens
    // it (`head` where case doesn't count, a link to a ref that's only
    // packed) and by `objects/` and `refs/` in its common dir (`commondir`):
    // the scan asks git which folders under `modules/` are git dirs and reads
    // their config through git. One that looks like one, but git won't take,
    // shuts the gate.
    #[cfg(unix)]
    #[test]
    fn a_git_dir_counts_however_it_hides_from_a_listing() {
        let (dir, top, git) = scratch_dir("hidden");
        git("", &["init", "-q", "-b", "main"]);
        git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
        let at = |p: &str| Path::new(&top).join(p);
        let bare = |gd: &str, key: &str| {
            git("", &["init", "-q", "--bare", gd]);
            git("", &["config", "--file", &format!("{gd}/config"), key, "hit"]);
        };
        bare(".git/modules/lower", "filter.evil.smudge");
        std::fs::rename(at(".git/modules/lower/HEAD"), at(".git/modules/lower/head")).unwrap();
        let ignores_case = at(".git/modules/lower/HEAD").exists();
        bare(".git/elsewhere", "core.sshCommand");
        std::fs::create_dir_all(at(".git/modules/common")).unwrap();
        std::fs::copy(at(".git/elsewhere/HEAD"), at(".git/modules/common/HEAD")).unwrap();
        std::fs::write(at(".git/modules/common/commondir"), "../../elsewhere\n").unwrap();
        bare(".git/modules/link", "core.gitProxy");
        std::fs::remove_file(at(".git/modules/link/HEAD")).unwrap();
        std::os::unix::fs::symlink("refs/heads/main", at(".git/modules/link/HEAD")).unwrap();
        // A submodule's git dir names a work tree (`core.worktree`) a removed
        // submodule may not have anymore: git there stops at it, not here.
        bare(".git/modules/gone", "filter.evil.clean");
        git("", &["config", "--file", ".git/modules/gone/config", "core.bare", "false"]);
        git("", &["config", "--file", ".git/modules/gone/config", "core.worktree", "../../../no/such/s"]);
        let git_dir = |gd: &str| std::process::Command::new("git").args(["rev-parse", "--git-dir"]).env("GIT_DIR", at(gd)).output().unwrap().status.success();
        assert!(git_dir(".git/modules/common") && git_dir(".git/modules/link"), "git doesn't take them, so this proves nothing");
        assert_eq!(git_dir(".git/modules/lower"), ignores_case);
        let got = programs_within(None, &top, Duration::from_secs(30));
        assert!(!git_dir(".git/modules/gone"), "git doesn't stop at the missing work tree, so this proves nothing");
        // `link/HEAD`, to a ref that's only packed, leads the kernel nowhere,
        // and the scan can't tell where git's real path would lead.
        let mut want = vec![".git/modules/common: core.sshcommand".to_string(), ".git/modules/gone: filter.evil.clean".into(), format!(".git/modules/link/HEAD: {UNREADABLE}"), ".git/modules/link: core.gitproxy".into()];
        if ignores_case {
            want.push(".git/modules/lower: filter.evil.smudge".into());
        }
        want.sort();
        assert_eq!(got, Ok(want.clone()));
        // `HEAD`, `objects/` and `refs/`, but a `HEAD` git can't read.
        git("", &["init", "-q", "--bare", ".git/modules/broken"]);
        std::fs::write(at(".git/modules/broken/HEAD"), "nonsense\n").unwrap();
        assert!(!git_dir(".git/modules/broken"));
        let got = programs_within(None, &top, Duration::from_secs(30));
        std::fs::remove_dir_all(&dir).unwrap();
        want.insert(0, format!(".git/modules/broken: {UNREADABLE}"));
        assert_eq!(got, Ok(want));
    }

    // `.git/modules/<name>` may be a gitfile (`gitdir: …`, or a link to one):
    // git follows it when it opens the submodule, as a push that checks the
    // submodule's commits does in its own process. The git dir it names
    // counts, and that git dir's own `modules/`; one git won't take shuts the
    // gate. Since git 2.54 `submodule.<name>.gitdir` may put one anywhere.
    #[cfg(unix)]
    #[test]
    fn a_gitfile_in_modules_counts_as_the_git_dir_it_names() {
        let (dir, top, git) = scratch_dir("gitfile");
        git("", &["init", "-q", "-b", "main"]);
        git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
        let at = |p: &str| Path::new(&top).join(p);
        let bare = |gd: &str, key: &str| {
            git("", &["init", "-q", "--bare", gd]);
            git("", &["config", "--file", &format!("{gd}/config"), key, "hit"]);
        };
        bare(".git/evil.git", "remote.origin.uploadPack");
        bare(".git/evil.git/modules/n", "filter.evil.smudge");
        bare(".git/other.git", "core.sshCommand");
        std::fs::create_dir_all(at(".git/modules")).unwrap();
        std::fs::create_dir_all(at(".git/elsewhere")).unwrap();
        std::fs::write(at(".git/modules/s"), "gitdir: ../evil.git\n").unwrap();
        // git reads a link's target, relative to where the link is.
        std::fs::write(at(".git/elsewhere/f"), "gitdir: ../other.git\n").unwrap();
        std::os::unix::fs::symlink("../elsewhere/f", at(".git/modules/l")).unwrap();
        // A link to a gitfile a level deeper: `GIT_DIR` reads `../d.git`
        // from the link's folder, git opening a submodule itself (its real
        // path first) from the file's. Each git dir's own `modules/` counts.
        bare(".git/d.git", "core.gitProxy");
        bare(".git/d.git/modules/n", "filter.link.clean");
        bare(".git/deep/d.git", "remote.origin.receivePack");
        std::fs::create_dir_all(at(".git/deep/a")).unwrap();
        std::fs::write(at(".git/deep/a/f"), "gitdir: ../d.git\n").unwrap();
        std::os::unix::fs::symlink("../deep/a/f", at(".git/modules/d")).unwrap();
        // git takes a gitfile by how it starts.
        std::fs::write(at(".git/modules/note"), "a note\ngitdir: ../evil.git\n").unwrap();
        let git_dir = |gd: &str| std::process::Command::new("git").args(["rev-parse", "--git-dir"]).env("GIT_DIR", at(gd)).output().unwrap().status.success();
        assert!(git_dir(".git/modules/s") && git_dir(".git/modules/l") && !git_dir(".git/modules/note"), "git doesn't take them as it should, so this proves nothing");
        let got = programs_within(None, &top, Duration::from_secs(30));
        let want = [".git/modules/d/.git/modules/n: filter.link.clean", ".git/modules/d: core.gitproxy", ".git/modules/d: remote.origin.receivepack", ".git/modules/l: core.sshcommand", ".git/modules/s/.git/modules/n: filter.evil.smudge", ".git/modules/s: remote.origin.uploadpack"];
        assert_eq!(got, Ok(want.map(String::from).to_vec()));
        std::fs::write(at(".git/modules/bad"), "gitdir: ../nowhere\n").unwrap();
        git("", &["config", "submodule.x.gitdir", "../anywhere"]);
        let got = programs_within(None, &top, Duration::from_secs(30));
        std::fs::remove_dir_all(&dir).unwrap();
        let mut want = want.map(String::from).to_vec();
        want.insert(0, format!(".git/modules/bad: {UNREADABLE}"));
        want.push("submodule.x.gitdir".into());
        assert_eq!(got, Ok(want));
    }

    // A link under `modules/` (or `modules/` itself) the kernel can't follow,
    // so neither `find -L` nor `[ -d ]`, git may: the real path it takes
    // before it opens a submodule in its own process reads `x/..` and a
    // trailing slash as written, whatever `x` is, and follows more links than
    // macOS does (32). The scan can't read where git would go, so one shuts
    // the gate; one the kernel follows is read as before.
    #[cfg(unix)]
    #[test]
    fn a_link_the_kernel_cant_follow_shuts_the_gate() {
        let (mut got, mut want) = (vec![], vec![]);
        for case in ["slash", "file-dotdot", "file-dotdot-dir", "modules", "chain-33", "dir-slash", "chain-32"] {
            let name = format!("kernel-{case}");
            let (dir, root, git) = scratch_dir(&name);
            let at = |p: &str| Path::new(&root).join(p);
            git("lib", &["init", "-q", "-b", "main"]);
            for n in ["one", "two"] {
                std::fs::write(at("lib/f"), n).unwrap();
                git("lib", &["add", "f"]);
                git("lib", &["commit", "-q", "-m", n]);
            }
            let one = git("lib", &["rev-parse", "HEAD~1"]);
            git("top", &["init", "-q", "-b", "main"]);
            git("top", &["-c", "protocol.file.allow=always", "submodule", "add", "-q", &at("lib").display().to_string(), "s"]);
            git("top/s", &["checkout", "-q", &one]);
            git("top", &["commit", "-q", "-am", "one"]);
            git("top/s", &["checkout", "-q", "main"]);
            git("top", &["commit", "-q", "-am", "two"]);
            git("top", &["submodule", "deinit", "-q", "-f", "s"]);
            // The submodule's git dir moved out of `modules/`, a gitfile to it.
            let g = |p: &str| at(&format!("top/.git/{p}"));
            std::fs::create_dir_all(g("deep/a")).unwrap();
            std::fs::rename(g("modules/s"), g("deep/evil.git")).unwrap();
            git("top", &["config", "--file", ".git/deep/evil.git/config", "core.sshCommand", "hit"]);
            std::fs::write(g("deep/a/f"), "gitdir: ../evil.git\n").unwrap();
            let link = |to: &str, from: &str| std::os::unix::fs::symlink(to, g(from)).unwrap();
            let chain = |n: usize| {
                std::fs::create_dir_all(g("l")).unwrap();
                (1..n - 1).for_each(|i| link(&(i + 1).to_string(), &format!("l/{i}")));
                link("../deep/evil.git", &format!("l/{}", n - 1));
                link("../l/1", "modules/s");
            };
            match case {
                "slash" => link("../deep/a/f/", "modules/s"),
                "file-dotdot" | "file-dotdot-dir" => {
                    std::fs::write(g("modules/x"), "").unwrap();
                    link(if case == "file-dotdot" { "x/../../deep/a/f" } else { "x/../../deep/evil.git" }, "modules/s");
                }
                "modules" => {
                    std::fs::remove_dir(g("modules")).unwrap();
                    std::fs::create_dir(g("store")).unwrap();
                    std::fs::rename(g("deep/evil.git"), g("store/s")).unwrap();
                    std::fs::write(g("x"), "").unwrap();
                    link("x/../store", "modules");
                }
                "chain-33" => chain(33),
                "chain-32" => chain(32),
                _ => link("../deep/evil.git/", "modules/s"),
            }
            let follows = std::fs::metadata(g("modules/s")).is_ok();
            let opens = git("top", &["diff", "--submodule=log", "HEAD~1", "HEAD"]).contains("> two");
            got.push((case, follows, opens, programs_within(None, &at("top").display().to_string(), Duration::from_secs(30))));
            std::fs::remove_dir_all(&dir).unwrap();
            // Linux follows 40 links, so 33 too.
            let kernel = matches!(case, "dir-slash" | "chain-32") || (case == "chain-33" && !cfg!(target_os = "macos"));
            // GNU find stops at a link it can't follow but for a missing
            // target, so there all of `modules/` can't be read.
            let scan = match (kernel, case) {
                (true, _) => ".git/modules/s: core.sshcommand".to_owned(),
                (false, "modules") => format!(".git/modules: {UNREADABLE}"),
                (false, _) if cfg!(target_os = "linux") => format!(".git/modules: {UNREADABLE}"),
                (false, _) => format!(".git/modules/s: {UNREADABLE}"),
            };
            want.push((case, kernel, true, Ok(vec![scan])));
        }
        assert_eq!(got, want, "(case, the kernel follows, git opens it in its own process, the scan)");
    }

    // A partial clone, by the keys git reads: its promisor remote is
    // contacted as it reads, whatever its URL.
    #[cfg(unix)]
    #[test]
    fn a_partial_clone_counts() {
        let (dir, top, git) = scratch_dir("partial");
        git("", &["init", "-q", "-b", "main"]);
        git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
        assert_eq!(programs_within(None, &top, Duration::from_secs(30)), Ok(vec![]));
        git("", &["config", "remote.origin.url", "https://example.invalid/r.git"]);
        git("", &["config", "remote.origin.promisor", "true"]);
        git("", &["config", "remote.origin.partialclonefilter", "blob:none"]);
        git("", &["config", "extensions.partialClone", "origin"]);
        let got = programs_within(None, &top, Duration::from_secs(30));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got, Ok(vec!["extensions.partialclone".to_string(), "remote.origin.partialclonefilter".into(), "remote.origin.promisor".into()]));
    }

    // A submodule not checked out keeps its git dir (`.git/modules/<name>`,
    // its own nested ones inside it), and `submodule update` reuses it, a
    // fetch recurses into it: its config's programs count, though no
    // gitlink's folder holds a `.git`.
    #[cfg(unix)]
    #[test]
    fn a_submodule_git_dir_nothing_checks_out_counts_too() {
        let (dir, top, git) = scratch_dir("modules");
        git("", &["init", "-q", "-b", "main"]);
        git("", &["commit", "-q", "--allow-empty", "-m", "x"]);
        assert_eq!(programs_within(None, &top, Duration::from_secs(30)), Ok(vec![]));
        // As `submodule deinit` leaves them: a name with a slash, one nested in another.
        for (gd, key) in [(".git/modules/s", "filter.evil.smudge"), (".git/modules/vendor/lib", "core.sshCommand"), (".git/modules/s/modules/n", "core.gitProxy"), (".git/modules/plain", "")] {
            git("", &["init", "-q", "--bare", gd]);
            if !key.is_empty() {
                git("", &["config", "--file", &format!("{gd}/config"), key, "hit"]);
            }
        }
        // A remote's HEAD and a reflog's are files named HEAD too, not git dirs.
        git("", &["update-ref", "refs/remotes/origin/HEAD", "HEAD"]);
        let got = programs_within(None, &top, Duration::from_secs(30));
        assert_eq!(got, Ok(vec![".git/modules/s/modules/n: core.gitproxy".to_string(), ".git/modules/s: filter.evil.smudge".into(), ".git/modules/vendor/lib: core.sshcommand".into()]));
        // A linked worktree keeps its own (`.git/worktrees/<id>/modules`); the
        // common dir's count there too. The main worktree never uses a linked one's.
        git("", &["worktree", "add", "-q", "wt"]);
        git("", &["init", "-q", "--bare", ".git/worktrees/wt/modules/x"]);
        git("", &["config", "--file", ".git/worktrees/wt/modules/x/config", "core.sshCommand", "hit"]);
        let at_wt = programs_within(None, &format!("{top}/wt"), Duration::from_secs(30));
        assert_eq!(programs_within(None, &top, Duration::from_secs(30)), got, "the main worktree");
        let mut want = got.clone().unwrap();
        want.push(".git/worktrees/wt/modules/x: core.sshcommand".into());
        assert_eq!(at_wt, Ok(want));
        // One whose name the scan can't read back whole shuts the gate.
        git("", &["init", "-q", "--bare", ".git/modules/a\nb"]);
        let got = programs_within(None, &top, Duration::from_secs(30)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(got.iter().any(|k| k.ends_with(UNREADABLE)), "{got:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_gitlink_the_scan_cant_look_into_shuts_the_gate() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, top, git) = scratch_dir("locked");
        git("", &["init", "-q"]);
        let evil = evil_repo(&git, &top, "ev", &dir.join("ran"));
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{evil},ev")]);
        // A folder that lists but can't be gone into: `ev/.git` may be there.
        // git can't look either and passes it over; the scan can't tell
        // that from a lookup failing for another reason, so it shuts on purpose.
        let ev = Path::new(&top).join("ev");
        std::fs::set_permissions(&ev, std::fs::Permissions::from_mode(0o600)).unwrap();
        let hidden = std::fs::metadata(ev.join(".git")).is_err(); // not for root
        let got = programs_within(None, &top, Duration::from_secs(30));
        std::fs::set_permissions(&ev, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        if hidden {
            assert_eq!(got, Ok(vec![format!("ev: {UNREADABLE}")]));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_gitlink_past_the_longest_path_from_the_root_is_read_by_its_name() {
        // `top/name/.git` is past PATH_MAX; git's `name/.git` isn't. Its
        // config is read by that name; below it the scan goes by its git
        // dir's path from the root, too long here (`--absolute-git-dir`
        // fails; a few bytes shorter, its index can't be opened), so it's
        // unreadable too.
        let (dir, top, git) = scratch_dir("deep");
        git("", &["init", "-q"]);
        let mark = dir.join("ran");
        let evil = evil_repo(&git, &top, "ev", &mark);
        let want = PATH_MAX - 3 - top.len() - 1;
        let mut name = String::new();
        while want - name.len() > 201 {
            name.push_str(&"d".repeat(200));
            name.push('/');
        }
        name.push_str(&"e".repeat(want - name.len()));
        let at = Path::new(&top).join(&name);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::rename(Path::new(&top).join("ev"), &at).unwrap();
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{evil},{name}")]);
        let got = programs_within(None, &top, Duration::from_secs(30));
        let ran = status_runs_the_filter(&top, &at, &mark);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ran, "status didn't run the filter, so this proves nothing");
        assert_eq!(got, Ok(vec![format!("{name}: {UNREADABLE}"), format!("{name}: filter.evil.clean")]));
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
    fn work_trees_pointing_back_up_end_the_scan_soon() {
        let (dir, top, git) = scratch_dir("cycle");
        let repo = |at: &str| {
            git(at, &["init", "-q"]);
            std::fs::write(Path::new(&top).join(at).join("f"), "x\n").unwrap();
            git(at, &["add", "f"]);
            git(at, &["commit", "-qm", "s"]);
        };
        repo("");
        for s in ["s1", "s2"] {
            repo(s);
            git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{},{s}", git(s, &["rev-parse", "HEAD"]))]);
        }
        // Both work in the top (their core.worktree) with its index: the
        // same two gitlinks again at every level, 2^16 folders to 16 levels.
        for s in ["s1", "s2"] {
            git(s, &["config", "core.worktree", "../.."]);
            std::fs::copy(Path::new(&top).join(".git/index"), Path::new(&top).join(s).join(".git/index")).unwrap();
        }
        let started = std::time::Instant::now();
        let got = programs_within(None, &top, Duration::from_secs(10));
        let took = started.elapsed();
        std::fs::remove_dir_all(&dir).unwrap();
        let back = |p: &str| format!("{p}: {UNREADABLE}");
        assert_eq!(got, Ok(vec![back("s1/s1"), back("s1/s2/s1"), back("s1/s2/s2")]), "each repo once, and a way back up counts as unreadable");
        assert!(took < Duration::from_secs(5), "took {took:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_scan_out_of_time_keeps_the_gate_shut_and_can_be_trusted() {
        let (dir, top, git) = scratch_dir("slow");
        git("", &["init", "-q"]);
        git("sub", &["init", "-q"]);
        std::fs::write(Path::new(&top).join("sub/f"), "x\n").unwrap();
        git("sub", &["add", "f"]);
        git("sub", &["commit", "-qm", "s"]);
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{},sub", git("sub", &["rev-parse", "HEAD"]))]);
        // A config git waits on forever (status would too).
        let config = Path::new(&top).join("sub/.git/config");
        std::fs::remove_file(&config).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(&config).status().unwrap().success());
        let got = programs_within(None, &top, Duration::from_secs(2));
        std::fs::remove_dir_all(&dir).unwrap();
        // Not an error: one would leave no way to trust the folder.
        assert_eq!(got, Ok(vec![UNREADABLE.to_string()]));
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

    /// The scan of `top` kept under `key` ([`programs_here_at`]) as if it
    /// began at `began`, with the git here; and how many times git ran.
    fn kept_scan(key: &str, top: &str, began: SystemTime) -> (Vec<String>, usize) {
        let mut runs = 0;
        let keys = programs_here_at(
            key,
            top,
            |args| {
                runs += 1;
                let out = std::process::Command::new("git").args(READ_FLAGS).args(args).current_dir(top).output()?;
                Ok::<_, std::io::Error>(whole(out.status.code().unwrap_or(-1), out.stdout))
            },
            began,
        )
        .unwrap();
        (keys, runs)
    }

    /// Well after any file a test just wrote ([`RACY`]).
    fn later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(60)
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|k| k.to_string()).collect()
    }

    /// A scratch repo with a commit, its answer kept under `name`.
    #[cfg(unix)]
    fn kept_repo(name: &str) -> (PathBuf, String, impl Fn(&str, &[&str]) -> String) {
        let (dir, top, git) = scratch_dir(name);
        git("", &["init", "-q"]);
        std::fs::write(Path::new(&top).join("f"), "x\n").unwrap();
        git("", &["add", "f"]);
        git("", &["commit", "-qm", "s"]);
        assert_eq!(kept_scan(name, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(name, &top, later()), (keys(&[]), 0), "kept");
        (dir, top, git)
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_answer_holds_while_nothing_it_was_read_from_changes() {
        let (dir, top, _git) = kept_repo("kept-holds");
        // The work tree isn't read.
        std::fs::write(Path::new(&top).join("g"), "y\n").unwrap();
        assert_eq!(kept_scan("kept-holds", &top, later()), (keys(&[]), 0));
        // Another caller, or another git, scans for itself.
        assert_eq!(kept_scan("kept-holds-other", &top, later()), (keys(&[]), 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_repos_own_config_changing_is_scanned_again() {
        let k = "kept-config";
        let (dir, top, git) = kept_repo(k);
        git("", &["config", "core.sshCommand", "true"]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        // The worktree's own, once the repo reads it.
        git("", &["config", "extensions.worktreeConfig", "true"]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1));
        git("", &["config", "--worktree", "diff.external", "true"]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand", "diff.external"]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_config_changed_in_place_or_behind_a_link_is_scanned_again() {
        let k = "kept-in-place";
        let (dir, top, _git) = kept_repo(k);
        let config = Path::new(&top).join(".git/config");
        let text = std::fs::read_to_string(&config).unwrap() + "[core]\n\tsshCommand = a\n";
        std::fs::write(&config, &text).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1));
        // The same file, the same size, its time set back: only its change
        // time tells.
        let was = std::fs::metadata(&config).unwrap().modified().unwrap();
        std::fs::write(&config, text.replace("sshCommand = a", "fsmonitor  = a")).unwrap();
        std::fs::File::options().write(true).open(&config).unwrap().set_modified(was).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.fsmonitor"]), 1));
        // Behind a link: what it leads to changes, the link doesn't.
        let real = dir.join("real-config");
        std::fs::rename(&config, &real).unwrap();
        std::os::unix::fs::symlink(&real, &config).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.fsmonitor"]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        std::io::Write::write_all(&mut std::fs::File::options().append(true).open(&real).unwrap(), b"[diff]\n\texternal = x\n").unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.fsmonitor", "diff.external"]), 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn remote_files_changing_are_scanned_again() {
        let k = "kept-remotes";
        let (dir, top, _git) = kept_repo(k);
        for d in ["remotes", "branches"] {
            let file = Path::new(&top).join(".git").join(d).join("o");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            kept_scan(k, &top, later());
            std::fs::write(&file, "URL: x\n").unwrap();
            assert_eq!(kept_scan(k, &top, later()), (vec![format!(".git/{d}")], 1));
            assert_eq!(kept_scan(k, &top, later()).1, 0);
            std::fs::remove_file(&file).unwrap();
            assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A gitlink not checked out has nothing to read, but may be checked out
    // any time: the index changing scans again, and one there is never kept.
    #[cfg(unix)]
    #[test]
    fn a_gitlink_in_the_index_is_scanned_again_and_never_kept() {
        let k = "kept-link";
        let (dir, top, git) = kept_repo(k);
        git("sub", &["init", "-q"]);
        std::fs::write(Path::new(&top).join("sub/f"), "x\n").unwrap();
        git("sub", &["add", "f"]);
        git("sub", &["commit", "-qm", "s"]);
        git("sub", &["config", "core.sshCommand", "true"]);
        let head = git("sub", &["rev-parse", "HEAD"]);
        let away = dir.join("away");
        std::fs::rename(Path::new(&top).join("sub"), &away).unwrap();
        git("", &["update-index", "--add", "--cacheinfo", &format!("160000,{head},sub")]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1), "kept with a gitlink in the index");
        std::fs::rename(&away, Path::new(&top).join("sub")).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["sub: core.sshcommand"]), 1));
        git("", &["update-index", "--force-remove", "sub"]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // git takes a `.git` only with a `HEAD` it can read, `objects/` and
    // `refs/`; else it goes on to a repo further up.
    #[cfg(unix)]
    #[test]
    fn a_git_dir_git_stops_taking_is_scanned_again() {
        let k = "kept-up";
        let (dir, outer, git) = scratch_dir(k);
        git("", &["init", "-q"]);
        git("", &["config", "core.sshCommand", "true"]);
        git("in", &["init", "-q"]);
        std::fs::write(Path::new(&outer).join("in/f"), "x\n").unwrap();
        git("in", &["add", "f"]);
        git("in", &["commit", "-qm", "s"]);
        let top = format!("{outer}/in");
        let g = Path::new(&top).join(".git");
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        let head = std::fs::read(g.join("HEAD")).unwrap();
        std::fs::write(g.join("HEAD"), "not a ref\n").unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "a HEAD git can't read");
        std::fs::write(g.join("HEAD"), head).unwrap();
        for d in ["objects", "refs"] {
            assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
            assert_eq!(kept_scan(k, &top, later()).1, 0);
            std::fs::rename(g.join(d), g.join("away")).unwrap();
            assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "no {d}");
            std::fs::rename(g.join("away"), g.join(d)).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_top_whose_git_dir_leads_elsewhere_is_scanned_again() {
        let k = "kept-lead";
        let (dir, base, git) = scratch_dir(k);
        git("wt", &["init", "-q", "--separate-git-dir", &format!("{base}/a.git")]);
        std::fs::write(Path::new(&base).join("wt/f"), "x\n").unwrap();
        git("wt", &["add", "f"]);
        git("wt", &["commit", "-qm", "s"]);
        assert!(std::process::Command::new("cp").args(["-R", &format!("{base}/a.git"), &format!("{base}/b.git")]).status().unwrap().success());
        git("", &["config", "--file", &format!("{base}/b.git/config"), "core.sshCommand", "true"]);
        let top = format!("{base}/wt");
        let gitfile = Path::new(&top).join(".git");
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        std::fs::write(&gitfile, format!("gitdir: {base}/b.git\n")).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "a gitfile naming another git dir");
        // A link on the way: the gitfile stays as it was.
        let link = Path::new(&base).join("link");
        std::os::unix::fs::symlink(format!("{base}/a.git"), &link).unwrap();
        std::fs::write(&gitfile, "gitdir: ../link\n").unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()).1, 0);
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(format!("{base}/b.git"), &link).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "a link on the way to the git dir");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A linked worktree: its git dir names the common dir in `commondir`,
    // and each keeps its own `modules/`.
    #[cfg(unix)]
    #[test]
    fn a_linked_worktree_is_kept_until_its_common_dir_leads_elsewhere() {
        let k = "kept-common";
        let (dir, base, git) = scratch_dir(k);
        git("main", &["init", "-q"]);
        std::fs::write(Path::new(&base).join("main/f"), "x\n").unwrap();
        git("main", &["add", "f"]);
        git("main", &["commit", "-qm", "s"]);
        git("main", &["worktree", "add", "-q", &format!("{base}/lw")]);
        let top = format!("{base}/lw");
        let (common, g) = (Path::new(&base).join("main/.git"), Path::new(&base).join("main/.git/worktrees/lw"));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "kept");
        for modules in [g.join("modules"), common.join("modules")] {
            std::fs::create_dir(&modules).unwrap();
            assert_eq!(kept_scan(k, &top, later()).1, 1);
            assert_eq!(kept_scan(k, &top, later()).1, 1, "kept with {}", modules.display());
            std::fs::remove_dir(&modules).unwrap();
            kept_scan(k, &top, later());
        }
        assert!(std::process::Command::new("cp").args(["-R", &common.to_string_lossy(), &format!("{base}/other.git")]).status().unwrap().success());
        git("", &["config", "--file", &format!("{base}/other.git/config"), "core.sshCommand", "true"]);
        let link = Path::new(&base).join("clink");
        std::os::unix::fs::symlink(&common, &link).unwrap();
        std::fs::write(g.join("commondir"), format!("{}\n", link.display())).unwrap();
        assert_eq!(kept_scan(k, &top, later()).0, keys(&[]));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "the same common dir");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(format!("{base}/other.git"), &link).unwrap();
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "a link on the way to the common dir");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn what_could_change_unseen_is_never_kept() {
        let k = "kept-never";
        let (dir, top, git) = kept_repo(k);
        let twice = |why: &str| {
            assert_eq!(kept_scan(k, &top, later()).1, 1, "{why}");
            assert_eq!(kept_scan(k, &top, later()).1, 1, "kept with {why}");
        };
        // What an include names could be anywhere, whatever its case.
        let config = Path::new(&top).join(".git/config");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("{text}[Include]\n\tpath = elsewhere\n")).unwrap();
        twice("an include");
        std::fs::write(&config, &text).unwrap();
        git("", &["config", "extensions.worktreeConfig", "true"]);
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()).1, 0, "kept");
        std::fs::write(Path::new(&top).join(".git/config.worktree"), "[includeIf \"gitdir:/\"]\n\tpath = elsewhere\n").unwrap();
        twice("an include in config.worktree");
        std::fs::remove_file(Path::new(&top).join(".git/config.worktree")).unwrap();
        // A submodule not checked out keeps its git dir there.
        std::fs::create_dir(Path::new(&top).join(".git/modules")).unwrap();
        twice("modules/");
        std::fs::remove_dir(Path::new(&top).join(".git/modules")).unwrap();
        // A scan git didn't finish.
        for _ in 0..2 {
            let mut runs = 0;
            let got = programs_here_at(
                k,
                &top,
                |args| {
                    runs += 1;
                    let out = std::process::Command::new("git").args(READ_FLAGS).args(args).current_dir(&top).output()?;
                    Ok::<_, std::io::Error>((1, out.stdout))
                },
                later(),
            );
            assert_eq!((got.unwrap(), runs), (keys(&[UNREADABLE]), 1));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A coarse clock could give a later change the same time.
    #[cfg(unix)]
    #[test]
    fn an_answer_from_files_changed_just_before_is_never_kept() {
        let k = "kept-racy";
        let (dir, top, git) = scratch_dir(k);
        git("", &["init", "-q"]);
        assert_eq!(kept_scan(k, &top, SystemTime::now()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, SystemTime::now()), (keys(&[]), 1), "kept though its files changed a moment ago");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Kept here, under this caller's key; never for a machine.
    #[cfg(unix)]
    #[test]
    fn a_repo_here_is_kept_and_one_on_a_machine_never() {
        let (dir, top, git) = scratch_dir("kept-wired");
        git("", &["init", "-q"]);
        std::thread::sleep(RACY + Duration::from_millis(200));
        assert_eq!(programs_within(None, &top, TIMEOUT), Ok(Vec::new()));
        assert!(kept().lock().unwrap().contains_key(&format!("yuzora-core:git\0{top}")));
        assert_ne!(programs_within(Some("yuzora-test-no-such-machine"), &top, Duration::from_secs(5)), Ok(Vec::new()), "a machine's answer from what's kept here");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_tops_own_records_make_a_plain_layout() {
        let plain = |extra: &str| plain_layout(format!("sub\0\0legacy\0.git/remotes\0dirs\0/g\n/c\0{extra}end\0{SCAN}\0").as_bytes());
        assert_eq!(plain(""), Some(("/g".into(), "/c".into())));
        for extra in ["links\0\0", "sub\0x\0", "sub\0.\0err\0index\0", "err\0commondir\0", "dirs\0/g\n/c\0", "local\0core.x\0"] {
            assert_eq!(plain(extra), None, "{extra:?}");
        }
        assert_eq!(plain_layout(b"sub\0\0end\0yuzora-trust-scan\0"), None, "no dirs");
        assert_eq!(plain_layout(b"legacy\0x\0dirs\0/g\n/c\0end\0yuzora-trust-scan\0"), None, "not the top's");
        assert_eq!(plain_layout(b"sub\0\0dirs\0/g\0end\0yuzora-trust-scan\0"), None, "one dir");
    }

    // A `.git` swapped for one made earlier while the scan ran: its files'
    // times are as old as they were, so only how they were before tells.
    #[cfg(unix)]
    #[test]
    fn a_git_dir_swapped_in_during_the_scan_is_never_kept() {
        let k = "kept-swap";
        let (dir, top, git) = scratch_dir(k);
        git("", &["init", "-q"]);
        let g = Path::new(&top).join(".git");
        assert!(std::process::Command::new("cp").args(["-R", &g.to_string_lossy(), &dir.join("evil").to_string_lossy()]).status().unwrap().success());
        git("", &["config", "--file", &dir.join("evil/config").to_string_lossy(), "core.sshCommand", "true"]);
        let mut runs = 0;
        let got = programs_here_at(
            k,
            &top,
            |args| {
                runs += 1;
                let out = std::process::Command::new("git").args(READ_FLAGS).args(args).current_dir(&top).output()?;
                std::fs::rename(&g, dir.join("old")).unwrap();
                std::fs::rename(dir.join("evil"), &g).unwrap();
                Ok::<_, std::io::Error>(whole(out.status.code().unwrap_or(-1), out.stdout))
            },
            later(),
        );
        assert_eq!((got.unwrap(), runs), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&["core.sshcommand"]), 1), "kept for the git dir swapped in");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Nothing to look at before the scan (the git dir its gitfile names came
    // only meanwhile): nothing tells the answer is still the one read.
    #[cfg(unix)]
    #[test]
    fn a_scan_with_nothing_before_it_is_never_kept() {
        let k = "kept-nothing-before";
        let (dir, top, git) = scratch_dir(k);
        git("", &["init", "-q"]);
        std::fs::rename(Path::new(&top).join(".git"), dir.join("made.git")).unwrap();
        std::fs::write(Path::new(&top).join(".git"), format!("gitdir: {}\n", dir.join("later.git").display())).unwrap();
        let mut runs = 0;
        let got = programs_here_at(
            k,
            &top,
            |args| {
                runs += 1;
                std::fs::rename(dir.join("made.git"), dir.join("later.git")).unwrap();
                let out = std::process::Command::new("git").args(READ_FLAGS).args(args).current_dir(&top).output()?;
                Ok::<_, std::io::Error>(whole(out.status.code().unwrap_or(-1), out.stdout))
            },
            later(),
        );
        assert_eq!((got.unwrap(), runs), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1), "kept with nothing to look at before the scan");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A split index keeps its entries in `sharedindex.*`, a sparse one
    // expands from trees: files that may change while the index doesn't.
    #[cfg(unix)]
    #[test]
    fn an_index_that_reads_other_files_is_never_kept() {
        let k = "kept-split";
        let (dir, top, git) = kept_repo(k);
        git("", &["update-index", "--split-index"]);
        assert!(std::fs::read_dir(Path::new(&top).join(".git")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("sharedindex.")));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1), "kept with a split index");
        git("", &["update-index", "--no-split-index"]);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "kept");
        for d in ["in", "out"] {
            std::fs::create_dir_all(Path::new(&top).join(d)).unwrap();
            std::fs::write(Path::new(&top).join(d).join("f"), "x\n").unwrap();
        }
        git("", &["add", "in", "out"]);
        git("", &["commit", "-qm", "dirs"]);
        git("", &["sparse-checkout", "set", "--cone", "--sparse-index", "in"]);
        assert!(git("", &["ls-files", "--sparse"]).lines().any(|l| l == "out/"), "the index isn't sparse, so this test proves nothing");
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1));
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 1), "kept with a sparse index");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Every index git writes reads to its end: versions 2 to 4 (3 when an
    // entry has extended flags, 4 with paths shortened), SHA-1 and SHA-256.
    #[cfg(unix)]
    #[test]
    fn indexes_of_each_version_and_hash_are_kept() {
        let k = "kept-versions";
        let (dir, top, git) = kept_repo(k);
        // An entry as long as a multiple of 8 (the header and `ab`) takes 8 NULs.
        // In version 4, `ab` after 129 `a`s drops 128: a number of two bytes,
        // the second NUL.
        for name in ["ab".to_string(), "a".repeat(129)] {
            std::fs::write(Path::new(&top).join(&name), "x\n").unwrap();
            git("", &["add", &name]);
        }
        std::fs::write(Path::new(&top).join("new"), "x\n").unwrap();
        git("", &["add", "-N", "new"]);
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "kept with version 3");
        git("", &["update-index", "--index-version", "4"]);
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "kept with version 4");
        std::fs::remove_dir_all(&dir).unwrap();
        let k = "kept-sha256";
        let (dir, top, git) = scratch_dir(k);
        git("", &["init", "-q", "--object-format=sha256"]);
        std::fs::write(Path::new(&top).join("f"), "x\n").unwrap();
        git("", &["add", "f"]);
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()), (keys(&[]), 0), "kept with SHA-256");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_an_index_that_reads_to_its_end_stands_alone() {
        assert_eq!(index_stands_alone(b""), None);
        assert_eq!(index_stands_alone(b"DIRC\0\0\0\x02\0\0\0\0"), None, "no checksum");
        let empty = [b"DIRC\0\0\0\x02\0\0\0\0".as_slice(), &[0; 20]].concat();
        assert_eq!(index_stands_alone(&empty), Some(true));
        let with = |ext: &[u8]| [b"DIRC\0\0\0\x02\0\0\0\0".as_slice(), ext, b"\0\0\0\x01x", &[0; 20]].concat();
        assert_eq!(index_stands_alone(&with(b"link")), Some(false));
        assert_eq!(index_stands_alone(&with(b"sdir")), Some(false));
        assert_eq!(index_stands_alone(&with(b"TREE")), Some(true));
        // One entry, `a`: 62 bytes before its path, 1 NUL after it.
        let entry = |len: u8| [b"DIRC\0\0\0\x02\0\0\0\x01".as_slice(), &[0; 60], &[0, len], b"a\0", &[0; 20]].concat();
        assert_eq!(index_stands_alone(&entry(1)), Some(true));
        assert_eq!(index_stands_alone(&entry(2)), None, "flags that don't hold the path's length");
        // No entries, then TREE (empty) and link (4 bytes): with 32-byte ids the
        // checksum begins after TREE, with 20-byte ones after link.
        let both = [b"DIRC\0\0\0\x02\0\0\0\0TREE\0\0\0\0link\0\0\0\x04xxxx".as_slice(), &[0; 20]].concat();
        assert_eq!((index_extensions(&both, 20).map(|e| e.len()), index_extensions(&both, 32).map(|e| e.len())), (Some(2), Some(1)));
        assert_eq!(index_stands_alone(&both), None, "read as an index either way");
        let past = [b"DIRC\0\0\0\x02\0\0\0\0TREE\0\0\0\x64x".as_slice(), &[0; 20]].concat();
        assert_eq!(index_stands_alone(&past), None, "an extension past the checksum");
        assert_eq!(index_stands_alone(&[b"DIRC\0\0\0\x05\0\0\0\0".as_slice(), &[0; 20]].concat()), None, "version 5");
    }

    // What git passes over may be anything: a FIFO there would wait for a
    // writer for ever, so only regular files are read, within a size.
    #[cfg(unix)]
    #[test]
    fn what_isnt_a_regular_file_is_never_read() {
        let k = "kept-fifo";
        let (dir, top, git) = kept_repo(k);
        let mkfifo = |path: &Path| assert!(std::process::Command::new("mkfifo").arg(path).status().unwrap().success());
        // Read only with extensions.worktreeConfig: git passes it over here.
        mkfifo(&Path::new(&top).join(".git/config.worktree"));
        let (done, answers) = std::sync::mpsc::channel();
        let there = top.clone();
        std::thread::spawn(move || {
            let mut got = vec![kept_scan(k, &there, later()), kept_scan(k, &there, later())];
            // A `.git` that is a FIFO: neither a folder nor a gitfile.
            std::fs::rename(Path::new(&there).join(".git"), Path::new(&there).join("away.git")).unwrap();
            mkfifo(&Path::new(&there).join(".git"));
            got.push(kept_scan(k, &there, later()));
            let _ = done.send(got);
        });
        let got = answers.recv_timeout(Duration::from_secs(60)).expect("a FIFO was read: the check waits for ever");
        assert_eq!((got[0].1, got[1].1), (1, 1), "kept with a FIFO for config.worktree");
        assert_eq!(got[2].1, 1);
        std::fs::remove_file(Path::new(&top).join(".git")).unwrap();
        std::fs::rename(Path::new(&top).join("away.git"), Path::new(&top).join(".git")).unwrap();
        std::fs::remove_file(Path::new(&top).join(".git/config.worktree")).unwrap();
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()).1, 0, "kept");
        // A config past what is read.
        let config = Path::new(&top).join(".git/config");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("{text}# {}\n", "x".repeat(ANSWER_CAP))).unwrap();
        assert_eq!(kept_scan(k, &top, later()).1, 1);
        assert_eq!(kept_scan(k, &top, later()).1, 1, "kept with a config past the cap");
        drop(git);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
