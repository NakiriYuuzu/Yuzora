//! A file's diff as the diff pane shows it, and partial staging from it.
//!
//! Both sides are read whole (`git cat-file --filters <rev>:<path>`, the
//! work tree from disk) and compared here ([`crate::diff`]). A session keeps
//! the texts, so staging a hunk or some of its lines builds the new index
//! content from exactly what was shown, without touching the work tree
//! (IntelliJ's partial commit, B8): `git hash-object -w --stdin --path=<p>`
//! then `git update-index --cacheinfo`. Before writing, the index entry (or
//! the work tree file, for a discard) must still be what was read.
//!
//! Each changed block is a hunk: the unit IntelliJ stages and rolls back.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};

use crate::diff::{self, Change, Whitespace};
use crate::exec::{Git, GitError, GitResult};
use crate::lru::Lru;
use crate::repo::{PathKind, Repo};
use crate::status::Scope;

/// Bigger sides aren't diffed (shown as "too large").
pub const MAX_SIDE: usize = 16 << 20;
/// Nor sides of more lines: each costs the diff far more than its bytes.
pub const MAX_LINES: usize = 1_000_000;

/// Whether a side is past what is diffed here ([`MAX_SIDE`], [`MAX_LINES`]).
pub fn is_too_large(side: &[u8]) -> bool {
    side.len() > MAX_SIDE || side.iter().filter(|b| **b == b'\n').count() > MAX_LINES
}
/// Word diff inside blocks up to this size.
const WORD_DIFF_LINES: usize = 300;
const WORD_DIFF_CHARS: usize = 40_000;
const TAB: &str = "    ";

/// Which two versions are compared.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// Index → work tree.
    Unstaged,
    /// HEAD → index.
    Staged,
    /// A commit's first parent → the commit.
    Commit(String),
    /// Two revisions; `None` on the right is the work tree.
    Compare(String, Option<String>),
}

impl Mode {
    pub fn parse(v: &Value) -> GitResult<Mode> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Ok(match s("mode").as_deref().unwrap_or("unstaged") {
            "unstaged" | "untracked" => Mode::Unstaged,
            "staged" => Mode::Staged,
            "commit" => Mode::Commit(s("rev").ok_or_else(|| GitError::other("a commit diff needs rev"))?),
            "compare" => Mode::Compare(s("rev").ok_or_else(|| GitError::other("a compare needs rev"))?, s("to")),
            m => return Err(GitError::Other(format!("unknown diff mode {m}"))),
        })
    }

    fn name(&self) -> &'static str {
        match self {
            Mode::Unstaged => "unstaged",
            Mode::Staged => "staged",
            Mode::Commit(_) => "commit",
            Mode::Compare(..) => "compare",
        }
    }
}

pub struct DiffSession {
    pub id: u64,
    pub repo: Arc<Repo>,
    pub path: String,
    pub orig: Option<String>,
    pub mode: Mode,
    pub left: Option<Vec<u8>>,
    pub right: Option<Vec<u8>>,
    /// The index entry (mode, oid) when this was read: staging checks it.
    pub index_entry: Option<(String, String)>,
    pub binary: bool,
    pub too_large: bool,
    /// The modes on each side when they differ (a file made a program, or
    /// one no longer): unstaged and staged diffs, as git sees them.
    pub modes: Option<(String, String)>,
    /// Exact changes (what staging uses).
    pub changes: Vec<Change>,
}

/// A hunk header's `start,count`: git leaves the count out for one line, and for none the start
/// is the line before.
fn range(start: isize, count: usize) -> String {
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        n => format!("{},{n}", start + 1),
    }
}

