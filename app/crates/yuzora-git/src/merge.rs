//! Three-way conflicts (IntelliJ A7): the base, ours and theirs of a
//! conflicted file come from the index stages (`:1:`, `:2:`, `:3:`), are
//! compared here, and the file is cut into chunks:
//!
//! - `equal`: nobody changed it;
//! - `ours` / `theirs`: one side changed it (not a conflict: applied by
//!   "apply non-conflicting");
//! - `same`: both changed it the same way;
//! - `conflict`: both changed it differently. When the two changes don't
//!   overlap word by word, `auto` holds the merged text (IntelliJ's
//!   "resolve simple conflicts").
//!
//! During a rebase git's "ours" is the branch being rebased onto and
//! "theirs" is your commit; the sides are swapped so "yours" is always your
//! work (IntelliJ's `isReverseRoot`, setting `swapSidesInRebase`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};

use crate::diff::{self, Change};
use crate::diffview::blob;
use crate::exec::{Git, GitError, GitResult};
use crate::lru::Lru;
use crate::repo::Repo;
use crate::status::Scope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Equal,
    Ours,
    Theirs,
    Same,
    Conflict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub kind: Kind,
    pub base: std::ops::Range<usize>,
    pub ours: std::ops::Range<usize>,
    pub theirs: std::ops::Range<usize>,
}

/// diff3 over numbered sequences: chunks covering all of base, ours and theirs.
pub fn merge3(base: &[u32], ours: &[u32], theirs: &[u32]) -> Vec<Chunk> {
    let co = diff::diff_ids(base, ours);
    let ct = diff::diff_ids(base, theirs);
    // Changes from both sides, by base position; touching changes group.
    let mut all: Vec<(usize, &Change)> = co.iter().map(|c| (0, c)).chain(ct.iter().map(|c| (1, c))).collect();
    all.sort_by_key(|(side, c)| (c.a.start, c.a.end, *side));
    let mut out = Vec::new();
    let (mut b, mut o, mut t) = (0usize, 0usize, 0usize);
    // Offsets of each side relative to base so far.
    let (mut do_, mut dt) = (0isize, 0isize);
    let mut i = 0;
    while i < all.len() {
        let start = all[i].1.a.start;
        let mut end = all[i].1.a.end;
        let mut group = vec![all[i]];
        i += 1;
        // Touching changes conflict too, as in git (no unchanged line between).
        while i < all.len() && all[i].1.a.start <= end {
            end = end.max(all[i].1.a.end);
            group.push(all[i]);
            i += 1;
        }
        if start > b {
            out.push(Chunk { kind: Kind::Equal, base: b..start, ours: o..o + (start - b), theirs: t..t + (start - b) });
        }
        let side_range = |side: usize, d: isize| -> std::ops::Range<usize> {
            let s = (start as isize + d) as usize;
            let grow: isize = group.iter().filter(|(x, _)| *x == side).map(|(_, c)| c.b.len() as isize - c.a.len() as isize).sum();
            s..((end as isize + d + grow) as usize)
        };
        let has = |side: usize| group.iter().any(|(x, _)| *x == side);
        let or = side_range(0, do_);
        let tr = side_range(1, dt);
        let kind = match (has(0), has(1)) {
            (true, false) => Kind::Ours,
            (false, true) => Kind::Theirs,
            _ if ours[or.clone()] == theirs[tr.clone()] => Kind::Same,
            _ => Kind::Conflict,
        };
        do_ += or.len() as isize - (end - start) as isize;
        dt += tr.len() as isize - (end - start) as isize;
        b = end;
        o = or.end;
        t = tr.end;
        out.push(Chunk { kind, base: start..end, ours: or, theirs: tr });
    }
    if b < base.len() || o < ours.len() || t < theirs.len() {
        out.push(Chunk { kind: Kind::Equal, base: b..base.len(), ours: o..ours.len(), theirs: t..theirs.len() });
    }
    out
}

