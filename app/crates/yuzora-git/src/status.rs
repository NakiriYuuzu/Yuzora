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

use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    pub gitlinks: Vec<String>,
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
        let crowded: Vec<String> = per_dir.into_iter().filter(|(_, n)| *n > FOLD_CHILDREN).map(|(d, _)| d.to_owned()).collect();
        if crowded.is_empty() {
            // A directory already in the set covers its children.
            let dirs: Vec<String> = paths.iter().cloned().collect();
            paths.retain(|p| !dirs.iter().any(|d| d != p && covers(d, p)));
            return Dirty::Paths(paths);
        }
        for d in crowded {
            if d.is_empty() {
                return Dirty::All;
            }
            paths.retain(|p| parent(p) != d);
            paths.insert(d);
        }
    }
}

fn parent(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
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
}

pub fn parse_porcelain_v2(out: &[u8]) -> Porcelain {
    let mut p = Porcelain::default();
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(rec) = fields.next() {
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
                p.entries.push(Entry {
                    path,
                    orig,
                    x: xy.first().copied().unwrap_or('.'),
                    y: xy.get(1).copied().unwrap_or('.'),
                    submodule: sub.starts_with('S').then(|| sub.to_owned()),
                    conflict: kind == b'u',
                });
            }
            b'?' => p.untracked.push(rec[2..].to_owned()),
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
    let pending = crate::trust::pending(repo);
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

fn status_cmd() -> Git {
    Git::read(["status", "--porcelain=v2", "-z", "--branch", "--show-stash", "--untracked-files=no"]).env("GIT_LITERAL_PATHSPECS", "1")
}

/// Joins a scoped read; a panic there is an error here, not a second panic.
fn joined<T>(h: std::thread::ScopedJoinHandle<'_, GitResult<T>>) -> GitResult<T> {
    h.join().unwrap_or_else(|_| Err(GitError::Other("a status read panicked".into())))
}

fn full(repo: &Repo) -> GitResult<()> {
    // The reads don't depend on each other (and take no lock): side by side,
    // a full refresh costs its slowest command rather than their sum.
    let (st, untracked, ignored_dirs) = std::thread::scope(|s| {
        let untracked = s.spawn(|| -> GitResult<BTreeSet<String>> { Ok(split_z(&repo.run(Git::read(["ls-files", "--others", "--exclude-standard", "-z"]))?.into_result()?).collect()) });
        let ignored = s.spawn(|| -> GitResult<Vec<String>> {
            if repo.place.is_remote() {
                return Ok(Vec::new());
            }
            Ok(split_z(&repo.run(Git::read(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]))?.into_result().unwrap_or_default())
                .filter_map(|p| p.strip_suffix('/').map(str::to_owned))
                .collect())
        });
        let st = repo.run(status_cmd()).and_then(|o| o.into_result()).map(|o| parse_porcelain_v2(&o));
        (st, joined(untracked), joined(ignored))
    });
    let (st, untracked, ignored_dirs) = (st?, untracked?, ignored_dirs?);
    let changed = st.entries.len();
    let (staged, unstaged, skipped) = if changed > NUMSTAT_LIMIT {
        (HashMap::new(), HashMap::new(), true)
    } else {
        let (staged, unstaged) = std::thread::scope(|s| {
            let staged = s.spawn(|| numstat(repo, true, &[]));
            (joined(staged), numstat(repo, false, &[]))
        });
        (staged?, unstaged?, false)
    };
    let lines = count_untracked(repo, untracked.iter());
    let operation = crate::repo::operation(repo);
    let gitlinks = gitlinks(repo);
    let mut c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    c.gitlinks = gitlinks;
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

/// The submodule paths `.gitmodules` lists.
fn gitlinks(repo: &Repo) -> Vec<String> {
    let out = repo.run(Git::read(["config", "--file", ".gitmodules", "-z", "--get-regexp", r"^submodule\..*\.path$"])).ok().filter(|o| o.ok()).map(|o| o.stdout).unwrap_or_default();
    // `-z`: `key\nvalue\0` per entry.
    String::from_utf8_lossy(&out).split('\0').filter_map(|e| e.split_once('\n')).map(|(_, v)| v.trim_end_matches('/').to_owned()).filter(|v| !v.is_empty()).collect()
}

fn partial(repo: &Repo, mut paths: Vec<String>) -> GitResult<()> {
    {
        let c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
        // git reports a submodule's own files as its gitlink, so ask for that.
        for p in &mut paths {
            if let Some(g) = c.gitlinks.iter().find(|g| p.starts_with(g.as_str()) && p.as_bytes().get(g.len()) == Some(&b'/')) {
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
    let (cmd, kept) = status_cmd().paths_or_all(&paths);
    if !kept {
        return full(repo);
    }
    let st = parse_porcelain_v2(&repo.run(cmd)?.into_result()?);
    let (ls, _) = Git::read(["ls-files", "--others", "--exclude-standard", "-z"]).env("GIT_LITERAL_PATHSPECS", "1").paths_or_all(&paths);
    let untracked: Vec<String> = split_z(&repo.run(ls)?.into_result()?).collect();
    let staged = numstat(repo, true, &paths)?;
    let unstaged = numstat(repo, false, &paths)?;
    let lines = count_untracked(repo, untracked.iter());
    let operation = crate::repo::operation(repo);
    let mut c = repo.status.lock().unwrap_or_else(|e| e.into_inner());
    let inside = |p: &str| paths.iter().any(|s| covers(s, p));
    c.tracked.retain(|p, _| !inside(p));
    c.untracked.retain(|p| !inside(p));
    c.staged_stats.retain(|p, _| !inside(p));
    c.unstaged_stats.retain(|p, _| !inside(p));
    c.untracked_lines.retain(|p, _| !inside(p));
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

fn numstat(repo: &Repo, cached: bool, paths: &[String]) -> GitResult<HashMap<String, Stat>> {
    let mut cmd = Git::read(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--no-textconv"]).env("GIT_LITERAL_PATHSPECS", "1");
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
            let meta = std::fs::metadata(&abs).ok()?;
            if !meta.is_file() || meta.len() > COUNT_LINES_MAX {
                return None;
            }
            let bytes = std::fs::read(&abs).ok()?;
            if bytes[..bytes.len().min(8000)].contains(&0) {
                return None;
            }
            let n = bytes.iter().filter(|b| **b == b'\n').count() + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
            Some((p.clone(), n as u32))
        })
        .collect()
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
        assert_eq!(s.repo.status.lock().unwrap().gitlinks, vec!["sub".to_string()]);
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
}