/// A file's name in a patch, `prefix` before it: quoted the way git does when it holds a quote, a
/// backslash or a control character, as it is otherwise (spaces and wide characters included).
fn patch_name(prefix: &str, name: &str) -> String {
    if name.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        let mut quoted = String::from("\"");
        for c in format!("{prefix}{name}").chars() {
            match c {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                '\n' => quoted.push_str("\\n"),
                '\t' => quoted.push_str("\\t"),
                c if c.is_control() => quoted.push_str(&format!("\\{:03o}", c as u32)),
                c => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    } else {
        format!("{prefix}{name}")
    }
}

/// Joins a scoped read; a panic there is an error here, not a second panic.
fn joined<T>(h: std::thread::ScopedJoinHandle<'_, GitResult<T>>) -> GitResult<T> {
    h.join().unwrap_or_else(|_| Err(GitError::Other("a diff read panicked".into())))
}

/// `git cat-file --filters <spec>`, or None when it doesn't exist there.
pub fn blob(repo: &Repo, spec: &str) -> GitResult<Option<Vec<u8>>> {
    let out = repo.run(Git::read(["cat-file", "--filters", spec]).max_output(MAX_SIDE + 1))?;
    Ok(out.ok().then_some(out.stdout))
}

/// The index's text of `path` as `oid` names it (the entry [`index_entry`] read), with
/// the work tree's conversions for that path, as `cat-file --filters :path` gives.
fn blob_by_id(repo: &Repo, path: &str, mode: &str, oid: &str) -> GitResult<Option<Vec<u8>>> {
    // A link's text is its target, as it is: `:path` knows the entry is a link and converts nothing;
    // an id alone doesn't say, so (as `write_index` does) no conversion is asked for.
    let cmd = if mode == "120000" { Git::read(["cat-file", "blob"]).arg(oid) } else { Git::read(["cat-file", "--filters"]).arg(format!("--path={path}")).arg(oid) };
    let out = repo.run(cmd.max_output(MAX_SIDE + 1))?;
    Ok(out.ok().then_some(out.stdout))
}

/// The modes on each side of an unstaged (index → work tree) or staged
/// (HEAD → index) diff, when git sees them differ (`core.fileMode` too).
fn changed_modes(repo: &Repo, path: &str, staged: bool) -> GitResult<Option<(String, String)>> {
    let mut cmd = Git::read(["diff", "--raw", "-z", "--no-renames"]);
    if staged {
        cmd = cmd.arg("--cached");
    }
    let out = repo.run(cmd.arg("--").arg(path))?.text()?;
    // `:<old mode> <new mode> <oid> <oid> <status>\0<path>\0`
    let meta = out.split('\0').next().unwrap_or_default();
    let f: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
    Ok(match f.as_slice() {
        [a, b, ..] if a != b && *a != "000000" && *b != "000000" => Some((a.to_string(), b.to_string())),
        _ => None,
    })
}

/// The stage-0 index entry of a path: (mode, oid).
pub fn index_entry(repo: &Repo, path: &str) -> GitResult<Option<(String, String)>> {
    // `-z`: names with quotes, tabs or newlines come unquoted.
    let out = repo.run(Git::read(["ls-files", "-s", "-z", "--", path]))?.text()?;
    for line in out.split('\0') {
        // <mode> <oid> <stage>\t<path>
        let (meta, p) = line.split_once('\t').unwrap_or((line, ""));
        let f: Vec<&str> = meta.split(' ').collect();
        if p == path && f.len() == 3 && f[2] == "0" {
            return Ok(Some((f[0].to_owned(), f[1].to_owned())));
        }
    }
    Ok(None)
}

pub(crate) fn worktree(repo: &Repo, path: &str) -> Option<Vec<u8>> {
    let abs = repo.abs(path);
    match repo.kind(&abs) {
        // A link's content in git is its target (on a machine too: `cat` reads through it).
        Ok(PathKind::Link(target)) => Some(target),
        // One byte past the limit is enough to call it too large.
        _ => repo.read_file_head(&abs, MAX_SIDE + 1),
    }
}

fn is_link(repo: &Repo, path: &str, index_mode: Option<&str>) -> bool {
    index_mode == Some("120000") || matches!(repo.kind(&repo.abs(path)), Ok(PathKind::Link(_)))
}

impl DiffSession {
    pub fn load(repo: Arc<Repo>, path: String, orig: Option<String>, mode: Mode) -> GitResult<DiffSession> {
        let before = orig.clone().unwrap_or_else(|| path.clone());
        // yuzora(perf): the reads that don't depend on each other go out together: on a
        // machine each is a round trip, here each a process. They are judged in the order
        // they used to run in (a failed index look first), so the same one fails.
        // The index side is the exception: it is read by the object id the index entry
        // named, so the text and the id staging later checks are one snapshot (a
        // `:path` read next to the entry could meet another client's `git add` between).
        let (index_entry, left, right, modes) = std::thread::scope(|s| {
            let repo = &*repo;
            let path = path.as_str();
            let entry = crate::exec::spawn_scoped(s, move || index_entry(repo, path));
            let blob_of = |spec: String| Some(crate::exec::spawn_scoped(s, move || blob(repo, &spec)));
            let worktree_of = || Some(crate::exec::spawn_scoped(s, move || Ok(worktree(repo, path))));
            let (mut left, mut right) = match &mode {
                Mode::Unstaged => (None, worktree_of()),
                Mode::Staged => (blob_of(format!("HEAD:{before}")), None),
                Mode::Commit(rev) => (blob_of(format!("{rev}^1:{before}")), blob_of(format!("{rev}:{path}"))),
                Mode::Compare(a, b) => (blob_of(format!("{a}:{before}")), match b {
                    Some(b) => blob_of(format!("{b}:{path}")),
                    None => worktree_of(),
                }),
            };
            enum Mine {
                Modes(bool),
                HeadMode(String),
            }
            let want = match (&mode, &orig) {
                (Mode::Unstaged, _) => Some(Mine::Modes(false)),
                // A rename: HEAD's mode of the old path against the index's of the new.
                (Mode::Staged, Some(o)) => Some(Mine::HeadMode(o.clone())),
                (Mode::Staged, None) => Some(Mine::Modes(true)),
                _ => None,
            };
            let modes = want.map(|w| match w {
                Mine::Modes(staged) => crate::exec::spawn_scoped(s, move || changed_modes(repo, path, staged)),
                Mine::HeadMode(o) => crate::exec::spawn_scoped(s, move || head_mode(repo, &o).map(|m| m.map(|m| (m, String::new())))),
            });
            let entry = joined(entry);
            // Nothing in the index (or no look at it): there is no index side to read. A commit
            // or a comparison has none either: nothing is started for it.
            if matches!(mode, Mode::Unstaged | Mode::Staged)
                && let Ok(Some((entry_mode, oid))) = &entry
            {
                let (entry_mode, oid) = (entry_mode.clone(), oid.clone());
                let from_index = Some(crate::exec::spawn_scoped(s, move || blob_by_id(repo, path, &entry_mode, &oid)));
                if matches!(mode, Mode::Unstaged) {
                    left = from_index;
                } else {
                    right = from_index;
                }
            }
            let side = |h: Option<std::thread::ScopedJoinHandle<'_, GitResult<Option<Vec<u8>>>>>| h.map_or(Ok(None), joined);
            (entry, side(left), side(right), modes.map(joined))
        });
        let index_entry = index_entry?;
        let (left, right) = (left?, right?);
        let modes = match (&mode, &orig, modes) {
            (_, _, None) => None,
            (Mode::Staged, Some(_), Some(m)) => m?.map(|(head, _)| head).zip(index_entry.as_ref().map(|e| e.0.clone())).filter(|(a, b)| a != b),
            (_, _, Some(m)) => m?,
        };
        let too_large = [&left, &right].iter().any(|s| s.as_deref().is_some_and(is_too_large));
        let binary = !too_large && [&left, &right].iter().any(|s| s.as_ref().is_some_and(|b| b[..b.len().min(8000)].contains(&0)));
        let changes = if too_large || binary {
            Vec::new()
        } else {
            let (l, r) = (left.as_deref().unwrap_or_default(), right.as_deref().unwrap_or_default());
            diff::diff_lines(&diff::lines(l), &diff::lines(r), Whitespace::Exact)
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(DiffSession { id: NEXT.fetch_add(1, Ordering::SeqCst), repo, path, orig, mode, left, right, index_entry, binary, too_large, modes, changes })
    }

    /// The view: hunks and rows (`side` or `unified`), `context` lines around
    /// changes (-1: all), whitespace policy.
    pub fn view(&self, layout: &str, context: i64, ws: Whitespace) -> Value {
        let l = self.left.as_deref().unwrap_or_default();
        let r = self.right.as_deref().unwrap_or_default();
        // Not shown: no lines either (a large side would split into many).
        let shown = !(self.binary || self.too_large);
        let (ll, rl) = if shown { (diff::lines(l), diff::lines(r)) } else { (Vec::new(), Vec::new()) };
        let changes = if ws == Whitespace::Exact { self.changes.clone() } else if shown { diff::diff_lines(&ll, &rl, ws) } else { Vec::new() };
        let add: usize = changes.iter().map(|c| c.b.len()).sum();
        let del: usize = changes.iter().map(|c| c.a.len()).sum();
        let hunks: Vec<Value> = changes.iter().enumerate().map(|(i, c)| json!({ "index": i, "a": [c.a.start, c.a.end], "b": [c.b.start, c.b.end] })).collect();
        let rows = if shown { rows(&ll, &rl, &changes, layout == "unified", context, ws) } else { Vec::new() };
        json!({
            "id": self.id,
            "path": self.path,
            "orig": self.orig,
            "mode": self.mode.name(),
            "binary": self.binary,
            "tooLarge": self.too_large,
            "leftExists": self.left.is_some(),
            "rightExists": self.right.is_some(),
            "identical": changes.is_empty() && self.left == self.right && self.modes.is_none(),
            "modes": self.modes.as_ref().map(|(a, b)| [a, b]),
            "add": add,
            "del": del,
            "hunks": hunks,
            "rows": rows,
            "stageable": ws == Whitespace::Exact && matches!(self.mode, Mode::Unstaged | Mode::Staged),
            "leftLines": ll.len(),
            "rightLines": rl.len(),
        })
    }
}

/// Display text: UTF-8 (lossy), no line ending, tabs as spaces.
fn display(line: &[u8]) -> String {
    String::from_utf8_lossy(diff::strip_eol(line)).replace('\t', TAB)
}

/// Word-level segments for one block: per line, `[{t, h}]`.
fn word_segments(left: &[String], right: &[String], ws: Whitespace) -> Option<(Vec<Vec<Value>>, Vec<Vec<Value>>)> {
    if left.is_empty() || right.is_empty() || left.len() > WORD_DIFF_LINES || right.len() > WORD_DIFF_LINES {
        return None;
    }
    let a = left.join("\n");
    let b = right.join("\n");
    if a.len() + b.len() > WORD_DIFF_CHARS {
        return None;
    }
    let (ra, rb) = diff::diff_words(&a, &b, ws);
    Some((split_segments(left, &ra), split_segments(right, &rb)))
}

/// Cuts highlight ranges of the joined block back into per-line segments.
fn split_segments(lines: &[String], ranges: &[std::ops::Range<usize>]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in lines {
        let (start, end) = (offset, offset + line.len());
        let mut segs = Vec::new();
        let mut pos = start;
        for r in ranges {
            let (s, e) = (r.start.max(start), r.end.min(end));
            if s >= e {
                continue;
            }
            if s > pos {
                segs.push(json!({ "t": &line[pos - start..s - start], "h": false }));
            }
            segs.push(json!({ "t": &line[s - start..e - start], "h": true }));
            pos = e;
        }
        if pos < end {
            segs.push(json!({ "t": &line[pos - start..], "h": false }));
        }
        out.push(segs);
        offset = end + 1;
    }
    out
}

/// The rows the pane draws.
fn rows(ll: &[&[u8]], rl: &[&[u8]], changes: &[Change], unified: bool, context: i64, ws: Whitespace) -> Vec<Value> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let equal = |out: &mut Vec<Value>, a: usize, b: usize, n: usize, first: bool, last: bool| {
        // An unchanged run: shown whole, or folded down to `context` lines
        // next to the changes around it.
        let ctx = if context < 0 { usize::MAX } else { context as usize };
        let head = if first { 0 } else { ctx.min(n) };
        let tail = if last { 0 } else { ctx.min(n - head) };
        let fold = n.saturating_sub(head + tail);
        let push = |out: &mut Vec<Value>, k: usize| {
            let (lt, rt) = (display(ll[a + k]), display(rl[b + k]));
            let row = if lt == rt { json!({ "k": "=", "l": a + k + 1, "r": b + k + 1, "t": lt }) } else { json!({ "k": "=", "l": a + k + 1, "r": b + k + 1, "t": lt, "rt": rt }) };
            out.push(row);
        };
        if ctx == usize::MAX || fold <= 1 {
            for k in 0..n {
                push(out, k);
            }
            return;
        }
        for k in 0..head {
            push(out, k);
        }
        out.push(json!({ "k": "fold", "n": fold, "l": a + head + 1, "r": b + head + 1 }));
        for k in n - tail..n {
            push(out, k);
        }
    };
    let total = changes.len();
    for (h, c) in changes.iter().enumerate() {
        equal(&mut out, i, j, c.a.start - i, h == 0, false);
        let left: Vec<String> = ll[c.a.clone()].iter().map(|l| display(l)).collect();
        let right: Vec<String> = rl[c.b.clone()].iter().map(|l| display(l)).collect();
        let segs = word_segments(&left, &right, ws);
        if unified {
            for (k, t) in left.iter().enumerate() {
                out.push(json!({ "k": "-", "h": h, "l": c.a.start + k + 1, "li": c.a.start + k, "t": t, "s": segs.as_ref().map(|s| &s.0[k]) }));
            }
            for (k, t) in right.iter().enumerate() {
                out.push(json!({ "k": "+", "h": h, "r": c.b.start + k + 1, "ri": c.b.start + k, "t": t, "s": segs.as_ref().map(|s| &s.1[k]) }));
            }
        } else {
            for k in 0..left.len().max(right.len()) {
                let mut row = json!({ "k": "~", "h": h });
                if k < left.len() {
                    row["l"] = json!(c.a.start + k + 1);
                    row["li"] = json!(c.a.start + k);
                    row["lt"] = json!(left[k]);
                    row["ls"] = json!(segs.as_ref().map(|s| &s.0[k]));
                }
                if k < right.len() {
                    row["r"] = json!(c.b.start + k + 1);
                    row["ri"] = json!(c.b.start + k);
                    row["rt"] = json!(right[k]);
                    row["rs"] = json!(segs.as_ref().map(|s| &s.1[k]));
                }
                out.push(row);
            }
        }
        i = c.a.end;
        j = c.b.end;
    }
    equal(&mut out, i, j, ll.len() - i, total == 0, true);
    out
}

/// Which parts of which hunks: all of a hunk, or some of its lines (left
/// indexes are deletions, right indexes additions, both absolute).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub hunks: HashMap<usize, Option<(HashSet<usize>, HashSet<usize>)>>,
}