fn number<'a>(seqs: [&[&'a [u8]]; 3]) -> [Vec<u32>; 3] {
    let mut ids: std::collections::HashMap<&'a [u8], u32> = std::collections::HashMap::new();
    seqs.map(|s| {
        s.iter()
            .map(|l| {
                let n = ids.len() as u32;
                *ids.entry(diff::strip_eol(l)).or_insert(n)
            })
            .collect()
    })
}

/// A conflict merged word by word, when the two sides' edits don't touch.
pub fn auto_resolve(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let tb = diff::tokens(base);
    let to = diff::tokens(ours);
    let tt = diff::tokens(theirs);
    let mut ids = std::collections::HashMap::new();
    let nb = token_ids(&mut ids, base, &tb);
    let no = token_ids(&mut ids, ours, &to);
    let nt = token_ids(&mut ids, theirs, &tt);
    let chunks = merge3(&nb, &no, &nt);
    let mut out = String::new();
    for c in &chunks {
        let piece = |s: &str, t: &[std::ops::Range<usize>], r: &std::ops::Range<usize>| if r.is_empty() { String::new() } else { s[t[r.start].start..t[r.end - 1].end].to_owned() };
        match c.kind {
            Kind::Conflict => return None,
            Kind::Theirs => out.push_str(&piece(theirs, &tt, &c.theirs)),
            _ => out.push_str(&piece(ours, &to, &c.ours)),
        }
    }
    Some(out)
}

fn token_ids<'a>(ids: &mut std::collections::HashMap<&'a str, u32>, s: &'a str, toks: &[std::ops::Range<usize>]) -> Vec<u32> {
    toks.iter()
        .map(|r| {
            let n = ids.len() as u32;
            *ids.entry(&s[r.clone()]).or_insert(n)
        })
        .collect()
}

pub struct ConflictSession {
    pub id: u64,
    pub repo: Arc<Repo>,
    pub path: String,
    /// Displayed sides swapped (rebase).
    pub swapped: bool,
    pub base: Option<Vec<u8>>,
    /// As displayed: yours.
    pub ours: Option<Vec<u8>>,
    /// As displayed: theirs.
    pub theirs: Option<Vec<u8>>,
    pub chunks: Vec<Chunk>,
}

impl ConflictSession {
    pub fn load(repo: Arc<Repo>, path: &str) -> GitResult<ConflictSession> {
        let op = crate::repo::operation(&repo);
        let swapped = op.kind == "rebasing" && crate::settings::get("swapSidesInRebase").as_bool().unwrap_or(true);
        let base = blob(&repo, &format!(":1:{path}"))?;
        let (mut ours, mut theirs) = (blob(&repo, &format!(":2:{path}"))?, blob(&repo, &format!(":3:{path}"))?);
        if swapped {
            std::mem::swap(&mut ours, &mut theirs);
        }
        let empty: &[u8] = &[];
        let (lb, lo, lt) = (diff::lines(base.as_deref().unwrap_or(empty)), diff::lines(ours.as_deref().unwrap_or(empty)), diff::lines(theirs.as_deref().unwrap_or(empty)));
        let [nb, no, nt] = number([&lb, &lo, &lt]);
        let chunks = merge3(&nb, &no, &nt);
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(ConflictSession { id: NEXT.fetch_add(1, Ordering::SeqCst), repo, path: path.to_owned(), swapped, base, ours, theirs, chunks })
    }

    fn sides(&self) -> [Vec<&[u8]>; 3] {
        let empty: &[u8] = &[];
        [diff::lines(self.base.as_deref().unwrap_or(empty)), diff::lines(self.ours.as_deref().unwrap_or(empty)), diff::lines(self.theirs.as_deref().unwrap_or(empty))]
    }

