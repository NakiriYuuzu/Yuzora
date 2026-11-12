//! Status, kept per repo and refreshed by dirty scope (IntelliJ B4):
//!
//! - tracked changes: `git status --porcelain=v2 -z --branch
//!   --untracked-files=no [-- <dirty paths>]`;
//! - untracked files take their own path: `git ls-files --others
//!   --exclude-standard -z [-- <dirty paths>]`;
//! - `+/-` per file: `git diff [--cached] --numstat -z -M [-- <dirty paths>]`.
//!
//! The watcher marks paths dirty ([`StatusCache::dirty`]); a directory with
//! more than [`FOLD_CHILDREN`] dirty children folds into the directory, and
//! more than [`ALL_DIRTY`] paths, or a pathspec past the 7600-character
//! limit, means the whole root. Anything under `.git` (index, HEAD, refs)
//! makes everything dirty.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{Value, json};

use crate::exec::{Git, GitError, GitResult};
use crate::repo::{Operation, Repo};

/// IntelliJ's `RootDirtySet`: 30 children fold into their directory…
pub const FOLD_CHILDREN: usize = 30;
/// …and 50 000 paths are the whole root.
pub const ALL_DIRTY: usize = 50_000;
/// Files past this many in one section are counted but not listed.
pub const LIST_LIMIT: usize = 5000;
/// Line counts are skipped when this many files changed (each costs a diff).
pub const NUMSTAT_LIMIT: usize = 3000;
/// Untracked files are counted line by line up to this size.
const COUNT_LINES_MAX: u64 = 2 << 20;

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    All,
    Paths(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    pub head: Option<String>,
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub detached: bool,
    /// No commit yet.
    pub initial: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// The path before a rename or copy (staged).
    pub orig: Option<String>,
    /// Index status (`.` unchanged, `M`, `T`, `A`, `D`, `R`, `C`, `U`).
    pub x: char,
    /// Work tree status.
    pub y: char,
    /// `S<c><m><u>` for a submodule.
    pub submodule: Option<String>,
    pub conflict: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stat {
    pub add: u32,
    pub del: u32,
    pub binary: bool,
}

#[derive(Clone, Debug, Default)]
enum Dirty {
    #[default]
    All,
    Paths(BTreeSet<String>),
}

#[derive(Debug, Default)]
pub struct StatusCache {
    pub branch: Branch,
    pub tracked: BTreeMap<String, Entry>,
    pub untracked: BTreeSet<String>,
    pub staged_stats: HashMap<String, Stat>,
    pub unstaged_stats: HashMap<String, Stat>,
    pub untracked_lines: HashMap<String, u32>,
    /// Ignored directories, so the watcher can skip a build writing into them.
    pub ignored_dirs: Vec<String>,
    /// Submodule paths: a change inside one is a change of its gitlink.
    /// `None` when the index couldn't be listed whole: then a refresh of
    /// some paths reads everything instead.
    pub gitlinks: Option<Vec<String>>,
    /// How paths that aren't UTF-8 are shown: no call acts on them by that
    /// name ([`refuse_lossy`]).
    pub lossy: BTreeSet<String>,
    pub stash: u32,
    pub operation: Operation,
    pub loaded: bool,
    pub numstat_skipped: bool,
    pending: Option<Dirty>,
}

impl StatusCache {
    /// Marks a scope stale; the next [`refresh`] reads it again.
    pub fn dirty(&mut self, scope: Scope) {
        self.pending = Some(match (self.pending.take(), scope) {
            (_, Scope::All) | (Some(Dirty::All), _) => Dirty::All,
            (None, Scope::Paths(p)) => fold(p.into_iter().collect()),
            (Some(Dirty::Paths(mut old)), Scope::Paths(p)) => {
                old.extend(p);
                fold(old)
            }
        });
    }

    pub fn is_dirty(&self) -> bool {
        !self.loaded || self.pending.is_some()
    }

    fn take_pending(&mut self) -> Option<Dirty> {
        if !self.loaded {
            self.pending = None;
            return Some(Dirty::All);
        }
        self.pending.take()
    }
}

/// Folds crowded directories and gives up past [`ALL_DIRTY`].
fn fold(mut paths: BTreeSet<String>) -> Dirty {
    loop {
        if paths.len() > ALL_DIRTY || paths.iter().any(|p| p.is_empty()) {
            return Dirty::All;
        }
        let mut per_dir: HashMap<&str, usize> = HashMap::new();
        for p in &paths {
            *per_dir.entry(parent(p)).or_default() += 1;
        }
        let crowded: HashSet<String> = per_dir.into_iter().filter(|(_, n)| *n > FOLD_CHILDREN).map(|(d, _)| d.to_owned()).collect();
        if crowded.is_empty() {
            // A directory already in the set covers its children: look up
            // each path's ancestors, never every other path.
            let covered: Vec<String> = paths.iter().filter(|p| ancestors(p).any(|d| paths.contains(d))).cloned().collect();
            for p in covered {
                paths.remove(&p);
            }
            return Dirty::Paths(paths);
        }
        if crowded.contains("") {
            return Dirty::All;
        }
        paths.retain(|p| !crowded.contains(parent(p)));
        paths.extend(crowded);
    }
}

fn parent(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
}

/// `a/b` and `a` for `a/b/c`.
fn ancestors(p: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(parent(p)), |d| Some(parent(d))).take_while(|d| !d.is_empty())
}

/// Whether pathspec `spec` (a file or a directory) covers `path`.
pub fn covers(spec: &str, path: &str) -> bool {
    path == spec || (path.len() > spec.len() && path.starts_with(spec) && path.as_bytes()[spec.len()] == b'/')
}