impl Selection {
    pub fn from_json(hunks: &Value, lines: &Value) -> Selection {
        let mut s = Selection::default();
        if let Some(a) = hunks.as_array() {
            for h in a.iter().filter_map(Value::as_u64) {
                s.hunks.insert(h as usize, None);
            }
        }
        if let Some(o) = lines.as_object() {
            for (k, v) in o {
                let Ok(h) = k.parse::<usize>() else { continue };
                let set = |key: &str| v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default();
                s.hunks.insert(h, Some((set("del"), set("add"))));
            }
        }
        s
    }

    /// Every hunk, whole or each of its lines picked one by one.
    fn covers_all(&self, changes: &[Change]) -> bool {
        !changes.is_empty()
            && changes.iter().enumerate().all(|(h, c)| match self.hunks.get(&h) {
                Some(None) => true,
                Some(Some((del, add))) => c.a.clone().all(|k| del.contains(&k)) && c.b.clone().all(|k| add.contains(&k)),
                None => false,
            })
    }
}

/// Apply (stage): the result starts from the left and takes the selected
/// changes. Revert (unstage, discard): it starts from the right and undoes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Apply,
    Revert,
}

/// Builds the new text from both sides and a selection.
pub fn combine(left: &[&[u8]], right: &[&[u8]], changes: &[Change], sel: &Selection, pick: Pick) -> Vec<u8> {
    let crlf = left.iter().chain(right).any(|l| l.ends_with(b"\r\n"));
    let mut out: Vec<u8> = Vec::new();
    let mut push = |line: &[u8]| {
        if !out.is_empty() && !out.ends_with(b"\n") {
            out.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
        }
        out.extend_from_slice(line);
    };
    let (mut i, mut j) = (0, 0);
    for (h, c) in changes.iter().enumerate() {
        match pick {
            Pick::Apply => left[i..c.a.start].iter().for_each(|l| push(l)),
            Pick::Revert => right[j..c.b.start].iter().for_each(|l| push(l)),
        }
        match (pick, sel.hunks.get(&h)) {
            (Pick::Apply, None) => left[c.a.clone()].iter().for_each(|l| push(l)),
            (Pick::Apply, Some(None)) => right[c.b.clone()].iter().for_each(|l| push(l)),
            (Pick::Apply, Some(Some((del, add)))) => {
                c.a.clone().filter(|k| !del.contains(k)).for_each(|k| push(left[k]));
                c.b.clone().filter(|k| add.contains(k)).for_each(|k| push(right[k]));
            }
            (Pick::Revert, None) => right[c.b.clone()].iter().for_each(|l| push(l)),
            (Pick::Revert, Some(None)) => left[c.a.clone()].iter().for_each(|l| push(l)),
            (Pick::Revert, Some(Some((del, add)))) => {
                c.a.clone().filter(|k| del.contains(k)).for_each(|k| push(left[k]));
                c.b.clone().filter(|k| !add.contains(k)).for_each(|k| push(right[k]));
            }
        }
        i = c.a.end;
        j = c.b.end;
    }
    match pick {
        Pick::Apply => left[i..].iter().for_each(|l| push(l)),
        Pick::Revert => right[j..].iter().for_each(|l| push(l)),
    }
    out
}

/// Writes `content` as the path's index entry.
pub(crate) fn write_index(repo: &Repo, path: &str, content: &[u8], mode: &str) -> GitResult<()> {
    // A link's target is stored as is; files go through their filters.
    let filters = if mode == "120000" { "--no-filters".to_owned() } else { format!("--path={path}") };
    let oid = repo.text(Git::write(["hash-object", "-w", "--stdin", &filters]).stdin(content.to_vec()))?;
    repo.run(Git::write(["update-index", "--add", "--cacheinfo", &format!("{mode},{},{path}", oid.trim())]))?.into_result()?;
    Ok(())
}

/// The mode of a path in `HEAD`, if it is there.
fn head_mode(repo: &Repo, path: &str) -> GitResult<Option<String>> {
    let out = repo.run(Git::read(["ls-tree", "-z", "--full-tree", "HEAD", "--", path]))?;
    if !out.ok() {
        return Ok(None);
    }
    let text = out.text()?;
    // <mode> <type> <oid>\t<path>
    Ok(text.split('\0').find_map(|e| e.split_once('\t').filter(|(_, p)| *p == path).and_then(|(meta, _)| meta.split(' ').next()).map(str::to_owned)))
}

fn new_file_mode(repo: &Repo, path: &str) -> GitResult<&'static str> {
    Ok(match repo.kind(&repo.abs(path))? {
        PathKind::Link(_) => "120000",
        PathKind::File { exec: true } => "100755",
        _ => "100644",
    })
}

/// Which side of a diff a run of lines is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Lines of context a patch keeps around a change, as `git diff` does.
const PATCH_CONTEXT: usize = 3;

impl DiffSession {
    fn sides(&self) -> (Vec<&[u8]>, Vec<&[u8]>) {
        (diff::lines(self.left.as_deref().unwrap_or_default()), diff::lines(self.right.as_deref().unwrap_or_default()))
    }

    /// The lines the pane's selection covers, as the file has them: each run is lines
    /// `start..end` (0-based) of one side, taken in the order given and joined by `\n`. A line
    /// keeps its tabs, spaces and characters, and loses only its line end; where a run runs past
    /// the side it stops at the side's last line. Nothing for a diff that shows no lines.
    pub fn lines_text(&self, runs: &[(Side, usize, usize)]) -> String {
        if self.binary || self.too_large {
            return String::new();
        }
        let (left, right) = self.sides();
        let mut out: Vec<String> = Vec::new();
        for &(side, start, end) in runs {
            let lines = if side == Side::Left { &left } else { &right };
            let end = end.min(lines.len());
            for line in lines.get(start..end).unwrap_or_default() {
                out.push(String::from_utf8_lossy(diff::strip_eol(line)).into_owned());
            }
        }
        out.join("\n")
    }