    pub fn view(&self) -> Value {
        let [b, o, t] = self.sides();
        let text = |ls: &[&[u8]], r: &std::ops::Range<usize>| -> Vec<String> { ls[r.clone()].iter().map(|l| String::from_utf8_lossy(diff::strip_eol(l)).replace('\t', "    ")).collect() };
        let binary = [&self.base, &self.ours, &self.theirs].iter().any(|s| s.as_ref().is_some_and(|x| x[..x.len().min(8000)].contains(&0)));
        let chunks: Vec<Value> = self
            .chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let auto = (c.kind == Kind::Conflict).then(|| auto_resolve(&text(&b, &c.base).join("\n"), &text(&o, &c.ours).join("\n"), &text(&t, &c.theirs).join("\n"))).flatten();
                json!({
                    "index": i,
                    "kind": match c.kind { Kind::Equal => "equal", Kind::Ours => "ours", Kind::Theirs => "theirs", Kind::Same => "same", Kind::Conflict => "conflict" },
                    "base": text(&b, &c.base), "ours": text(&o, &c.ours), "theirs": text(&t, &c.theirs),
                    "at": [c.base.start, c.ours.start, c.theirs.start],
                    "auto": auto,
                })
            })
            .collect();
        let op = crate::repo::operation(&self.repo);
        let label = |rev: &str| self.repo.run(Git::read(["log", "-1", "--format=%h %s", rev])).ok().filter(|o| o.ok()).map(|o| o.stdout_text().trim().to_owned());
        let other = match op.kind.as_str() {
            "merging" => label("MERGE_HEAD"),
            "cherry-picking" => label("CHERRY_PICK_HEAD"),
            "reverting" => label("REVERT_HEAD"),
            "rebasing" => label("REBASE_HEAD"),
            _ => None,
        };
        let head = label("HEAD");
        let (yours, theirs) = if self.swapped { (other, head) } else { (head, other) };
        json!({
            "id": self.id,
            "path": self.path,
            "swapped": self.swapped,
            "operation": op.kind,
            "labels": { "ours": yours, "theirs": theirs },
            "exists": { "base": self.base.is_some(), "ours": self.ours.is_some(), "theirs": self.theirs.is_some() },
            "binary": binary,
            "conflicts": self.chunks.iter().filter(|c| c.kind == Kind::Conflict).count(),
            "chunks": chunks,
        })
    }

    /// The result: each chunk as chosen (`ours`, `theirs`, `both`,
    /// `both-theirs-first`, `base`, `auto`, or `{text}`); unchosen
    /// non-conflicts take their change, unchosen conflicts are refused
    /// unless `markers` (git's conflict markers are written).
    pub fn result(&self, choices: &Value, markers: bool) -> GitResult<Vec<u8>> {
        let [b, o, t] = self.sides();
        let crlf = [&b, &o, &t].iter().any(|s| s.iter().any(|l| l.ends_with(b"\r\n")));
        let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
        let mut out: Vec<u8> = Vec::new();
        let mut put = |ls: &[&[u8]]| {
            for l in ls {
                if !out.is_empty() && !out.ends_with(b"\n") {
                    out.extend_from_slice(eol);
                }
                out.extend_from_slice(l);
            }
        };
        let text_lines = |s: &str| -> Vec<Vec<u8>> {
            if s.is_empty() {
                return Vec::new();
            }
            s.split('\n').map(|l| [l.as_bytes(), eol].concat()).collect()
        };
        for (i, c) in self.chunks.iter().enumerate() {
            let choice = choices.get(i.to_string());
            let pick = choice.and_then(Value::as_str);
            match (pick, &c.kind) {
                (Some("ours"), _) => put(&o[c.ours.clone()]),
                (Some("theirs"), _) => put(&t[c.theirs.clone()]),
                (Some("base"), _) => put(&b[c.base.clone()]),
                (Some("both"), _) => {
                    put(&o[c.ours.clone()]);
                    put(&t[c.theirs.clone()]);
                }
                (Some("both-theirs-first"), _) => {
                    put(&t[c.theirs.clone()]);
                    put(&o[c.ours.clone()]);
                }
                (Some("auto"), Kind::Conflict) => {
                    let join = |ls: &[&[u8]]| ls.iter().map(|l| String::from_utf8_lossy(diff::strip_eol(l)).into_owned()).collect::<Vec<_>>().join("\n");
                    let merged = auto_resolve(&join(&b[c.base.clone()]), &join(&o[c.ours.clone()]), &join(&t[c.theirs.clone()])).ok_or_else(|| GitError::Other(format!("chunk {i} can't be resolved automatically")))?;
                    let ls = text_lines(&merged);
                    put(&ls.iter().map(Vec::as_slice).collect::<Vec<_>>());
                }
                (None, _) if choice.and_then(|c| c.get("text")).is_some() => {}
                (None, Kind::Equal | Kind::Ours | Kind::Same) => put(&o[c.ours.clone()]),
                (None, Kind::Theirs) => put(&t[c.theirs.clone()]),
                (None, Kind::Conflict) if markers => {
                    let m = |s: &str| [s.as_bytes(), eol].concat();
                    put(&[&m("<<<<<<< yours")]);
                    put(&o[c.ours.clone()]);
                    put(&[&m("=======")]);
                    put(&t[c.theirs.clone()]);
                    put(&[&m(">>>>>>> theirs")]);
                }
                (None, Kind::Conflict) => return Err(GitError::Other(format!("chunk {i} is still a conflict"))),
                (Some(other), _) => return Err(GitError::Other(format!("unknown choice {other}"))),
            }
            if let Some(text) = choice.and_then(|c| c.get("text")).and_then(Value::as_str) {
                let ls = text_lines(text);
                put(&ls.iter().map(Vec::as_slice).collect::<Vec<_>>());
            }
        }
        Ok(out)
    }

    /// Writes the result and marks the file resolved (`git add`).
    pub fn resolve(&self, choices: &Value) -> GitResult<Value> {
        let content = self.result(choices, false)?;
        self.repo.write_file(&self.repo.abs(&self.path), &content)?;
        self.repo.run(Git::write(["add", "--", &self.path]))?.into_result()?;
        self.repo.invalidate(Scope::All);
        Ok(json!({ "ok": true }))
    }
}