/// What `git status --porcelain=v2 -z --branch --show-stash` printed.
#[derive(Debug, Default, PartialEq)]
pub struct Porcelain {
    pub branch: Branch,
    pub entries: Vec<Entry>,
    pub untracked: Vec<String>,
    pub stash: u32,
    /// How the paths that aren't UTF-8 are shown (U+FFFD for each bad byte).
    pub lossy: Vec<String>,
}

pub fn parse_porcelain_v2(out: &[u8]) -> Porcelain {
    let mut p = Porcelain::default();
    // Each field, and whether it had to be shown other than it is.
    let mut fields = out.split(|b| *b == 0).map(|f| (String::from_utf8_lossy(f).into_owned(), std::str::from_utf8(f).is_err()));
    while let Some((rec, lossy)) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" => {
                    p.branch.initial = v == "(initial)";
                    p.branch.oid = (!p.branch.initial).then(|| v.to_owned());
                }
                "branch.head" => {
                    p.branch.detached = v == "(detached)";
                    p.branch.head = (!p.branch.detached).then(|| v.to_owned());
                }
                "branch.upstream" => p.branch.upstream = Some(v.to_owned()),
                "branch.ab" => {
                    for n in v.split(' ') {
                        if let Some(a) = n.strip_prefix('+') {
                            p.branch.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = n.strip_prefix('-') {
                            p.branch.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                "stash" => p.stash = v.parse().unwrap_or(0),
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' | b'2' | b'u' => {
                // 1 XY sub mH mI mW hH hI path
                // 2 XY sub mH mI mW hH hI Xscore path\0orig
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let n = match kind {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                let parts: Vec<&str> = rec.splitn(n, ' ').collect();
                if parts.len() < n {
                    continue;
                }
                let xy: Vec<char> = parts[1].chars().collect();
                let sub = parts[2];
                let path = parts[n - 1].to_owned();
                let orig = if kind == b'2' { fields.next() } else { None };
                if lossy {
                    p.lossy.push(path.clone());
                }
                let orig = orig.map(|(o, bad)| {
                    if bad {
                        p.lossy.push(o.clone());
                    }
                    o
                });
                p.entries.push(Entry {
                    path,
                    orig,
                    x: xy.first().copied().unwrap_or('.'),
                    y: xy.get(1).copied().unwrap_or('.'),
                    submodule: sub.starts_with('S').then(|| sub.to_owned()),
                    conflict: kind == b'u',
                });
            }
            b'?' => {
                if lossy {
                    p.lossy.push(rec[2..].to_owned());
                }
                p.untracked.push(rec[2..].to_owned())
            }
            _ => {}
        }
    }
    p
}

/// `git diff --numstat -z` (with renames: `a\td\t\0old\0new\0`).
pub fn parse_numstat(out: &[u8]) -> HashMap<String, Stat> {
    let mut m = HashMap::new();
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let mut cols = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (cols.next(), cols.next(), cols.next()) else { continue };
        let path = if path.is_empty() {
            let _old = fields.next();
            match fields.next() {
                Some(new) => new,
                None => continue,
            }
        } else {
            path.to_owned()
        };
        let binary = a == "-";
        m.insert(path, Stat { add: a.parse().unwrap_or(0), del: d.parse().unwrap_or(0), binary });
    }
    m
}

/// The `status` call: the cache brought up to date (`force`: all of it read
/// again). While the repo's own config names programs and its folder isn't
/// trusted, no git runs: what is known so far, with `trust` listing them.
pub fn read(repo: &Repo, force: bool) -> GitResult<Value> {
    if force {
        repo.status.lock().unwrap_or_else(|e| e.into_inner()).dirty(Scope::All);
    }
    let pending = crate::trust::pending(repo)?;
    if !pending.is_empty() {
        let mut v = snapshot(repo);
        v["trust"] = json!(pending);
        return Ok(v);
    }
    refresh(repo)?;
    Ok(snapshot(repo))
}

/// Brings the cache up to date (a no-op when nothing is dirty).
pub fn refresh(repo: &Repo) -> GitResult<()> {
    let _one = repo.refreshing.lock().unwrap_or_else(|e| e.into_inner());
    let pending = repo.status.lock().unwrap_or_else(|e| e.into_inner()).take_pending();
    let Some(pending) = pending else { return Ok(()) };
    let r = match pending {
        Dirty::All => full(repo),
        Dirty::Paths(p) => partial(repo, p.into_iter().collect()),
    };
    if r.is_err() {
        // Read it all next time.
        repo.status.lock().unwrap_or_else(|e| e.into_inner()).loaded = false;
    }
    r
}

/// `--branch` counts ahead/behind, walking to the upstream's commit.
fn status_cmd(repo: &Repo, cap: usize) -> Git {
    crate::trust::own_read(repo, Git::read(["status", "--porcelain=v2", "-z", "--branch", "--show-stash", "--untracked-files=no"])).env("GIT_LITERAL_PATHSPECS", "1").max_output(cap + 1)
}

fn untracked_cmd(cap: usize) -> Git {
    Git::read(["ls-files", "--others", "--exclude-standard", "-z"]).env("GIT_LITERAL_PATHSPECS", "1").max_output(cap + 1)
}

/// A listing read whole: past `cap` it was cut short, and a short list would
/// leave changes out without saying so.
fn whole(out: crate::exec::Output, cap: usize) -> GitResult<Vec<u8>> {
    let out = out.into_result()?;
    if out.len() > cap {
        return Err(GitError::Other(format!("git listed more than {} MiB of changes: too many to show", cap >> 20)));
    }
    Ok(out)
}

/// The most a status listing may print.
const OUTPUT_CAP: usize = crate::exec::DEFAULT_MAX_OUTPUT;

