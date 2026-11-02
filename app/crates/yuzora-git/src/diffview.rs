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
use crate::repo::Repo;
use crate::status::Scope;

/// Bigger sides aren't diffed (shown as "too large").
pub const MAX_SIDE: usize = 16 << 20;
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
    /// Exact changes (what staging uses).
    pub changes: Vec<Change>,
}

/// `git cat-file --filters <spec>`, or None when it doesn't exist there.
pub fn blob(repo: &Repo, spec: &str) -> GitResult<Option<Vec<u8>>> {
    let out = repo.run(Git::read(["cat-file", "--filters", spec]).max_output(MAX_SIDE + 1))?;
    Ok(out.ok().then_some(out.stdout))
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
    if !repo.place.is_remote() && std::fs::symlink_metadata(&abs).is_ok_and(|m| m.file_type().is_symlink()) {
        // A link's content in git is its target.
        return std::fs::read_link(&abs).ok().map(|t| t.to_string_lossy().into_owned().into_bytes());
    }
    // One byte past the limit is enough to call it too large.
    repo.read_file_head(&abs, MAX_SIDE + 1)
}

fn is_link(repo: &Repo, path: &str, index_mode: Option<&str>) -> bool {
    index_mode == Some("120000") || (!repo.place.is_remote() && std::fs::symlink_metadata(repo.abs(path)).is_ok_and(|m| m.file_type().is_symlink()))
}

impl DiffSession {
    pub fn load(repo: Arc<Repo>, path: String, orig: Option<String>, mode: Mode) -> GitResult<DiffSession> {
        let before = orig.clone().unwrap_or_else(|| path.clone());
        let index_entry = index_entry(&repo, &path)?;
        let (left, right) = match &mode {
            Mode::Unstaged => (if index_entry.is_some() { blob(&repo, &format!(":{path}"))? } else { None }, worktree(&repo, &path)),
            Mode::Staged => (blob(&repo, &format!("HEAD:{before}"))?, blob(&repo, &format!(":{path}"))?),
            Mode::Commit(rev) => (blob(&repo, &format!("{rev}^1:{before}"))?, blob(&repo, &format!("{rev}:{path}"))?),
            Mode::Compare(a, b) => (blob(&repo, &format!("{a}:{before}"))?, match b {
                Some(b) => blob(&repo, &format!("{b}:{path}"))?,
                None => worktree(&repo, &path),
            }),
        };
        let too_large = left.as_ref().is_some_and(|l| l.len() > MAX_SIDE) || right.as_ref().is_some_and(|r| r.len() > MAX_SIDE);
        let binary = !too_large && [&left, &right].iter().any(|s| s.as_ref().is_some_and(|b| b[..b.len().min(8000)].contains(&0)));
        let changes = if too_large || binary {
            Vec::new()
        } else {
            let (l, r) = (left.as_deref().unwrap_or_default(), right.as_deref().unwrap_or_default());
            diff::diff_lines(&diff::lines(l), &diff::lines(r), Whitespace::Exact)
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(DiffSession { id: NEXT.fetch_add(1, Ordering::SeqCst), repo, path, orig, mode, left, right, index_entry, binary, too_large, changes })
    }

    /// The view: hunks and rows (`side` or `unified`), `context` lines around
    /// changes (-1: all), whitespace policy.
    pub fn view(&self, layout: &str, context: i64, ws: Whitespace) -> Value {
        let l = self.left.as_deref().unwrap_or_default();
        let r = self.right.as_deref().unwrap_or_default();
        let ll = diff::lines(l);
        let rl = diff::lines(r);
        let changes = if ws == Whitespace::Exact { self.changes.clone() } else if self.binary || self.too_large { Vec::new() } else { diff::diff_lines(&ll, &rl, ws) };
        let add: usize = changes.iter().map(|c| c.b.len()).sum();
        let del: usize = changes.iter().map(|c| c.a.len()).sum();
        let hunks: Vec<Value> = changes.iter().enumerate().map(|(i, c)| json!({ "index": i, "a": [c.a.start, c.a.end], "b": [c.b.start, c.b.end] })).collect();
        let rows = if self.binary || self.too_large { Vec::new() } else { rows(&ll, &rl, &changes, layout == "unified", context, ws) };
        json!({
            "id": self.id,
            "path": self.path,
            "orig": self.orig,
            "mode": self.mode.name(),
            "binary": self.binary,
            "tooLarge": self.too_large,
            "leftExists": self.left.is_some(),
            "rightExists": self.right.is_some(),
            "identical": changes.is_empty() && self.left == self.right,
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

    fn covers_all(&self, changes: &[Change]) -> bool {
        !changes.is_empty() && (0..changes.len()).all(|h| matches!(self.hunks.get(&h), Some(None)))
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

fn new_file_mode(repo: &Repo, path: &str) -> &'static str {
    if is_link(repo, path, None) {
        return "120000";
    }
    #[cfg(unix)]
    if !repo.place.is_remote() {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::metadata(repo.abs(path)).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
            return "100755";
        }
    }
    let _ = (repo, path);
    "100644"
}

impl DiffSession {
    fn sides(&self) -> (Vec<&[u8]>, Vec<&[u8]>) {
        (diff::lines(self.left.as_deref().unwrap_or_default()), diff::lines(self.right.as_deref().unwrap_or_default()))
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
        self.check_index()?;
        let p = vec![self.path.clone()];
        if sel.covers_all(&self.changes) || (self.changes.is_empty() && self.left != self.right) {
            // Whole file: let git record deletions, modes and new files itself.
            self.repo.run(Git::write(["add", "-A"]).pathspec(&p))?.into_result()?;
        } else {
            let (l, r) = self.sides();
            let content = combine(&l, &r, &self.changes, sel, Pick::Apply);
            let mode = self.index_entry.as_ref().map(|e| e.0.clone()).unwrap_or_else(|| new_file_mode(&self.repo, &self.path).to_owned());
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
        if worktree(&self.repo, &self.path) != self.right {
            return Err(GitError::other("the file changed since this diff was read; reload it"));
        }
        let (l, r) = self.sides();
        let abs = self.repo.abs(&self.path);
        let all = sel.covers_all(&self.changes);
        let p = vec![self.path.clone()];
        if all && self.left.is_none() {
            crate::repo::remove_path(&self.repo.place, &abs)?;
        } else if all {
            // Back to the index: git restores the type, mode and filters.
            self.repo.run(Git::write(["restore", "--worktree"]).pathspec(&p))?.into_result()?;
        } else if is_link(&self.repo, &self.path, self.index_entry.as_ref().map(|e| e.0.as_str())) {
            return Err(GitError::other("a symbolic link changes as a whole: discard all of it"));
        } else {
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
}