/// Takes one side for whole files (`ours` = yours as displayed).
pub fn accept(repo: &Repo, paths: &[String], side: &str) -> GitResult<Value> {
    let op = crate::repo::operation(repo);
    let swapped = op.kind == "rebasing" && crate::settings::get("swapSidesInRebase").as_bool().unwrap_or(true);
    let git_side = match (side, swapped) {
        ("ours", false) | ("theirs", true) => "ours",
        ("theirs", false) | ("ours", true) => "theirs",
        _ => return Err(GitError::Other(format!("unknown side {side}"))),
    };
    let stage = if git_side == "ours" { 2 } else { 3 };
    for p in paths {
        // A side that deleted the file: accepting it deletes the file.
        let exists = repo.run(Git::read(["cat-file", "-e", &format!(":{stage}:{p}")]))?.ok();
        if exists {
            repo.run(Git::write(["checkout", &format!("--{git_side}"), "--", p]))?.into_result()?;
            repo.run(Git::write(["add", "--", p]))?.into_result()?;
        } else {
            repo.run(Git::write(["rm", "-q", "--", p]))?.into_result()?;
        }
    }
    repo.invalidate(Scope::All);
    Ok(json!({ "ok": true }))
}

fn sessions() -> &'static Mutex<Lru<u64, Arc<ConflictSession>>> {
    static S: OnceLock<Mutex<Lru<u64, Arc<ConflictSession>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Lru::new(32)))
}

pub fn keep(s: ConflictSession) -> Arc<ConflictSession> {
    let s = Arc::new(s);
    sessions().lock().unwrap_or_else(|e| e.into_inner()).put(s.id, s.clone());
    s
}