    /// The patch of some hunks of the view `ws` makes (their numbers there), as `git apply` takes it:
    /// from the left text to the right one, with only these hunks. Each keeps up to three lines of
    /// context that are the same on both sides, hunks near enough to touch share one `@@`, and the
    /// new side's numbers count only the hunks in the patch. Empty for no hunk.
    pub fn patch(&self, ws: Whitespace, hunks: &[usize]) -> String {
        if self.binary || self.too_large {
            return String::new();
        }
        let (left, right) = self.sides();
        let changes = if ws == Whitespace::Exact { self.changes.clone() } else { diff::diff_lines(&left, &right, ws) };
        let mut picked: Vec<usize> = hunks.iter().copied().filter(|h| *h < changes.len()).collect();
        picked.sort_unstable();
        picked.dedup();
        if picked.is_empty() {
            return String::new();
        }
        let mut out: Vec<u8> = self.patch_header().into_bytes();
        // What the earlier hunks of this patch add to the file's length.
        let mut grown: isize = 0;
        let mut at = 0;
        while at < picked.len() {
            // Hunks that are neighbours with at most two contexts between them make one `@@`.
            let (first, mut last) = (picked[at], picked[at]);
            at += 1;
            while at < picked.len() && picked[at] == last + 1 && changes[last + 1].a.start - changes[last].a.end <= 2 * PATCH_CONTEXT {
                last = picked[at];
                at += 1;
            }
            let before = if first == 0 { 0 } else { changes[first - 1].a.end };
            let after = changes.get(last + 1).map_or(left.len(), |c| c.a.start);
            let from = changes[first].a.start - PATCH_CONTEXT.min(changes[first].a.start - before);
            let to = changes[last].a.end + PATCH_CONTEXT.min(after - changes[last].a.end);

            let mut body: Vec<u8> = Vec::new();
            let mut line = |sign: u8, raw: &[u8]| {
                body.push(sign);
                body.extend_from_slice(raw);
                if !raw.ends_with(b"\n") {
                    body.extend_from_slice(b"\n\\ No newline at end of file\n");
                }
            };
            let (mut was, mut now) = (0usize, 0usize);
            let unchanged = |line: &mut dyn FnMut(u8, &[u8]), range: std::ops::Range<usize>, last_of_hunk: usize| {
                for i in range {
                    // The last line of a text with no line end can't be a context line when
                    // more of the hunk follows it: the added lines after it need it ended.
                    if !left[i].ends_with(b"\n") && i + 1 != last_of_hunk {
                        line(b'-', left[i]);
                        let mut ended = left[i].to_vec();
                        ended.push(b'\n');
                        line(b'+', &ended);
                    } else {
                        line(b' ', left[i]);
                    }
                }
            };
            let mut next = from;
            for h in first..=last {
                let c = &changes[h];
                unchanged(&mut line, next..c.a.start, usize::MAX);
                was += c.a.len();
                now += c.b.len();
                for l in &left[c.a.clone()] {
                    line(b'-', l);
                }
                for l in &right[c.b.clone()] {
                    line(b'+', l);
                }
                next = c.a.end;
            }
            unchanged(&mut line, next..to, to);
            let kept = to - from - was;
            let (was, now) = (kept + was, kept + now);
            let start = from as isize + grown;
            out.extend_from_slice(format!("@@ -{} +{} @@\n", range(from as isize, was), range(start, now)).as_bytes());
            out.extend_from_slice(&body);
            grown += now as isize - was as isize;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// `diff --git` down to the `+++` line: the file's names, and that it is new, gone or renamed.
    fn patch_header(&self) -> String {
        let old = self.orig.as_deref().unwrap_or(&self.path);
        let (a, b) = (patch_name("a/", old), patch_name("b/", &self.path));
        let mut head = format!("diff --git {a} {b}\n");
        if self.left.is_none() {
            head.push_str("new file mode 100644\n");
        } else if self.right.is_none() {
            head.push_str("deleted file mode 100644\n");
        } else if old != self.path {
            head.push_str(&format!("rename from {}\nrename to {}\n", patch_name("", old), patch_name("", &self.path)));
        }
        let from = if self.left.is_none() { "/dev/null".to_owned() } else { patch_name("a/", old) };
        let to = if self.right.is_none() { "/dev/null".to_owned() } else { patch_name("b/", &self.path) };
        head.push_str(&format!("--- {from}\n+++ {to}\n"));
        head
    }

    fn check_index(&self) -> GitResult<()> {
        if index_entry(&self.repo, &self.path)? != self.index_entry {
            return Err(GitError::other("the index changed since this diff was read; reload it"));
        }
        Ok(())
    }

    /// Stages the selection (an unstaged diff).
    pub fn stage(&self, sel: &Selection) -> GitResult<()> {
        if self.mode != Mode::Unstaged {
            return Err(GitError::other("only an unstaged diff can be staged"));
        }
        // `diffOpen` checked trust, but the repo's config may have gained a
        // program since (writes run filters).
        crate::trust::check(&self.repo)?;
        self.check_index()?;
        let p = vec![self.path.clone()];
        if sel.covers_all(&self.changes) || (self.changes.is_empty() && (self.left != self.right || self.modes.is_some())) {
            // Whole file: let git record deletions, modes and new files itself.
            self.repo.run(Git::write(["add", "-A"]).pathspec(&p))?.into_result()?;
        } else {
            // Built from the file as read: once edited elsewhere, it would
            // stage lines the file no longer has.
            if worktree(&self.repo, &self.path) != self.right {
                return Err(GitError::other("the file changed since this diff was read; reload it"));
            }
            let (l, r) = self.sides();
            let content = combine(&l, &r, &self.changes, sel, Pick::Apply);
            let mode = match &self.index_entry {
                Some(e) => e.0.clone(),
                None => new_file_mode(&self.repo, &self.path)?.to_owned(),
            };
            write_index(&self.repo, &self.path, &content, &mode)?;
        }
        self.repo.invalidate(Scope::Paths(p));
        Ok(())
    }

    /// Unstages the selection (a staged diff).
    pub fn unstage(&self, sel: &Selection) -> GitResult<()> {
        if self.mode != Mode::Staged {
            return Err(GitError::other("only a staged diff can be unstaged"));
        }
        crate::trust::check(&self.repo)?;
        self.check_index()?;
        let mut p = vec![self.path.clone()];
        p.extend(self.orig.clone());
        if sel.covers_all(&self.changes) || self.changes.is_empty() {
            unstage_paths(&self.repo, &p)?;
        } else {
            // The result is built from this HEAD: once another client
            // committed, it would stage a change nobody made.
            let before = self.orig.as_deref().unwrap_or(&self.path);
            if blob(&self.repo, &format!("HEAD:{before}"))? != self.left {
                return Err(GitError::other("HEAD moved since this diff was read; reload it"));
            }
            let (l, r) = self.sides();
            let content = combine(&l, &r, &self.changes, sel, Pick::Revert);
            // A staged deletion has no index entry: it comes back as HEAD had it.
            let mode = match &self.index_entry {
                Some(e) => e.0.clone(),
                None => head_mode(&self.repo, before)?.unwrap_or_else(|| "100644".into()),
            };
            write_index(&self.repo, &self.path, &content, &mode)?;
        }
        self.repo.invalidate(Scope::Paths(p));
        Ok(())
    }

    /// Throws away the selection from the work tree (an unstaged diff).
    pub fn discard(&self, sel: &Selection) -> GitResult<()> {
        if self.mode != Mode::Unstaged {
            return Err(GitError::other("only unstaged changes can be discarded"));
        }
        crate::trust::check(&self.repo)?;
        if worktree(&self.repo, &self.path) != self.right {
            return Err(GitError::other("the file changed since this diff was read; reload it"));
        }
        let abs = self.repo.abs(&self.path);
        // A change of mode alone goes back whole (git restores the mode).
        let all = sel.covers_all(&self.changes) || (self.changes.is_empty() && self.modes.is_some());
        let p = vec![self.path.clone()];
        if all && self.left.is_none() {
            crate::repo::remove_path(&self.repo.place, &abs)?;
        } else if all {
            // Back to the index: git restores the type, mode and filters.
            self.repo.run(Git::write(["restore", "--worktree"]).pathspec(&p))?.into_result()?;
        } else if is_link(&self.repo, &self.path, self.index_entry.as_ref().map(|e| e.0.as_str())) {
            return Err(GitError::other("a symbolic link changes as a whole: discard all of it"));
        } else {
            let (l, r) = self.sides();
            let content = combine(&l, &r, &self.changes, sel, Pick::Revert);
            if self.right.is_none() && self.left.is_some() {
                // A deleted file comes back as git had it (mode, type) first.
                self.repo.run(Git::write(["restore", "--worktree"]).pathspec(&p))?.into_result()?;
            }
            self.repo.write_file(&abs, &content)?;
        }
        self.repo.invalidate(Scope::Paths(vec![self.path.clone()]));
        Ok(())
    }
}

/// `git reset -- <paths>` (before the first commit: `git rm --cached`).
pub fn unstage_paths(repo: &Repo, paths: &[String]) -> GitResult<()> {
    let has_head = repo.run(Git::read(["rev-parse", "-q", "--verify", "HEAD"]))?.ok();
    let cmd = if has_head { Git::write(["reset", "-q"]) } else { Git::write(["rm", "--cached", "-r", "-q", "--ignore-unmatch"]) };
    repo.run(cmd.pathspec(paths))?.into_result()?;
    Ok(())
}

fn sessions() -> &'static Mutex<Lru<u64, Arc<DiffSession>>> {
    static S: OnceLock<Mutex<Lru<u64, Arc<DiffSession>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Lru::new(64)))
}

pub fn keep(s: DiffSession) -> Arc<DiffSession> {
    let s = Arc::new(s);
    sessions().lock().unwrap_or_else(|e| e.into_inner()).put(s.id, s.clone());
    s
}