#[cfg(test)]
thread_local! {
    /// [`OUTPUT_CAP`] for listings run on this thread (tests).
    static CAP: std::cell::Cell<usize> = const { std::cell::Cell::new(OUTPUT_CAP) };
}

fn cap() -> usize {
    #[cfg(test)]
    return CAP.with(|c| c.get());
    #[cfg(not(test))]
    OUTPUT_CAP
}

/// Joins a scoped read; a panic there is an error here, not a second panic.
fn joined<T>(h: std::thread::ScopedJoinHandle<'_, GitResult<T>>) -> GitResult<T> {
    h.join().unwrap_or_else(|_| Err(GitError::Other("a status read panicked".into())))
}

fn full(repo: &Repo) -> GitResult<()> {
    let cap = cap();
    // The reads don't depend on each other (and take no lock): side by side,
    // a full refresh costs its slowest command rather than their sum.
    let (st, untracked, ignored_dirs, gitlinks, operation, counts) = std::thread::scope(|s| {
        // yuzora(perf): what is in progress (a machine: one more round trip) is read with the rest.
        let operation = s.spawn(|| crate::repo::operation(repo));
        let untracked = s.spawn(|| -> GitResult<(BTreeSet<String>, Vec<String>)> {
            let out = whole(repo.run(untracked_cmd(cap))?, cap)?;
            Ok((split_z(&out).collect(), lossy_z(&out).collect()))
        });
        let ignored = s.spawn(|| -> GitResult<Vec<String>> {
            if repo.place.is_remote() {
                return Ok(Vec::new());
            }
            Ok(split_z(&repo.run(Git::read(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]))?.into_result().unwrap_or_default())
                .filter_map(|p| p.strip_suffix('/').map(str::to_owned))
                .collect())
        });
        let links = s.spawn(|| gitlinks(repo, cap));
        let st = repo.run(status_cmd(repo, cap)).and_then(|o| whole(o, cap)).map(|o| parse_porcelain_v2(&o));
        // yuzora(perf): the change counts need only how many files changed, which `status` answers well before the two
        // listings of the whole work tree (the slowest reads here): they start then, beside those, not after them all.
        let counts = match &st {
            Ok(st) if st.entries.len() <= NUMSTAT_LIMIT => {
                let staged = s.spawn(|| numstat(repo, true, &[]));
                let unstaged = s.spawn(|| numstat(repo, false, &[]));
                Some((staged, unstaged))
            }
            _ => None,
        };
        (
            st,
            joined(untracked),
            joined(ignored),
            links.join().unwrap_or_default(),
            operation.join(),
            counts.map(|(staged, unstaged)| (joined(staged), joined(unstaged))),
        )
    });
    let operation = operation.map_err(|_| GitError::Other("a status read panicked".into()))?;
    let (st, (untracked, untracked_lossy), ignored_dirs) = (st?, untracked?, ignored_dirs?);
    let (staged, unstaged, skipped) = match counts {
        Some((staged, unstaged)) => (staged?, unstaged?, false),
        None => (HashMap::new(), HashMap::new(), true),
    };
    let lines = count_untracked(repo, untracked.iter());
    let mut c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    c.gitlinks = gitlinks;
    c.lossy = st.lossy.into_iter().chain(untracked_lossy).collect();
    c.branch = st.branch;
    c.stash = st.stash;
    c.tracked = st.entries.into_iter().map(|e| (e.path.clone(), e)).collect();
    c.untracked = untracked;
    c.staged_stats = staged;
    c.unstaged_stats = unstaged;
    c.untracked_lines = lines;
    c.ignored_dirs = ignored_dirs;
    c.operation = operation;
    c.numstat_skipped = skipped;
    c.loaded = true;
    Ok(())
}

/// The gitlinks in the index, whether or not `.gitmodules` names them:
/// status reports what changed inside one as the gitlink. `None` when the
/// listing failed or was cut short (an index past the output cap): a prefix
/// would miss the gitlinks after it.
fn gitlinks(repo: &Repo, cap: usize) -> Option<Vec<String>> {
    // `<mode> <oid> <stage>\t<path>\0`
    let index = repo.run(Git::read(["ls-files", "-s", "-z"]).max_output(cap + 1)).ok().filter(|o| o.ok() && o.stdout.len() <= cap)?.stdout;
    let mut links: Vec<String> = split_z(&index).filter_map(|e| e.split_once(' ').filter(|(mode, _)| is_gitlink(mode)).and_then(|(_, e)| e.split_once('\t')).map(|(_, p)| p.to_owned())).collect();
    // A conflict lists a path once per stage.
    links.dedup();
    Some(links)
}

/// Whether a mode as `ls-files -s` prints it (octal) is a gitlink's: by its
/// type bits, as git tells one (`S_ISGITLINK`), whatever the others are (an
/// index made by hand may set them).
fn is_gitlink(mode: &str) -> bool {
    u32::from_str_radix(mode, 8).is_ok_and(|m| m & 0o170000 == 0o160000)
}