pub fn get(id: u64) -> GitResult<Arc<ConflictSession>> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(&id).ok_or_else(|| GitError::other("this conflict view is gone; reload it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;

    fn kinds(c: &[Chunk]) -> Vec<Kind> {
        c.iter().map(|c| c.kind.clone()).collect()
    }

    #[test]
    fn diff3_separates_one_sided_changes_from_conflicts() {
        // base: a b c d e; ours changes b; theirs changes d; both change... nothing else.
        let c = merge3(&[1, 2, 3, 4, 5], &[1, 20, 3, 4, 5], &[1, 2, 3, 40, 5]);
        assert_eq!(kinds(&c), [Kind::Equal, Kind::Ours, Kind::Equal, Kind::Theirs, Kind::Equal]);
        let c = merge3(&[1, 2, 3], &[1, 20, 3], &[1, 21, 3]);
        assert_eq!(kinds(&c), [Kind::Equal, Kind::Conflict, Kind::Equal]);
        assert_eq!((c[1].ours.clone(), c[1].theirs.clone()), (1..2, 1..2));
        let c = merge3(&[1, 2, 3], &[1, 9, 3], &[1, 9, 3]);
        assert_eq!(kinds(&c), [Kind::Equal, Kind::Same, Kind::Equal]);
        // Insertions shift what follows on that side only.
        let c = merge3(&[1, 2, 3], &[0, 1, 2, 3], &[1, 2, 3, 4]);
        assert_eq!(kinds(&c), [Kind::Ours, Kind::Equal, Kind::Theirs]);
        assert_eq!(c[1].ours, 1..4);
        assert_eq!(c[2].theirs, 3..4);
    }

    #[test]
    fn simple_conflicts_resolve_word_by_word() {
        assert_eq!(auto_resolve("let a = 1; let b = 2;", "let a = 10; let b = 2;", "let a = 1; let b = 20;").as_deref(), Some("let a = 10; let b = 20;"));
        assert_eq!(auto_resolve("x = 1", "x = 2", "x = 3"), None);
    }

    #[test]
    fn a_conflicted_file_is_loaded_resolved_and_added() {
        let s = scratch();
        s.commit("f.txt", "top\nmid\nend\n", "base");
        s.git(&["switch", "-q", "-c", "other"]);
        s.commit("f.txt", "top\nTHEIRS\nend\nadded\n", "theirs");
        s.git(&["switch", "-q", "main"]);
        s.commit("f.txt", "TOP\nOURS\nend\n", "ours");
        assert!(!s.repo.run(Git::write(["merge", "other"])).unwrap().ok());
        let c = ConflictSession::load(s.repo.clone(), "f.txt").unwrap();
        let v = c.view();
        assert_eq!(v["swapped"], false);
        assert_eq!(v["conflicts"], 1);
        let kinds: Vec<&str> = v["chunks"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["conflict", "equal", "theirs"], "{v}");
        assert!(c.result(&json!({}), false).is_err());
        let with_markers = String::from_utf8(c.result(&json!({}), true).unwrap()).unwrap();
        assert!(with_markers.contains("<<<<<<< yours\nTOP\nOURS\n=======\ntop\nTHEIRS\n>>>>>>> theirs\n"));
        c.resolve(&json!({ "0": "ours" })).unwrap();
        assert_eq!(s.read("f.txt"), "TOP\nOURS\nend\nadded\n");
        assert_eq!(s.git(&["diff", "--name-only", "--diff-filter=U"]), "");
    }

    #[test]
    fn during_a_rebase_yours_is_your_commit() {
        let s = scratch();
        s.commit("f.txt", "base\n", "base");
        s.git(&["switch", "-q", "-c", "mine"]);
        s.commit("f.txt", "mine\n", "my change");
        s.git(&["switch", "-q", "main"]);
        s.commit("f.txt", "upstream\n", "upstream change");
        s.git(&["switch", "-q", "mine"]);
        assert!(!s.repo.run(Git::write(["rebase", "main"])).unwrap().ok());
        let c = ConflictSession::load(s.repo.clone(), "f.txt").unwrap();
        assert!(c.swapped);
        let v = c.view();
        assert_eq!(v["chunks"][0]["ours"], json!(["mine"]));
        assert_eq!(v["chunks"][0]["theirs"], json!(["upstream"]));
        accept(&s.repo, &["f.txt".into()], "ours").unwrap();
        assert_eq!(s.read("f.txt"), "mine\n");
    }
}