pub fn get(id: u64) -> GitResult<Arc<DiffSession>> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(&id).ok_or_else(|| GitError::other("this diff is gone; reload it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;
    #[cfg(unix)]
    use crate::repo::testing::{most_at_once, timed_git};

    fn sel_all(h: &[usize]) -> Selection {
        Selection { hunks: h.iter().map(|h| (*h, None)).collect() }
    }

    #[test]
    fn combining_applies_or_reverts_what_was_picked() {
        let l = diff::lines(b"a\nb\nc\nd\ne\n");
        let r = diff::lines(b"a\nB\nc\nd\nE\nF\n");
        let cs = diff::diff_lines(&l, &r, Whitespace::Exact);
        assert_eq!(cs.len(), 2);
        assert_eq!(combine(&l, &r, &cs, &sel_all(&[0]), Pick::Apply), b"a\nB\nc\nd\ne\n");
        assert_eq!(combine(&l, &r, &cs, &sel_all(&[1]), Pick::Revert), b"a\nB\nc\nd\ne\n");
        // Lines: of the second hunk (e → E, F) take only the addition F.
        let mut s = Selection::default();
        s.hunks.insert(1, Some((HashSet::new(), HashSet::from([5]))));
        assert_eq!(combine(&l, &r, &cs, &s, Pick::Apply), b"a\nb\nc\nd\ne\nF\n");
        // A last line without a newline gets one when something follows it.
        let l2 = diff::lines(b"x");
        let r2 = diff::lines(b"x\ny");
        let c2 = diff::diff_lines(&l2, &r2, Whitespace::Exact);
        assert_eq!(combine(&l2, &r2, &c2, &sel_all(&[0]), Pick::Apply), b"x\ny");
        let crlf = diff::lines(b"1\r\n2\r\n");
        let crlf2 = diff::lines(b"1\r\n2\r\n3\r\n");
        let c3 = diff::diff_lines(&crlf, &crlf2, Whitespace::Exact);
        assert_eq!(combine(&crlf, &crlf2, &c3, &sel_all(&[0]), Pick::Apply), b"1\r\n2\r\n3\r\n");
    }

    #[test]
    fn rows_fold_unchanged_runs_and_carry_word_segments() {
        let a: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 10\n", "line ten\n");
        let (la, lb) = (diff::lines(a.as_bytes()), diff::lines(b.as_bytes()));
        let cs = diff::diff_lines(&la, &lb, Whitespace::Exact);
        let side = rows(&la, &lb, &cs, false, 3, Whitespace::Exact);
        let kinds: Vec<&str> = side.iter().map(|r| r["k"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["fold", "=", "=", "=", "~", "=", "=", "=", "fold"]);
        assert_eq!(side[0]["n"], 6);
        assert_eq!(side[4]["ls"], json!([{ "t": "line ", "h": false }, { "t": "10", "h": true }]));
        let all = rows(&la, &lb, &cs, true, -1, Whitespace::Exact);
        assert_eq!(all.len(), 21);
        assert_eq!(all[9]["k"], "-");
        assert_eq!(all[10]["k"], "+");
    }

    #[test]
    fn a_hunk_is_staged_without_touching_the_work_tree() {
        let s = scratch();
        s.commit("f.txt", "a\nb\nc\nd\ne\nf\ng\nh\n", "base");
        s.write("f.txt", "a\nB\nc\nd\ne\nf\ng\nH\n");
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Unstaged).unwrap();
        let v = d.view("side", 3, Whitespace::Exact);
        assert_eq!(v["hunks"].as_array().unwrap().len(), 2);
        assert_eq!(v["stageable"], true);
        d.stage(&sel_all(&[0])).unwrap();
        assert_eq!(s.git(&["show", ":f.txt"]), "a\nB\nc\nd\ne\nf\ng\nh\n");
        assert_eq!(s.read("f.txt"), "a\nB\nc\nd\ne\nf\ng\nH\n", "work tree untouched");
        // A stale session refuses.
        assert!(d.stage(&sel_all(&[1])).is_err());
        // Unstage it again from the staged diff.
        let st = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Staged).unwrap();
        st.unstage(&sel_all(&[0])).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "");
        // Discard the second hunk from the work tree.
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Unstaged).unwrap();
        d.discard(&sel_all(&[1])).unwrap();
        assert_eq!(s.read("f.txt"), "a\nB\nc\nd\ne\nf\ng\nh\n");
    }

    #[cfg(unix)]
    #[test]
    fn new_links_and_programs_on_a_machine_keep_their_type_when_partly_staged() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        s.commit("base.txt", "x\n", "base");
        let remote = crate::repo::testing::on_machine(&s);
        let root = std::path::Path::new(s.root());
        s.write("target.txt", "one\ntwo\nthree\n");
        std::os::unix::fs::symlink("target.txt", root.join("link")).unwrap();
        s.write("run.sh", "#!/bin/sh\necho one\necho two\n");
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let first = Selection::from_json(&json!([]), &json!({ "0": { "add": [0] } }));
        // A link's content in git is its target, there as here.
        let link = DiffSession::load(remote.clone(), "link".into(), None, Mode::Unstaged).unwrap();
        assert_eq!(link.right.as_deref(), Some(&b"target.txt"[..]));
        link.stage(&first).unwrap();
        // Part of a new program is staged as a program.
        let run = DiffSession::load(remote.clone(), "run.sh".into(), None, Mode::Unstaged).unwrap();
        run.stage(&first).unwrap();
        let staged = s.git(&["ls-files", "-s", "--", "link", "run.sh"]);
        assert!(staged.lines().any(|l| l.starts_with("120000 ") && l.ends_with("\tlink")), "{staged}");
        assert!(staged.lines().any(|l| l.starts_with("100755 ") && l.ends_with("\trun.sh")), "{staged}");
        assert_eq!(s.git(&["show", ":link"]), "target.txt");
    }

    #[test]
    fn an_open_diff_stops_once_its_folder_needs_trust() {
        let s = scratch();
        s.commit("f.txt", "a\nb\n", "base");
        s.commit("g.txt", "1\n", "g");
        s.write("f.txt", "a\nB\n");
        s.write("g.txt", "2\n");
        s.git(&["add", "--", "g.txt"]);
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Unstaged).unwrap();
        let st = DiffSession::load(s.repo.clone(), "g.txt".into(), None, Mode::Staged).unwrap();
        // The pane was open when the repo's config gained a program
        // (`git lfs install --local`, say): staging would run it.
        s.git(&["config", "filter.lfs.clean", "git-lfs clean -- %f"]);
        for r in [d.stage(&sel_all(&[0])), d.discard(&sel_all(&[0])), st.unstage(&sel_all(&[0]))] {
            assert!(r.as_ref().is_err_and(|e| e.to_string().starts_with(crate::trust::UNTRUSTED)), "{r:?}");
        }
        assert_eq!(s.read("f.txt"), "a\nB\n");
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "g.txt\n");
    }

    #[test]
    fn a_side_of_too_many_lines_is_too_large_to_compare() {
        let s = scratch();
        s.commit("big.txt", "a\n", "small");
        // Far under 16 MiB, but each line costs the diff more than its byte.
        s.write("big.txt", &"\n".repeat(MAX_LINES + 1));
        let d = DiffSession::load(s.repo.clone(), "big.txt".into(), None, Mode::Unstaged).unwrap();
        assert!(d.too_large && d.changes.is_empty(), "too large: {}", d.too_large);
        let v = d.view("side", 3, Whitespace::Exact);
        assert_eq!((v["tooLarge"].clone(), v["rows"].as_array().map(Vec::len)), (json!(true), Some(0)));
        // Whole, it still stages.
        d.stage(&sel_all(&[])).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]).trim(), "big.txt");
    }

    #[test]
    fn new_and_deleted_files_and_commits_diff_too() {
        let s = scratch();
        let c1 = s.commit("gone.txt", "bye\n", "add");
        s.write("new.txt", "one\ntwo\n");
        let d = DiffSession::load(s.repo.clone(), "new.txt".into(), None, Mode::Unstaged).unwrap();
        assert!(d.left.is_none());
        assert_eq!(d.view("side", 3, Whitespace::Exact)["add"], 2);
        d.stage(&sel_all(&[0])).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-status"]).trim(), "A\tnew.txt");
        let c = DiffSession::load(s.repo.clone(), "gone.txt".into(), None, Mode::Commit(c1)).unwrap();
        assert!(c.left.is_none() && c.right.is_some());
        std::fs::remove_file(format!("{}/gone.txt", s.root())).unwrap();
        let del = DiffSession::load(s.repo.clone(), "gone.txt".into(), None, Mode::Unstaged).unwrap();
        assert!(del.right.is_none());
        del.stage(&sel_all(&[0])).unwrap();
        assert!(s.git(&["diff", "--cached", "--name-status"]).contains("D\tgone.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn discarding_a_link_or_a_deleted_file_restores_what_git_had() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let s = scratch();
        s.write("a", "target a\n");
        s.write("b", "keep b\n");
        symlink("a", format!("{}/link", s.root())).unwrap();
        s.write("run.sh", "#!/bin/sh\necho 1\necho 2\n");
        std::fs::set_permissions(format!("{}/run.sh", s.root()), std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["add", "-A"]);
        s.git(&["commit", "-qm", "files"]);
        // The link now points at b: discarding puts the link back, b is untouched.
        std::fs::remove_file(format!("{}/link", s.root())).unwrap();
        symlink("b", format!("{}/link", s.root())).unwrap();
        let d = DiffSession::load(s.repo.clone(), "link".into(), None, Mode::Unstaged).unwrap();
        d.discard(&sel_all(&[0])).unwrap();
        assert_eq!(std::fs::read_link(format!("{}/link", s.root())).unwrap().to_string_lossy(), "a");
        assert_eq!(s.read("b"), "keep b\n", "the discard wrote through the link");
        // A deleted executable comes back executable.
        std::fs::remove_file(format!("{}/run.sh", s.root())).unwrap();
        let d = DiffSession::load(s.repo.clone(), "run.sh".into(), None, Mode::Unstaged).unwrap();
        d.discard(&sel_all(&[0])).unwrap();
        let m = std::fs::symlink_metadata(format!("{}/run.sh", s.root())).unwrap();
        assert!(m.is_file() && m.permissions().mode() & 0o111 != 0, "{m:?}");
        // Names git would quote are still found in the index.
        let odd = "say \"hi\"\tnow.txt";
        s.commit(odd, "1\n2\n", "odd name");
        assert!(index_entry(&s.repo, odd).unwrap().is_some());
        s.write(odd, "1\n");
        let d = DiffSession::load(s.repo.clone(), odd.into(), None, Mode::Unstaged).unwrap();
        d.discard(&sel_all(&[0])).unwrap();
        assert_eq!(s.read(odd), "1\n2\n", "restored, not deleted");
    }

    #[cfg(unix)]
    #[test]
    fn staging_some_lines_keeps_what_kind_of_file_it_was() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let s = scratch();
        s.write("run.sh", "#!/bin/sh\necho 1\necho 2\n");
        std::fs::set_permissions(format!("{}/run.sh", s.root()), std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["add", "-A"]);
        s.git(&["commit", "-qm", "files"]);
        let lines = |del: &[usize], add: &[usize]| Selection::from_json(&json!([]), &json!({ "0": { "del": del, "add": add } }));
        // Unstaging part of a staged deletion brings back an executable.
        s.git(&["rm", "-q", "run.sh"]);
        let st = DiffSession::load(s.repo.clone(), "run.sh".into(), None, Mode::Staged).unwrap();
        st.unstage(&lines(&[0], &[])).unwrap();
        let entry = s.git(&["ls-files", "-s", "run.sh"]);
        assert!(entry.starts_with("100755 "), "{entry}");
        // Part of a new link is still a link.
        symlink("a\nb", format!("{}/link", s.root())).unwrap();
        let d = DiffSession::load(s.repo.clone(), "link".into(), None, Mode::Unstaged).unwrap();
        d.stage(&lines(&[], &[0])).unwrap();
        let entry = s.git(&["ls-files", "-s", "link"]);
        assert!(entry.starts_with("120000 "), "{entry}");
    }

    #[cfg(unix)]
    #[test]
    fn part_of_a_new_link_keeps_target_bytes_that_arent_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        // `café` in Latin-1 on the first line of the target.
        std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(b"caf\xe9\nb"), format!("{}/link", s.root())).unwrap();
        let d = DiffSession::load(s.repo.clone(), "link".into(), None, Mode::Unstaged).unwrap();
        d.stage(&Selection::from_json(&json!([]), &json!({ "0": { "del": [], "add": [0] } }))).unwrap();
        let blob = std::process::Command::new("git").args(["cat-file", "blob", ":link"]).current_dir(s.root()).output().unwrap().stdout;
        assert!(blob.starts_with(b"caf\xe9"), "{:?}", String::from_utf8_lossy(&blob));
    }

    #[test]
    fn every_line_picked_one_by_one_is_the_whole_file() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.write("new.txt", "one\ntwo\n");
        s.git(&["add", "new.txt"]);
        // Every added line of a staged new file, picked as lines.
        let lines = Selection::from_json(&json!([]), &json!({ "0": { "del": [], "add": [0, 1] } }));
        let d = DiffSession::load(s.repo.clone(), "new.txt".into(), None, Mode::Staged).unwrap();
        d.unstage(&lines).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "", "an empty file stayed staged in its place");
        assert_eq!(s.read("new.txt"), "one\ntwo\n");
    }

    #[test]
    fn a_partial_stage_refuses_once_the_file_changed_under_it() {
        let s = scratch();
        s.commit("f.txt", "a\nb\nc\nd\ne\nf\ng\nh\n", "base");
        s.write("f.txt", "a\nB\nc\nd\ne\nf\ng\nH\n");
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Unstaged).unwrap();
        // Edited in another editor after the diff was read: its `B` is gone.
        s.write("f.txt", "a\nBee\nc\nd\ne\nf\ng\nH\n");
        assert!(d.stage(&sel_all(&[0])).is_err());
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "", "a stale stage wrote a line the file no longer has");
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_rename_that_changed_mode_shows_it() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        s.commit("old.sh", "echo\n", "first");
        s.git(&["mv", "old.sh", "new.sh"]);
        std::fs::set_permissions(s.repo.abs("new.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        s.git(&["add", "new.sh"]);
        let d = DiffSession::load(s.repo.clone(), "new.sh".into(), Some("old.sh".into()), Mode::Staged).unwrap();
        let v = d.view("side", 3, Whitespace::Exact);
        assert_eq!((v["identical"].clone(), v["modes"].clone()), (json!(false), json!(["100644", "100755"])), "{v}");
        d.unstage(&sel_all(&[])).unwrap();
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn a_partial_unstage_refuses_once_head_moved_under_it() {
        let s = scratch();
        s.commit("f.txt", "a\nb\nc\nd\ne\nf\ng\nh\n", "base");
        s.write("f.txt", "a\nB\nc\nd\ne\nf\ng\nH\n");
        s.git(&["add", "--", "f.txt"]);
        let st = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Staged).unwrap();
        // Another client commits what was staged: the index is the same, HEAD is not.
        s.git(&["commit", "-qm", "elsewhere"]);
        assert!(st.unstage(&sel_all(&[0])).is_err());
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "", "a stale unstage staged a change");
    }

    #[cfg(unix)]
    #[test]
    fn a_change_of_mode_alone_shows_and_stages_unstages_and_discards() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        s.commit("run.sh", "echo\n", "first");
        let index_mode = || s.git(&["ls-files", "-s", "--", "run.sh"]).split(' ').next().unwrap_or_default().to_owned();
        let abs = s.repo.abs("run.sh");
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o755)).unwrap();
        let open = |mode: Mode| DiffSession::load(s.repo.clone(), "run.sh".into(), None, mode).unwrap();
        let d = open(Mode::Unstaged);
        let v = d.view("side", 3, Whitespace::Exact);
        assert_eq!((v["identical"].clone(), v["modes"].clone()), (json!(false), json!(["100644", "100755"])), "{v}");
        d.stage(&sel_all(&[0])).unwrap();
        assert_eq!(index_mode(), "100755", "staging from the pane kept the old mode");
        let d = open(Mode::Staged);
        assert_eq!(d.view("side", 3, Whitespace::Exact)["modes"], json!(["100644", "100755"]));
        d.unstage(&sel_all(&[0])).unwrap();
        assert_eq!(index_mode(), "100644");
        open(Mode::Unstaged).discard(&sel_all(&[0])).unwrap();
        assert_eq!(std::fs::metadata(&abs).unwrap().permissions().mode() & 0o111, 0, "discarding left it a program");
        assert_eq!(s.git(&["status", "--porcelain"]), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_diff_reads_its_sides_and_modes_together() {
        let s = scratch();
        s.commit("f.txt", "a\nb\n", "base");
        s.write("f.txt", "a\nB\n");
        // unstaged: the index look, the left side and the modes
        let (repo, log) = timed_git(&s, None);
        let d = DiffSession::load(repo, "f.txt".into(), None, Mode::Unstaged).unwrap();
        assert_eq!((d.left.as_deref(), d.right.as_deref()), (Some(&b"a\nb\n"[..]), Some(&b"a\nB\n"[..])));
        assert_eq!(most_at_once(&log), 2, "the reads ran one after the other");
        // staged: the index look, both sides and the modes
        s.git(&["add", "f.txt"]);
        let (repo, log) = timed_git(&s, None);
        let d = DiffSession::load(repo, "f.txt".into(), None, Mode::Staged).unwrap();
        assert_eq!((d.left.as_deref(), d.right.as_deref()), (Some(&b"a\nb\n"[..]), Some(&b"a\nB\n"[..])));
        assert_eq!(most_at_once(&log), 3, "the reads ran one after the other");
    }

    /// Another client stages the file between the index look and the index text:
    /// what the session holds must still be one snapshot (the text of the entry's id).
    #[cfg(unix)]
    #[test]
    fn the_index_text_is_the_entrys_even_when_another_client_stages_in_between() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [Mode::Unstaged, Mode::Staged] {
            let s = scratch();
            s.commit("f.txt", "head\n", "base");
            s.write("f.txt", "one\n");
            s.git(&["add", "f.txt"]);
            s.write("f.txt", "two\n");
            let dir = s.dir.path().join("race");
            std::fs::create_dir_all(&dir).unwrap();
            let real = String::from_utf8(std::process::Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap().trim().to_owned();
            // After `ls-files` has answered, the other client stages "two"; the text read waits for that.
            let body = r#"#!/bin/sh
for a in "$@"; do
  case "$a" in
    ls-files) 'REAL' "$@" > "$0.out"; 'REAL' -C 'ROOT' add f.txt; cat "$0.out"; exit 0 ;;
    cat-file) sleep 0.5 ;;
  esac
done
exec 'REAL' "$@"
"#;
            let shim = dir.join("git");
            std::fs::write(&shim, body.replace("REAL", &real).replace("ROOT", s.root())).unwrap();
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
            let place = crate::exec::Place { git: shim.to_string_lossy().into_owned(), ..s.repo.place.clone() };
            let git_dir = s.repo.git_dir.clone();
            let repo = Arc::new(Repo::new(s.repo.id.clone(), place, git_dir.clone(), git_dir));
            let d = DiffSession::load(repo, "f.txt".into(), None, mode).unwrap();
            let oid = d.index_entry.as_ref().unwrap().1.clone();
            let side = if matches!(d.mode, Mode::Unstaged) { &d.left } else { &d.right };
            assert_eq!(s.git(&["cat-file", "blob", &oid]), "one\n", "the entry is the one from before the other client staged");
            assert_eq!(side.as_deref(), Some(&b"one\n"[..]), "the session holds text that is not the entry's");
        }
    }

    /// A link and a submodule entry read as they did with `:path`: a link is not
    /// converted by `.gitattributes` (eol, filters), a gitlink prints its commit object.
    #[cfg(unix)]
    #[test]
    fn links_and_submodule_entries_show_no_false_difference() {
        let s = scratch();
        s.write(".gitattributes", "* eol=crlf\n");
        std::os::unix::fs::symlink("target\nline", std::path::Path::new(s.root()).join("lnk")).unwrap();
        s.git(&["add", "."]);
        s.git(&["commit", "-q", "-m", "link"]);
        for mode in [Mode::Unstaged, Mode::Staged] {
            let d = DiffSession::load(s.repo.clone(), "lnk".into(), None, mode).unwrap();
            assert_eq!(d.left.as_deref(), Some(&b"target\nline"[..]));
            assert_eq!(d.right.as_deref(), Some(&b"target\nline"[..]), "the link's target was converted: a difference that is not there");
            assert!(d.changes.is_empty());
        }
        // A submodule entry: the index side is what `:sub` gave before (git prints the commit object).
        let head = s.git(&["rev-parse", "HEAD"]).trim().to_owned();
        s.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{head},sub")]);
        let d = DiffSession::load(s.repo.clone(), "sub".into(), None, Mode::Staged).unwrap();
        assert!(d.right.is_some());
        assert_eq!(d.right, blob(&s.repo, ":sub").unwrap());
        assert!(d.index_entry.as_ref().is_some_and(|e| e.0 == "160000"));
    }

    /// A commit or a comparison reads nothing from the index (an extra round trip, and a
    /// smudge filter to wait for, for a side it never shows).
    #[cfg(unix)]
    #[test]
    fn a_commit_or_a_comparison_does_not_read_the_index_text() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        let one = s.commit("f.txt", "one\n", "one");
        let two = s.commit("f.txt", "two\n", "two");
        let dir = s.dir.path().join("count");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let real = String::from_utf8(std::process::Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap().trim().to_owned();
        let body = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> 'LOG'\nexec 'REAL' \"$@\"\n".replace("LOG", &log.display().to_string()).replace("REAL", &real);
        let shim = dir.join("git");
        std::fs::write(&shim, body).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let place = crate::exec::Place { git: shim.to_string_lossy().into_owned(), ..s.repo.place.clone() };
        let git_dir = s.repo.git_dir.clone();
        let repo = Arc::new(Repo::new(s.repo.id.clone(), place, git_dir.clone(), git_dir));
        // `--path=` is how the index text is read by id (the sides of a commit are read by name).
        let by_id = |mode: Mode| {
            let _ = std::fs::remove_file(&log);
            DiffSession::load(repo.clone(), "f.txt".into(), None, mode).unwrap();
            std::fs::read_to_string(&log).unwrap().lines().filter(|l| l.contains("--path=")).count()
        };
        assert_eq!(by_id(Mode::Commit(two.clone())), 0);
        assert_eq!(by_id(Mode::Compare(one, Some(two))), 0);
        assert_eq!(by_id(Mode::Unstaged), 1);
        assert_eq!(by_id(Mode::Staged), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_read_is_not_an_answer() {
        let s = scratch();
        s.commit("f.txt", "a\n", "base");
        s.write("f.txt", "b\n");
        for (fail, mode) in [("ls-files", Mode::Unstaged), ("diff", Mode::Unstaged), ("ls-files", Mode::Staged)] {
            let (repo, _) = timed_git(&s, Some(fail));
            assert!(DiffSession::load(repo, "f.txt".into(), None, mode).is_err(), "{fail} failed and the diff was shown anyway");
        }
        // Nothing in the index: the left side is not asked for, and not missed.
        s.write("new.txt", "x\n");
        let (repo, _) = timed_git(&s, Some("cat-file"));
        let d = DiffSession::load(repo, "new.txt".into(), None, Mode::Unstaged).unwrap();
        assert_eq!((d.left, d.right.as_deref()), (None, Some(&b"x\n"[..])));
    }

    // yuzora(v010): copying from the pane. What a selection covers is the file's own text.
    #[test]
    fn copied_lines_are_the_files_own_text() {
        let s = scratch();
        s.commit("f.txt", "keep\n", "base");
        let text = "\tindent with a tab\n  trailing spaces  \n\n\u{6F22}\u{5B57}\u{FF21}\u{1F600}\ncrlf line\r\nlast line, no newline";
        s.write("f.txt", text);
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Unstaged).unwrap();
        let right = |from, to| d.lines_text(&[(Side::Right, from, to)]);
        assert_eq!(right(0, 2), "\tindent with a tab\n  trailing spaces  ", "tab and trailing spaces kept");
        assert_eq!(right(1, 3), "  trailing spaces  \n", "an empty line is a line");
        assert_eq!(right(3, 4), "\u{6F22}\u{5B57}\u{FF21}\u{1F600}", "wide characters kept");
        assert_eq!(right(4, 5), "crlf line", "only the line end goes, the carriage return with it");
        assert_eq!(right(5, 6), "last line, no newline", "a last line with no newline gets none");
        assert_eq!(d.lines_text(&[(Side::Left, 0, 1)]), "keep", "the left side is the index's text");
        assert_eq!(d.lines_text(&[(Side::Right, 5, 6), (Side::Left, 0, 1)]), "last line, no newline\nkeep", "runs in the order given, either side");
        assert_eq!(right(5, 99), "last line, no newline", "a run past the end stops at the last line");
        assert_eq!((right(99, 100), right(3, 3), right(4, 2)), (String::new(), String::new(), String::new()));
        assert_eq!(d.lines_text(&[(Side::Right, 1, 1), (Side::Right, 0, 1)]), "\tindent with a tab", "a run of no lines adds no empty line");
        assert_eq!(d.lines_text(&[]), "");

        s.commit("g.txt", "a\n", "g");
        std::fs::write(std::path::Path::new(s.root()).join("g.txt"), b"a\xffb\nsecond\n").unwrap();
        let g = DiffSession::load(s.repo.clone(), "g.txt".into(), None, Mode::Unstaged).unwrap();
        assert_eq!(g.lines_text(&[(Side::Right, 0, 2)]), "a\u{FFFD}b\nsecond", "a byte that is not UTF-8 shows as the replacement character");

        s.write("bin.dat", "a\0b\n");
        let b = DiffSession::load(s.repo.clone(), "bin.dat".into(), None, Mode::Unstaged).unwrap();
        assert_eq!(b.lines_text(&[(Side::Right, 0, 1)]), "", "a binary file shows no lines");
    }

    /// Two commits of `path`, and the first one's id: the patch of the second's diff applies to the first.
    fn two_texts(s: &crate::repo::testing::Scratch, path: &str, old: &str, new: &str) -> (String, String) {
        let one = s.commit(path, old, "old");
        let two = s.commit(path, new, "new");
        (one, two)
    }

    /// The tree at `rev` with `patch` applied, once `git apply --check` has accepted it (the branch is left behind: [`back`] returns to it).
    fn applied(s: &crate::repo::testing::Scratch, rev: &str, patch: &str) {
        s.git(&["checkout", "-q", "-f", "--detach", rev]);
        s.git(&["clean", "-fdq"]);
        s.write("g.patch", patch);
        s.git(&["apply", "--check", "g.patch"]);
        s.git(&["apply", "g.patch"]);
    }

    fn back(s: &crate::repo::testing::Scratch) {
        s.git(&["checkout", "-q", "-f", "main"]);
        s.git(&["clean", "-fdq"]);
    }

    fn lines_of(n: usize, change: impl Fn(usize) -> Option<Option<String>>) -> String {
        // `change(i)`: None keeps line i, Some(None) deletes it, Some(Some(t)) puts t in its place.
        (1..=n).map(|i| match change(i) { None => Some(format!("l{i}\t\n")), Some(t) => t.map(|t| format!("{t}\n")) }).flatten().collect()
    }

    #[test]
    fn a_patch_of_some_hunks_applies_to_the_old_text_and_makes_what_those_hunks_changed() {
        let s = scratch();
        let old = lines_of(20, |_| None);
        // A line changed, one far below it deleted, one added near that, the last changed.
        let new: String = (1..=20).flat_map(|i| match i {
            2 => vec!["L2 changed\t".to_owned(), "and one more".to_owned()],
            10 => vec![],
            12 => vec![format!("l12\t"), "added after 12".to_owned()],
            20 => vec!["last".to_owned()],
            i => vec![format!("l{i}\t")],
        }).map(|l| format!("{l}\n")).collect();
        let (one, two) = two_texts(&s, "f.txt", &old, &new);
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Commit(two)).unwrap();
        let (ll, rl) = (diff::lines(old.as_bytes()), diff::lines(new.as_bytes()));
        let cs = diff::diff_lines(&ll, &rl, Whitespace::Exact);
        assert_eq!(cs.len(), 4, "{cs:?}");
        let all = d.patch(Whitespace::Exact, &[0, 1, 2, 3]);
        assert!(all.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n"), "{all}");
        assert_eq!(all.matches("\n@@ ").count(), 4 - 1, "the two hunks with two lines between them share one: {all}");
        assert!(all.contains("\n@@ -7,9 +8,9 @@\n") && all.contains("\n@@ -17,4 +18,4 @@\n"), "the new side counts the line the first hunk added: {all}");
        let (middle, last) = (d.patch(Whitespace::Exact, &[1, 2]), d.patch(Whitespace::Exact, &[3]));
        assert!(middle.contains("\n@@ -7,9 +7,9 @@\n") && last.contains("\n@@ -17,4 +17,4 @@\n"), "without it, it counts nothing: {middle} {last}");
        applied(&s, &one, &all);
        assert_eq!(s.read("f.txt"), new, "every hunk in: the new text");
        for picked in [vec![0], vec![1, 2], vec![3], vec![0, 3], vec![2], vec![0, 1, 2]] {
            let patch = d.patch(Whitespace::Exact, &picked);
            applied(&s, &one, &patch);
            let sel = Selection { hunks: picked.iter().map(|h| (*h, None)).collect() };
            let want = String::from_utf8(combine(&ll, &rl, &cs, &sel, Pick::Apply)).unwrap();
            assert_eq!(s.read("f.txt"), want, "hunks {picked:?}: {patch}");
        }
        assert_eq!(d.patch(Whitespace::Exact, &[3, 0, 3, 0]), d.patch(Whitespace::Exact, &[0, 3]), "in order, once each");
        assert_eq!((d.patch(Whitespace::Exact, &[]), d.patch(Whitespace::Exact, &[4, 99])), (String::new(), String::new()), "no hunk, no patch");
    }

    #[test]
    fn hunks_with_a_hunk_between_them_keep_their_contexts_apart() {
        let s = scratch();
        // Changes at lines 3, 5 and 7, one line apart.
        let old: String = (1..=12).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l3\n", "L3\n").replace("l5\n", "L5\n").replace("l7\n", "L7\n");
        let (one, two) = two_texts(&s, "f.txt", &old, &new);
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Commit(two)).unwrap();
        let patch = d.patch(Whitespace::Exact, &[0, 2]);
        assert!(patch.contains("\n@@ -1,4 +1,4 @@\n l1\n l2\n-l3\n+L3\n l4\n@@ -6,5 +6,5 @@\n l6\n-l7\n+L7\n l8\n l9\n l10\n"), "the first stops at the line before the hunk left out, the second starts after it: {patch}");
        applied(&s, &one, &patch);
        assert_eq!(s.read("f.txt"), old.replace("l3\n", "L3\n").replace("l7\n", "L7\n"));
    }

    #[test]
    fn a_patch_names_a_new_file_a_deleted_one_a_renamed_one_and_a_name_with_spaces() {
        let s = scratch();
        let head = |s: &crate::repo::testing::Scratch| s.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let name = "dir/a file \u{6F22}.txt";
        let base = s.commit("keep.txt", "k\n", "base");
        let added = s.commit(name, "one\n\ttwo \n", "add");
        s.git(&["mv", name, "moved.txt"]);
        s.write("moved.txt", "one\n\ttwo \nthree\n");
        s.git(&["add", "-A"]);
        s.git(&["commit", "-q", "-m", "move and change"]);
        let moved = head(&s);
        s.git(&["rm", "-q", "moved.txt"]);
        s.git(&["commit", "-q", "-m", "delete"]);
        let gone = head(&s);

        let d = DiffSession::load(s.repo.clone(), name.into(), None, Mode::Commit(added.clone())).unwrap();
        let patch = d.patch(Whitespace::Exact, &[0]);
        assert!(patch.contains("new file mode 100644\n--- /dev/null\n+++ b/dir/a file \u{6F22}.txt\n@@ -0,0 +1,2 @@\n"), "{patch}");
        applied(&s, &base, &patch);
        assert_eq!(s.read(name), "one\n\ttwo \n", "a new file");
        back(&s);

        let d = DiffSession::load(s.repo.clone(), "moved.txt".into(), Some(name.into()), Mode::Commit(moved.clone())).unwrap();
        let patch = d.patch(Whitespace::Exact, &[0]);
        assert!(patch.contains("rename from dir/a file \u{6F22}.txt\nrename to moved.txt\n"), "{patch}");
        applied(&s, &added, &patch);
        assert_eq!((s.read("moved.txt"), std::path::Path::new(s.root()).join(name).exists()), ("one\n\ttwo \nthree\n".to_owned(), false), "renamed and changed");
        back(&s);

        let d = DiffSession::load(s.repo.clone(), "moved.txt".into(), None, Mode::Commit(gone)).unwrap();
        let patch = d.patch(Whitespace::Exact, &[0]);
        assert!(patch.contains("deleted file mode 100644\n--- a/moved.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n"), "{patch}");
        applied(&s, &moved, &patch);
        assert!(!std::path::Path::new(s.root()).join("moved.txt").exists(), "a deleted file");
    }

    #[test]
    fn a_patch_keeps_a_missing_last_newline_where_it_was_and_ends_a_line_that_gets_company() {
        for (old, new) in [("a\nb", "a\nb\nc\n"), ("a\nb\nc", "a\nB\nc"), ("a\nb\n", "a\nB"), ("a\nb", "a\nB\n"), ("a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\nd")] {
            let s = scratch();
            let (one, two) = two_texts(&s, "f.txt", old, new);
            let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Commit(two)).unwrap();
            let n = d.view("side", 3, Whitespace::Exact)["hunks"].as_array().unwrap().len();
            assert!(n > 0, "{old:?} to {new:?} has a hunk");
            let patch = d.patch(Whitespace::Exact, &(0..n).collect::<Vec<_>>());
            applied(&s, &one, &patch);
            assert_eq!(s.read("f.txt"), new, "{old:?} to {new:?}: {patch}");
        }
    }

    #[test]
    fn a_patch_of_a_view_that_ignores_whitespace_applies_to_the_old_text() {
        let s = scratch();
        let old = "a\n  b\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let new = "a\n    b\nc\nD\ne\nf\ng\nh\ni\nj\nk\n";
        let (one, two) = two_texts(&s, "f.txt", old, new);
        let d = DiffSession::load(s.repo.clone(), "f.txt".into(), None, Mode::Commit(two)).unwrap();
        assert_eq!(d.view("side", 3, Whitespace::Trim)["hunks"].as_array().unwrap().len(), 1, "the indent is not a hunk here");
        let patch = d.patch(Whitespace::Trim, &[0]);
        applied(&s, &one, &patch);
        assert_eq!(s.read("f.txt"), "a\n  b\nc\nD\ne\nf\ng\nh\ni\nj\nk\n", "the hunk that is there, the indent as it was");
    }
}