fn partial(repo: &Repo, mut paths: Vec<String>) -> GitResult<()> {
    let cap = cap();
    {
        let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
        // Asked for by that name, git would list only the file that really
        // has it and the cache would lose the other one. And without every
        // gitlink, a file inside one can't be asked about as its gitlink.
        if c.gitlinks.is_none() || paths.iter().any(|p| shown_for_lossy(&c.lossy, p)) {
            drop(c);
            return full(repo);
        }
        // git reports a submodule's own files as its gitlink, so ask for that.
        for p in &mut paths {
            if let Some(g) = c.gitlinks.iter().flatten().find(|g| p.starts_with(g.as_str()) && p.as_bytes().get(g.len()) == Some(&b'/')) {
                *p = g.clone();
            }
        }
        // Renames, adds and deletes are re-read with the rest, so a rename is
        // still seen as one when only one of its sides changed.
        for e in c.tracked.values() {
            if matches!(e.x, 'A' | 'D' | 'R' | 'C') {
                paths.push(e.path.clone());
                paths.extend(e.orig.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    let (cmd, kept) = status_cmd(repo, cap).paths_or_all(&paths);
    if !kept {
        return full(repo);
    }
    let st = parse_porcelain_v2(&whole(repo.run(cmd)?, cap)?);
    let (ls, _) = untracked_cmd(cap).paths_or_all(&paths);
    let out = whole(repo.run(ls)?, cap)?;
    let untracked: Vec<String> = split_z(&out).collect();
    let staged = numstat(repo, true, &paths)?;
    let unstaged = numstat(repo, false, &paths)?;
    let lines = count_untracked(repo, untracked.iter());
    let operation = crate::repo::operation(repo);
    let mut c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    forget(&mut c, &paths);
    c.lossy.extend(st.lossy);
    c.lossy.extend(lossy_z(&out));
    c.branch = st.branch;
    c.stash = st.stash;
    for e in st.entries {
        c.tracked.insert(e.path.clone(), e);
    }
    c.untracked.extend(untracked);
    c.staged_stats.extend(staged);
    c.unstaged_stats.extend(unstaged);
    c.untracked_lines.extend(lines);
    c.operation = operation;
    Ok(())
}

/// Drops what the cache knows inside `paths`, which were read again. Each
/// entry looks itself and its folders up, rather than going through `paths`.
fn forget(c: &mut StatusCache, paths: &[String]) {
    let read: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let inside = |p: &str| read.contains(p) || ancestors(p).any(|d| read.contains(d));
    c.tracked.retain(|p, _| !inside(p));
    c.untracked.retain(|p| !inside(p));
    c.staged_stats.retain(|p, _| !inside(p));
    c.unstaged_stats.retain(|p, _| !inside(p));
    c.untracked_lines.retain(|p, _| !inside(p));
    c.lossy.retain(|p| !inside(p));
}

fn numstat(repo: &Repo, cached: bool, paths: &[String]) -> GitResult<HashMap<String, Stat>> {
    let mut cmd = crate::trust::own_read(repo, Git::read(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--no-textconv"])).env("GIT_LITERAL_PATHSPECS", "1");
    if cached {
        cmd = cmd.arg("--cached");
    }
    let (cmd, _) = cmd.paths_or_all(paths);
    Ok(parse_numstat(&repo.run(cmd)?.into_result()?))
}

fn count_untracked<'a>(repo: &Repo, paths: impl Iterator<Item = &'a String>) -> HashMap<String, u32> {
    if repo.place.is_remote() {
        return HashMap::new();
    }
    paths
        .take(500)
        .filter_map(|p| {
            let abs = repo.abs(p);
            // A link counts as git records it, its target: never the file
            // it points to (which may be outside the work tree).
            let meta = std::fs::symlink_metadata(&abs).ok()?;
            let bytes = if meta.file_type().is_symlink() {
                crate::repo::link_bytes(std::fs::read_link(&abs).ok()?)
            } else if meta.is_file() && meta.len() <= COUNT_LINES_MAX {
                std::fs::read(&abs).ok()?
            } else {
                return None;
            };
            if bytes[..bytes.len().min(8000)].contains(&0) {
                return None;
            }
            let n = bytes.iter().filter(|b| **b == b'\n').count() + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
            Some((p.clone(), n as u32))
        })
        .collect()
}

/// How the `-z` fields that aren't UTF-8 are shown.
pub fn lossy_z(out: &[u8]) -> impl Iterator<Item = String> + '_ {
    out.split(|b| *b == 0).filter(|f| std::str::from_utf8(f).is_err()).map(|f| String::from_utf8_lossy(f).into_owned())
}

/// Whether `name` is (or is a folder of) how a path that isn't UTF-8 is
/// shown: given to git, it is other bytes, maybe another file's name.
fn shown_for_lossy(lossy: &BTreeSet<String>, name: &str) -> bool {
    if !name.contains('\u{FFFD}') {
        return false;
    }
    let dir = format!("{name}/");
    lossy.contains(name) || lossy.range(dir.clone()..).next().is_some_and(|l| l.starts_with(&dir))
}

/// Refuses a call whose arguments name a path by how it is shown when that
/// path isn't UTF-8 ([`StatusCache::lossy`]).
pub fn refuse_lossy(repo: &Repo, args: &[Value]) -> GitResult<()> {
    fn find<'a>(v: &'a Value, lossy: &BTreeSet<String>) -> Option<&'a str> {
        match v {
            Value::String(s) => shown_for_lossy(lossy, s).then_some(s.as_str()),
            Value::Array(a) => a.iter().find_map(|v| find(v, lossy)),
            Value::Object(o) => o.values().find_map(|v| find(v, lossy)),
            _ => None,
        }
    }
    let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    if c.lossy.is_empty() {
        return Ok(());
    }
    let Some(path) = args.iter().find_map(|a| find(a, &c.lossy)) else { return Ok(()) };
    // In the shown language (the sidebar's strings); tests keep to English.
    let text = if cfg!(test) { String::new() } else { yuzora_core::i18n::translate("yuzora-sidebar-git", "path.lossy", &[("path", path.to_owned())]) };
    Err(GitError::Other(if text.is_empty() || text == "path.lossy" { format!("{path}: this name isn't UTF-8, so it can't be handed to git as it is; use a terminal for it") } else { text }))
}


pub fn split_z(out: &[u8]) -> impl Iterator<Item = String> + '_ {
    out.split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned())
}

/// A display letter for one side of a status.
fn letter(c: char) -> &'static str {
    match c {
        'A' => "A",
        'D' => "D",
        'R' => "R",
        'C' => "C",
        'T' | 'M' => "M",
        'U' => "U",
        _ => "M",
    }
}

/// The cache as scripts see it.
pub fn snapshot(repo: &Repo) -> Value {
    let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    let stat = |m: &HashMap<String, Stat>, p: &str| m.get(p).copied();
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut conflicts = Vec::new();
    for e in c.tracked.values() {
        if e.conflict {
            conflicts.push(json!({ "path": e.path, "kind": format!("{}{}", e.x, e.y) }));
            continue;
        }
        if e.x != '.' {
            let s = stat(&c.staged_stats, &e.path);
            staged.push(json!({
                "path": e.path, "orig": e.orig, "kind": letter(e.x),
                "add": s.map(|s| s.add), "del": s.map(|s| s.del), "binary": s.is_some_and(|s| s.binary),
                "submodule": e.submodule,
            }));
        }
        if e.y != '.' {
            let s = stat(&c.unstaged_stats, &e.path);
            unstaged.push(json!({
                "path": e.path, "kind": letter(e.y),
                "add": s.map(|s| s.add), "del": s.map(|s| s.del), "binary": s.is_some_and(|s| s.binary),
                "submodule": e.submodule,
            }));
        }
    }
    let untracked: Vec<Value> = c.untracked.iter().take(LIST_LIMIT).map(|p| json!({ "path": p, "kind": "?", "add": c.untracked_lines.get(p), "del": 0, "binary": false })).collect();
    let mut changed: BTreeSet<&str> = c.tracked.keys().map(String::as_str).collect();
    changed.extend(c.untracked.iter().map(String::as_str));
    let counts = json!({ "staged": staged.len(), "unstaged": unstaged.len(), "untracked": c.untracked.len(), "conflicts": conflicts.len(), "total": changed.len() });
    let truncated = staged.len() > LIST_LIMIT || unstaged.len() > LIST_LIMIT || c.untracked.len() > LIST_LIMIT;
    staged.truncate(LIST_LIMIT);
    unstaged.truncate(LIST_LIMIT);
    let b = &c.branch;
    let op = &c.operation;
    json!({
        "repo": repo.id,
        "root": repo.place.root,
        "machine": repo.place.on,
        "generation": repo.generation(),
        "branch": { "head": b.head, "oid": b.oid, "upstream": b.upstream, "ahead": b.ahead, "behind": b.behind, "detached": b.detached, "initial": b.initial },
        "operation": { "kind": if op.kind.is_empty() { "none" } else { op.kind.as_str() }, "step": op.step.map(|(n, t)| json!([n, t])), "onto": op.onto, "head": op.head, "interactive": op.interactive },
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
        "conflicts": conflicts,
        "counts": counts,
        "stash": c.stash,
        "truncated": truncated,
        "numstatSkipped": c.numstat_skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;

    #[test]
    fn porcelain_v2_records_parse() {
        let out = b"# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0# stash 3\0\
1 .M N... 100644 100644 100644 aaa aaa src/a b.rs\0\
2 R. N... 100644 100644 100644 aaa aaa R100 new name.rs\0old name.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
1 M. S.M. 160000 160000 160000 aaa bbb vendor/sub\0\
? fresh.txt\0";
        let p = parse_porcelain_v2(out);
        assert_eq!(p.branch.head.as_deref(), Some("main"));
        assert_eq!(p.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!((p.branch.ahead, p.branch.behind, p.stash), (2, 1, 3));
        assert_eq!(p.entries.len(), 4);
        assert_eq!(p.entries[0].path, "src/a b.rs");
        assert_eq!((p.entries[0].x, p.entries[0].y), ('.', 'M'));
        assert_eq!(p.entries[1].path, "new name.rs");
        assert_eq!(p.entries[1].orig.as_deref(), Some("old name.rs"));
        assert!(p.entries[2].conflict);
        assert_eq!(p.entries[3].submodule.as_deref(), Some("S.M."));
        assert_eq!(p.untracked, ["fresh.txt"]);
        let detached = parse_porcelain_v2(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert!(detached.branch.initial && detached.branch.detached);
    }

    #[test]
    fn numstat_reads_renames_and_binaries() {
        let m = parse_numstat(b"3\t1\ta.txt\0-\t-\timg.png\0" .as_slice());
        assert_eq!(m["a.txt"], Stat { add: 3, del: 1, binary: false });
        assert!(m["img.png"].binary);
        let r = parse_numstat(b"2\t0\t\0old.txt\0new.txt\0");
        assert_eq!(r["new.txt"].add, 2);
    }

    #[test]
    fn dirty_paths_fold_into_their_directory_and_then_the_root() {
        let mut c = StatusCache { loaded: true, ..Default::default() };
        c.dirty(Scope::Paths(vec!["a/x.txt".into(), "b.txt".into()]));
        c.dirty(Scope::Paths((0..31).map(|i| format!("a/f{i}.txt")).collect()));
        match c.pending.clone().unwrap() {
            Dirty::Paths(p) => assert_eq!(p.into_iter().collect::<Vec<_>>(), ["a", "b.txt"]),
            Dirty::All => panic!("folded too far"),
        }
        c.dirty(Scope::Paths((0..31).map(|i| format!("top{i}.txt")).collect()));
        assert!(matches!(c.pending, Some(Dirty::All)));
        assert!(covers("a", "a/b/c") && covers("a/b", "a/b") && !covers("a", "ab/c"));
    }

    /// The change counts need only the list of changed files, so they start the moment it is read, and run
    /// beside the listings of the whole work tree (the longest reads of a refresh) instead of after them.
    #[cfg(unix)]
    #[test]
    fn the_change_counts_are_read_beside_the_listings_of_the_work_tree() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.write("a.txt", "2\n");
        let dir = s.dir.path().join("shim");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let real = String::from_utf8(
            std::process::Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        // Each call notes when it starts and ends under the name of its command; `ls-files` takes a while.
        let body = "#!/bin/sh\nname=\"$1\"\nfor a in \"$@\"; do case \"$a\" in ls-files|diff|status) name=$a; break ;; esac; done\necho \"s $name\" >> 'LOG'\n[ \"$name\" = ls-files ] && sleep 0.6\n'REAL' \"$@\"; rc=$?\necho \"e $name\" >> 'LOG'\nexit $rc\n"
            .replace("LOG", &log.display().to_string())
            .replace("REAL", &real);
        let shim = dir.join("git");
        std::fs::write(&shim, body).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let place = crate::exec::Place {
            git: shim.to_string_lossy().into_owned(),
            ..s.repo.place.clone()
        };
        let git_dir = s.repo.git_dir.clone();
        let repo = Arc::new(Repo::new(
            s.repo.id.clone(),
            place,
            git_dir.clone(),
            git_dir,
        ));
        refresh(&repo).unwrap();
        let events: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let at = |what: &str, last: bool| {
            let mut found = events
                .iter()
                .enumerate()
                .filter(|(_, e)| *e == what)
                .map(|(i, _)| i);
            let index = if last {
                found.next_back()
            } else {
                found.next()
            };
            index.unwrap_or_else(|| panic!("{what} never happened: {events:?}"))
        };
        assert!(
            at("s diff", false) < at("e ls-files", true),
            "the counts waited for the listings: {events:?}"
        );
        let c = repo.status.lock().unwrap();
        assert_eq!(
            c.unstaged_stats["a.txt"].add, 1,
            "and still counted the change"
        );
    }

    #[test]
    fn refresh_reads_everything_then_only_the_dirty_paths() {
        let s = scratch();
        s.commit("keep.txt", "1\n2\n3\n", "first");
        s.commit("dir/old.txt", "x\n", "second");
        s.write("keep.txt", "1\n2\nthree\nfour\n");
        s.git(&["mv", "dir/old.txt", "dir/new.txt"]);
        s.write("new file.txt", "a\nb");
        refresh(&s.repo).unwrap();
        let v = snapshot(&s.repo);
        assert_eq!(v["branch"]["head"], "main");
        assert_eq!(v["counts"]["total"], 3);
        assert_eq!(v["unstaged"][0]["path"], "keep.txt");
        assert_eq!((v["unstaged"][0]["add"].as_u64(), v["unstaged"][0]["del"].as_u64()), (Some(2), Some(1)));
        assert_eq!(v["staged"][0]["kind"], "R");
        assert_eq!(v["staged"][0]["orig"], "dir/old.txt");
        assert_eq!(v["untracked"][0]["path"], "new file.txt");
        assert_eq!(v["untracked"][0]["add"], 2);
        // Nothing dirty: a refresh runs no git at all.
        assert!(!s.repo.status.lock().unwrap().is_dirty());
        // One file changes back; only it is read again.
        s.write("keep.txt", "1\n2\n3\n");
        s.repo.status.lock().unwrap().dirty(Scope::Paths(vec!["keep.txt".into()]));
        refresh(&s.repo).unwrap();
        let v = snapshot(&s.repo);
        assert_eq!(v["counts"]["unstaged"], 0);
        assert_eq!(v["counts"]["staged"], 1, "the rename stays");
        assert_eq!(v["counts"]["untracked"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn an_untracked_link_counts_its_target_not_the_file_it_points_to() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        // A link to a three-line file outside the repo: git records the
        // link's target (one line), and nothing reads the file.
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("far.txt"), "1\n2\n3\n").unwrap();
        std::os::unix::fs::symlink(out.path().join("far.txt"), format!("{}/link", s.root())).unwrap();
        refresh(&s.repo).unwrap();
        let v = snapshot(&s.repo);
        assert_eq!(v["untracked"][0]["path"], "link");
        assert_eq!(v["untracked"][0]["add"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn status_never_runs_the_repos_fsmonitor_program() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let marker = format!("{}/ran", s.dir.path().display());
        let hook = format!("{}/fsmonitor.sh", s.dir.path().display());
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{marker}'\n")).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["config", "core.fsmonitor", &hook]);
        s.write("a.txt", "2\n");
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["counts"]["unstaged"], 1);
        assert!(!std::path::Path::new(&marker).exists(), "status ran the repo's fsmonitor program");
    }

    #[test]
    fn a_change_inside_a_submodule_refreshes_its_gitlink() {
        let lib = scratch();
        lib.commit("f.txt", "1\n", "lib");
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.root(), "sub"]);
        s.git(&["commit", "-qm", "sub"]);
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["counts"]["total"], 0);
        assert_eq!(s.repo.status.lock().unwrap().gitlinks, Some(vec!["sub".to_string()]));
        // What the watcher sees is the file inside; the change is the gitlink's.
        s.write("sub/f.txt", "2\n");
        s.repo.invalidate(Scope::Paths(vec!["sub/f.txt".into()]));
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["unstaged"][0]["path"], "sub");
        s.write("sub/f.txt", "1\n");
        s.repo.invalidate(Scope::Paths(vec!["sub/f.txt".into()]));
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["counts"]["total"], 0, "the old gitlink entry stayed");
    }

    #[test]
    fn a_change_inside_a_gitlink_no_gitmodules_names_refreshes_it() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        // A repo inside, linked in the index only (no `submodule add`).
        s.write("sub/f.txt", "1\n");
        s.git(&["-C", "sub", "init", "-q"]);
        s.git(&["-C", "sub", "add", "f.txt"]);
        s.git(&["-C", "sub", "-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "-qm", "s"]);
        let head = s.git(&["-C", "sub", "rev-parse", "HEAD"]);
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{},sub", head.trim())]);
        s.git(&["commit", "-qm", "sub"]);
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["counts"]["total"], 0);
        // What the watcher sees is the file inside; the change is the gitlink's.
        s.write("sub/f.txt", "2\n");
        s.repo.invalidate(Scope::Paths(vec!["sub/f.txt".into()]));
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["unstaged"][0]["path"], "sub", "the cached clean gitlink stayed");
    }

    // git tells a gitlink by the type bits of its mode, whatever the others
    // are (an index made by hand may set them).
    #[test]
    fn a_gitlink_whose_mode_has_other_bits_is_still_a_gitlink() {
        for mode in [0o160755u32, 0o360000] {
            let s = scratch();
            s.commit("a.txt", "1\n", "first");
            s.write("sub/f.txt", "1\n");
            s.git(&["-C", "sub", "init", "-q"]);
            s.git(&["-C", "sub", "add", "f.txt"]);
            s.git(&["-C", "sub", "-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "-qm", "s"]);
            let head = s.git(&["-C", "sub", "rev-parse", "HEAD"]);
            s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{},sub", head.trim())]);
            // The gitlink's mode, written over in the index file (git doesn't check the file's checksum as it reads).
            let index = std::path::Path::new(s.root()).join(".git/index");
            let mut bytes = std::fs::read(&index).unwrap();
            let at: Vec<usize> = bytes.windows(4).enumerate().filter(|(_, w)| *w == 0o160000u32.to_be_bytes()).map(|(i, _)| i).collect();
            assert_eq!(at.len(), 1, "one gitlink mode in the index");
            bytes[at[0]..at[0] + 4].copy_from_slice(&mode.to_be_bytes());
            std::fs::write(&index, bytes).unwrap();
            assert!(s.git(&["ls-files", "-s", "sub"]).starts_with(&format!("{mode:06o} ")), "git doesn't read the mode as written");
            assert!(s.git(&["status", "--porcelain=v2"]).lines().any(|l| l.split(' ').nth(2).is_some_and(|sub| sub.starts_with('S')) && l.ends_with(" sub")), "git doesn't take it for a submodule, so this test proves nothing");
            refresh(&s.repo).unwrap();
            assert_eq!(s.repo.status.lock().unwrap().gitlinks, Some(vec!["sub".to_string()]), "mode {mode:o}");
        }
    }

    #[test]
    fn a_gitlink_list_cut_short_isnt_taken_as_whole() {
        let s = scratch();
        for i in 0..10 {
            s.write(&format!("f{i}.txt"), "1\n");
        }
        // A repo inside, linked in the index only, listed after the files.
        s.write("zz/sub/f.txt", "1\n");
        s.git(&["-C", "zz/sub", "init", "-q"]);
        s.git(&["-C", "zz/sub", "add", "f.txt"]);
        s.git(&["-C", "zz/sub", "-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "-qm", "s"]);
        let head = s.git(&["-C", "zz/sub", "rev-parse", "HEAD"]);
        s.git(&["add", "."]);
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{},zz/sub", head.trim())]);
        s.git(&["commit", "-qm", "files and sub"]);
        // The index listing (about 60 bytes an entry) is cut before `zz/sub`;
        // status itself still fits.
        CAP.with(|c| c.set(300));
        refresh(&s.repo).unwrap();
        assert_eq!(snapshot(&s.repo)["counts"]["total"], 0);
        s.write("zz/sub/f.txt", "2\n");
        s.repo.invalidate(Scope::Paths(vec!["zz/sub/f.txt".into()]));
        refresh(&s.repo).unwrap();
        CAP.with(|c| c.set(OUTPUT_CAP));
        assert_eq!(snapshot(&s.repo)["unstaged"][0]["path"], "zz/sub", "a gitlink past the cut was never asked about");
    }

    #[test]
    fn many_paths_in_their_own_folders_fold_without_comparing_every_pair() {
        // One path per folder, none crowded: nothing folds, every path is checked.
        let paths: Vec<String> = (0..40_000).map(|i| format!("d{i}/f.txt")).collect();
        let mut c = StatusCache { loaded: true, ..Default::default() };
        let started = std::time::Instant::now();
        c.dirty(Scope::Paths(paths.clone()));
        c.dirty(Scope::Paths(vec!["d7".into()]));
        let took = started.elapsed();
        match c.pending.clone().unwrap() {
            // `d7` covers `d7/f.txt`.
            Dirty::Paths(p) => assert_eq!((p.len(), p.contains("d7"), p.contains("d7/f.txt")), (40_000, true, false)),
            Dirty::All => panic!("folded too far"),
        }
        assert!(took < std::time::Duration::from_secs(2), "{took:?} for 40 000 paths");
    }

    // A partial clone fetches what it lacks from its remote as it reads:
    // `--branch` counts ahead/behind and would fetch the upstream commit. It
    // waits for trust from the start, and git run past the gate stays offline.
    #[test]
    fn an_untrusted_partial_clone_fetches_nothing_for_its_status() {
        let s = scratch();
        s.commit("a.txt", "1\n", "one");
        let two = s.commit("a.txt", "2\n", "two");
        let src = s.dir.path().join("src.git");
        let out = std::process::Command::new("git").args(["clone", "-q", "--bare", "--no-local", s.root()]).arg(&src).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        for (key, value) in [("remote.origin.url", format!("file://{}", src.display())), ("remote.origin.promisor", "true".into()), ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*".into()), ("branch.main.remote", "origin".into()), ("branch.main.merge", "refs/heads/main".into())] {
            s.git(&["config", key, &value]);
        }
        s.git(&["update-ref", "refs/remotes/origin/main", &two]);
        s.git(&["reset", "-q", "--hard", "HEAD~1"]);
        std::fs::remove_file(s.repo.git_path(&format!("objects/{}/{}", &two[..2], &two[2..]))).unwrap();
        let present = || s.repo.run(Git::read(["cat-file", "-e", &two]).env("GIT_NO_LAZY_FETCH", "1")).unwrap().ok();
        assert!(!present());
        let v = read(&s.repo, true).unwrap();
        assert_eq!(v["trust"], serde_json::json!(["remote.origin.promisor"]), "{v}");
        assert!(!present(), "status fetched from the promisor remote before the folder was trusted");
        // Past the gate, as a git before 2.44 (Ubuntu 22.04 has 2.34) that
        // ignores GIT_NO_LAZY_FETCH: the empty GIT_ALLOW_PROTOCOL stops it alone.
        s.repo.run(status_cmd(&s.repo, OUTPUT_CAP).env("GIT_NO_LAZY_FETCH", "0")).unwrap();
        assert!(!present(), "a git that ignores GIT_NO_LAZY_FETCH fetched");
        crate::trust::trust(&s.repo).unwrap();
        let v = read(&s.repo, true).unwrap();
        assert!(present(), "status didn't fetch once trusted, so this test proves nothing: {v}");
    }

    #[test]
    fn forgetting_many_sparse_paths_looks_each_entry_up_once() {
        let mut c = StatusCache::default();
        for i in 0..100_000 {
            let p = format!("d{}/f{i}", i % 1000);
            c.tracked.insert(p.clone(), Entry { path: p.clone(), orig: None, x: '.', y: 'M', submodule: None, conflict: false });
            c.staged_stats.insert(p.clone(), Stat::default());
            c.unstaged_stats.insert(p, Stat::default());
        }
        // 5 000 files, none crowding a folder, and the folder `d7`.
        let mut paths: Vec<String> = (0..5000).map(|i| format!("d{}/f{i}", i % 1000)).collect();
        paths.push("d7".into());
        let started = std::time::Instant::now();
        forget(&mut c, &paths);
        let took = started.elapsed();
        // `d7` holds 100 of them, 5 of which are listed as files too.
        let left = 100_000 - 5000 - 100 + 5;
        assert_eq!((c.tracked.len(), c.staged_stats.len(), c.unstaged_stats.len()), (left, left, left));
        assert!(!c.tracked.contains_key("d7/f5007") && c.tracked.contains_key("d8/f5008"));
        assert!(took < std::time::Duration::from_secs(2), "{took:?} for 100 000 entries");
    }

    #[test]
    fn a_listing_past_the_cap_is_an_error_not_a_short_list() {
        let s = scratch();
        for i in 0..5 {
            s.commit(&format!("f{i}.txt"), "1\n", "add");
        }
        CAP.with(|c| c.set(200));
        // About 110 bytes a record: five don't fit.
        for i in 0..5 {
            s.write(&format!("f{i}.txt"), "2\n");
        }
        let e = refresh(&s.repo).unwrap_err();
        assert!(e.to_string().contains("more than"), "{e}");
        assert!(!s.repo.status.lock().unwrap().loaded, "nothing short is kept");
        // Only the dirty paths are read again: the same holds there.
        CAP.with(|c| c.set(OUTPUT_CAP));
        refresh(&s.repo).unwrap();
        CAP.with(|c| c.set(200));
        s.write("f0.txt", "3\n");
        s.repo.status.lock().unwrap().dirty(Scope::Paths((0..5).map(|i| format!("f{i}.txt")).collect()));
        assert!(refresh(&s.repo).unwrap_err().to_string().contains("more than"));
        // Untracked files are listed by another command, with the same cap.
        CAP.with(|c| c.set(OUTPUT_CAP));
        s.git(&["checkout", "-q", "--", "."]);
        refresh(&s.repo).unwrap();
        CAP.with(|c| c.set(200));
        for i in 0..5 {
            s.write(&format!("an untracked file with a long name, number {i}.txt"), "x\n");
        }
        s.repo.status.lock().unwrap().dirty(Scope::All);
        assert!(refresh(&s.repo).unwrap_err().to_string().contains("more than"));
        CAP.with(|c| c.set(OUTPUT_CAP));
    }

    #[test]
    fn names_that_arent_utf8_are_remembered_as_shown() {
        let out = b"# branch.head main\0\
1 .D N... 100644 100644 000000 aaa aaa caf\xe9.txt\0\
2 R. N... 100644 100644 100644 aaa aaa R100 new.txt\0d\xe9j\xe0/old.txt\0\
1 .M N... 100644 100644 100644 aaa aaa plain.txt\0";
        let p = parse_porcelain_v2(out);
        assert_eq!(p.entries.len(), 3);
        assert_eq!(p.lossy, vec!["caf\u{FFFD}.txt".to_owned(), "d\u{FFFD}j\u{FFFD}/old.txt".to_owned()]);
        assert_eq!(lossy_z(b"ok.txt\0b\xffd.txt\0").collect::<Vec<_>>(), vec!["b\u{FFFD}d.txt".to_owned()]);
    }
}
