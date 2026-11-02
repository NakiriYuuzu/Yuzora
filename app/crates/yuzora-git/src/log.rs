//! The log, read the way IntelliJ reads it (B1):
//!
//! 1. **first block**: `git log HEAD --branches --remotes --max-count=N`
//!    (no `--tags`, no sorting: both cost on big repos) with full metadata,
//!    sorted here into date-topological order. The first screen shows from
//!    this;
//! 2. **all hashes**: `git log HEAD --branches --remotes --tags --date-order
//!    --format=%H %P %ct`, parsed as it streams, becomes the full graph.
//!    Metadata for rows past the first block is read when they are shown
//!    (`--no-walk=unsorted --stdin`, a batch at a time);
//! 3. **refresh**: when refs move, the most recent commits are read again and
//!    joined onto the loaded graph; only when that can't be done (a reset, a
//!    deleted branch, a long fetch) is everything read again.
//!
//! Fields are separated by control characters (`%x01%x01` record start,
//! `%x02%x02` between fields, `%x03%x03` record end), as IntelliJ's
//! `GitLogParser`.
//!
//! Filters (branch, user, date, path, text, hash, range, no merges) go to
//! git with `--max-count` raised in stages (100, 2000, all), each stage
//! replacing the last, since there is no persistent index (beta.2 keeps the
//! seam: [`Filter`] is the query an index would answer).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use serde_json::{Value, json};

use crate::exec::{Cancel, Git, GitError, GitResult};
use crate::graph::{Graph, RawCommit, RefKind, RowPrint, topo_sort};
use crate::lru::Lru;
use crate::repo::Repo;

pub const RECORD_START: &str = "\x01\x01";
pub const FIELD: &str = "\x02\x02";
pub const RECORD_END: &str = "\x03\x03";
/// Metadata for the list: hash, parents, commit time, author time, author, email, subject.
const META_FORMAT: &str = "--format=%x01%x01%H%x02%x02%P%x02%x02%ct%x02%x02%at%x02%x02%an%x02%x02%ae%x02%x02%s%x03%x03";
/// A short list (push preview, compare): hash, subject, author, email, author time.
pub const META_FORMAT_LIST: &str = "--format=%x01%x01%H%x02%x02%s%x02%x02%an%x02%x02%ae%x02%x02%at%x03%x03";
/// Rows of metadata read per batch past the first block.
const META_BATCH: usize = 500;
/// Commits re-read on a refresh before giving up on joining.
const RECENT: usize = 1000;
/// Filter stages (IntelliJ's `CommitCountStage`, after its in-memory 5).
const STAGES: [usize; 4] = [5, 100, 2000, usize::MAX];

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub subject: String,
    pub author: String,
    pub email: String,
    pub time: i64,
}

/// A log query. Empty means everything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub branches: Vec<String>,
    pub users: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub paths: Vec<String>,
    pub text: Option<String>,
    pub regex: bool,
    pub match_case: bool,
    pub hashes: Vec<String>,
    pub range: Option<String>,
    pub no_merges: bool,
    /// Follow one file through renames (file history).
    pub follow: bool,
}

impl Filter {
    pub fn from_json(v: &Value) -> Filter {
        let list = |k: &str| -> Vec<String> {
            match v.get(k) {
                Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(str::to_owned).filter(|s| !s.trim().is_empty()).collect(),
                Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
                _ => Vec::new(),
            }
        };
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        Filter {
            branches: list("branches"),
            users: list("users"),
            since: text("since"),
            until: text("until"),
            paths: list("paths"),
            text: text("text"),
            regex: v.get("regex").and_then(Value::as_bool).unwrap_or(false),
            match_case: v.get("matchCase").and_then(Value::as_bool).unwrap_or(false),
            hashes: list("hashes"),
            range: text("range"),
            no_merges: v.get("noMerges").and_then(Value::as_bool).unwrap_or(false),
            follow: v.get("follow").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// Whether git rewrites parents for this filter, so the graph still means
    /// something (branches, ranges, paths); text, user and date filters show
    /// the matches as a list.
    pub fn keeps_graph(&self) -> bool {
        self.users.is_empty() && self.text.is_none() && self.since.is_none() && self.until.is_none() && self.hashes.is_empty() && !self.no_merges
    }

    /// The revisions and options for `git log`.
    pub fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if !self.hashes.is_empty() {
            a.push("--no-walk=unsorted".into());
            a.extend(self.hashes.iter().cloned());
        } else if let Some(r) = &self.range {
            a.push(r.clone());
        } else if !self.branches.is_empty() {
            a.extend(self.branches.iter().cloned());
        } else {
            a.extend(["HEAD".into(), "--branches".into(), "--remotes".into(), "--tags".into()]);
        }
        for u in &self.users {
            a.push(format!("--author={u}"));
        }
        if let Some(s) = &self.since {
            a.push(format!("--since={s}"));
        }
        if let Some(u) = &self.until {
            a.push(format!("--until={u}"));
        }
        if let Some(t) = &self.text {
            a.push(format!("--grep={t}"));
            a.push(if self.regex { "--extended-regexp".into() } else { "--fixed-strings".into() });
            if !self.match_case {
                a.push("--regexp-ignore-case".into());
            }
        }
        if self.no_merges {
            a.push("--no-merges".into());
        }
        if self.keeps_graph() && self.hashes.is_empty() {
            a.push("--parents".into());
            a.push("--date-order".into());
        }
        if !self.paths.is_empty() {
            if self.follow && self.paths.len() == 1 {
                a.push("--follow".into());
            } else {
                a.extend(["--full-history".into(), "--simplify-merges".into()]);
            }
            a.push("--".into());
            a.extend(self.paths.iter().cloned());
        }
        a
    }
}

/// Parses `META_FORMAT` records as they stream (records may split anywhere).
#[derive(Default)]
pub struct MetaParser {
    buf: Vec<u8>,
}

impl MetaParser {
    pub fn feed(&mut self, chunk: &[u8], mut out: impl FnMut(RawCommit, Meta)) {
        self.buf.extend_from_slice(chunk);
        let mut consumed = 0;
        loop {
            let rest = &self.buf[consumed..];
            let Some(start) = find(rest, RECORD_START.as_bytes()) else { break };
            let body = &rest[start + 2..];
            let Some(end) = find(body, RECORD_END.as_bytes()) else { break };
            let rec = String::from_utf8_lossy(&body[..end]).into_owned();
            consumed += start + 2 + end + 2;
            let f: Vec<&str> = rec.split(FIELD).collect();
            if f.len() < 7 {
                continue;
            }
            let commit = RawCommit { hash: hex(f[0]), parents: f[1].split_whitespace().map(hex).collect(), time: f[2].parse().unwrap_or(0) };
            let meta = Meta { subject: f[6].to_owned(), author: f[4].to_owned(), email: f[5].to_owned(), time: f[3].parse().unwrap_or(0) };
            out(commit, meta);
        }
        self.buf.drain(..consumed);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Hex text to bytes (an odd or bad digit leaves its byte as 0).
pub fn hex(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    (0..b.len() / 2).map(|i| (val(b[2 * i]) << 4) | val(b[2 * i + 1])).collect()
}

fn val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Parses `%H%x00%P%x00%ct` lines as they stream.
#[derive(Default)]
pub struct HashParser {
    buf: Vec<u8>,
}

impl HashParser {
    pub fn feed(&mut self, chunk: &[u8], mut out: impl FnMut(RawCommit)) {
        self.buf.extend_from_slice(chunk);
        let mut consumed = 0;
        while let Some(nl) = self.buf[consumed..].iter().position(|b| *b == b'\n') {
            let line = &self.buf[consumed..consumed + nl];
            consumed += nl + 1;
            let mut f = line.split(|b| *b == 0);
            let (Some(h), Some(p), Some(t)) = (f.next(), f.next(), f.next()) else { continue };
            let h = String::from_utf8_lossy(h);
            if h.is_empty() {
                continue;
            }
            out(RawCommit {
                hash: hex(&h),
                parents: String::from_utf8_lossy(p).split_whitespace().map(hex).collect(),
                time: String::from_utf8_lossy(t).trim().parse().unwrap_or(0),
            });
        }
        self.buf.drain(..consumed);
    }
}

/// One ref label on a commit.
#[derive(Clone, Debug, PartialEq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug, Default)]
pub struct Refs {
    pub by_commit: HashMap<Vec<u8>, Vec<RefLabel>>,
    pub head: Option<Vec<u8>>,
    pub branch: Option<String>,
}

impl Refs {
    pub fn kinds(&self) -> HashMap<Vec<u8>, RefKind> {
        self.by_commit.iter().map(|(h, l)| (h.clone(), l.iter().map(|x| x.kind).min().unwrap_or(RefKind::Tag))).collect()
    }
}

fn is_main(name: &str) -> bool {
    matches!(name, "main" | "master" | "trunk" | "develop")
}

/// Branches, remotes and tags with the commits they point at.
pub fn read_refs(repo: &Repo) -> GitResult<Refs> {
    let out = repo.run(Git::read(["for-each-ref", "--format=%(objectname)%00%(*objectname)%00%(refname)", "refs/heads", "refs/remotes", "refs/tags"]))?.into_result()?;
    let branch = repo.run(Git::read(["symbolic-ref", "-q", "--short", "HEAD"]))?;
    let branch = branch.ok().then(|| branch.stdout_text().trim().to_owned()).filter(|b| !b.is_empty());
    let head = repo.run(Git::read(["rev-parse", "-q", "--verify", "HEAD^{commit}"]))?;
    let head = head.ok().then(|| hex(head.stdout_text().trim()));
    let mut refs = Refs { head: head.clone(), branch: branch.clone(), ..Default::default() };
    for line in String::from_utf8_lossy(&out).lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 3 {
            continue;
        }
        let target = if f[1].is_empty() { f[0] } else { f[1] };
        let name = f[2];
        let (label, kind) = if let Some(b) = name.strip_prefix("refs/heads/") {
            let kind = if Some(b) == branch.as_deref() { RefKind::Current } else if is_main(b) { RefKind::Main } else { RefKind::Local };
            (b.to_owned(), kind)
        } else if let Some(r) = name.strip_prefix("refs/remotes/") {
            if r.ends_with("/HEAD") {
                continue;
            }
            (r.to_owned(), RefKind::Remote)
        } else if let Some(t) = name.strip_prefix("refs/tags/") {
            (t.to_owned(), RefKind::Tag)
        } else {
            continue;
        };
        refs.by_commit.entry(hex(target)).or_default().push(RefLabel { name: label, kind });
    }
    if branch.is_none()
        && let Some(h) = head
    {
        refs.by_commit.entry(h).or_default().insert(0, RefLabel { name: "HEAD".into(), kind: RefKind::Current });
    }
    for labels in refs.by_commit.values_mut() {
        labels.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    }
    Ok(refs)
}

#[derive(Default)]
pub struct LogData {
    pub graph: Arc<Graph>,
    pub meta: HashMap<Vec<u8>, Arc<Meta>>,
    pub refs: Refs,
    pub loading: bool,
    pub stage: &'static str,
    pub complete: bool,
    pub error: Option<String>,
    /// Whether the rows are a graph (false: a filtered list).
    pub graph_shown: bool,
    pub width: u32,
    pub first_ms: Option<u64>,
    pub full_ms: Option<u64>,
    pub refs_generation: u64,
}

pub struct LogSession {
    pub id: u64,
    pub repo: Arc<Repo>,
    pub filter: Filter,
    pub data: RwLock<LogData>,
    generation: AtomicU64,
    waiters: Mutex<Vec<async_channel::Sender<u64>>>,
    rows: Mutex<Lru<u32, Arc<RowPrint>>>,
    loading_meta: Mutex<HashSet<Vec<u8>>>,
    pub cancel: Mutex<Cancel>,
}

impl LogSession {
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn bump(&self) {
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        for tx in std::mem::take(&mut *self.waiters.lock().unwrap_or_else(|e| e.into_inner())) {
            let _ = tx.try_send(g);
        }
    }

    /// Resolves when the session changed past `since` (or the repo's refs
    /// moved, which starts a refresh first).
    pub fn wait(self: &Arc<Self>, since: u64) -> impl std::future::Future<Output = u64> + Send + use<> {
        let (tx, rx) = async_channel::bounded(1);
        let now = self.generation();
        if now > since {
            let _ = tx.try_send(now);
        } else {
            self.waiters.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        }
        let repo_wait = self.repo.wait(self.repo.generation());
        let me = self.clone();
        async move {
            // Whichever comes first: this session changed, or the repo did.
            let either = futures_or(rx.recv(), repo_wait);
            match either.await {
                Either::Left(g) => g.unwrap_or(now),
                Either::Right(_) => {
                    me.refresh_if_refs_moved();
                    me.generation()
                }
            }
        }
    }

    fn set_graph(&self, d: &mut LogData, graph: Graph) {
        d.width = graph.width(500).max(1);
        d.graph = Arc::new(graph);
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// One row's drawing, cached.
    pub fn row_print(&self, row: u32) -> Option<Arc<RowPrint>> {
        if let Some(p) = self.rows.lock().unwrap_or_else(|e| e.into_inner()).get(&row) {
            return Some(p);
        }
        // A blocking read: writers only hold the lock to swap in a graph or
        // add metadata, and a cell left empty for a frame would flicker.
        let d = self.data.read().unwrap_or_else(|e| e.into_inner());
        if !d.graph_shown || row as usize >= d.graph.len() {
            return None;
        }
        let p = Arc::new(d.graph.row(row));
        drop(d);
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).put(row, p.clone());
        Some(p)
    }

    /// Rows `start..end` as scripts show them; missing metadata is fetched.
    pub fn rows(self: &Arc<Self>, start: usize, end: usize) -> Value {
        let d = self.data.read().unwrap_or_else(|e| e.into_inner());
        let g = d.graph.clone();
        let end = end.min(g.len());
        let mut missing = Vec::new();
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for r in start..end {
            let h = g.hash(r as u32);
            let m = d.meta.get(h);
            if m.is_none() {
                missing.push(h.to_vec());
            }
            let refs: Vec<Value> = d.refs.by_commit.get(h).map(|l| l.iter().map(|x| json!({ "name": x.name, "kind": kind_name(x.kind) })).collect()).unwrap_or_default();
            let hash = to_hex(h);
            out.push(json!({
                "row": r,
                "hash": hash,
                "short": &hash[..hash.len().min(8)],
                "subject": m.map(|m| m.subject.as_str()),
                "author": m.map(|m| m.author.as_str()),
                "email": m.map(|m| m.email.as_str()),
                "time": m.map(|m| m.time),
                "refs": refs,
                "head": d.refs.head.as_deref() == Some(h),
                "merge": g.parent_count(r as u32) > 1,
            }));
        }
        drop(d);
        if !missing.is_empty() {
            self.load_meta(missing);
        }
        Value::Array(out)
    }

    fn load_meta(self: &Arc<Self>, hashes: Vec<Vec<u8>>) {
        let todo: Vec<Vec<u8>> = {
            let mut busy = self.loading_meta.lock().unwrap_or_else(|e| e.into_inner());
            hashes.into_iter().filter(|h| busy.insert(h.clone())).collect()
        };
        if todo.is_empty() {
            return;
        }
        let me = self.clone();
        let _ = std::thread::Builder::new().name("yuzora-git-meta".into()).spawn(move || {
            for batch in todo.chunks(META_BATCH) {
                let input: String = batch.iter().map(|h| to_hex(h) + "\n").collect();
                let mut got = Vec::new();
                let mut p = MetaParser::default();
                let r = me.repo.run(Git::read(["log", "--no-walk=unsorted", "--stdin", "--encoding=UTF-8", META_FORMAT]).stdin(input.into_bytes()));
                if let Ok(out) = r {
                    p.feed(&out.stdout, |c, m| got.push((c.hash, Arc::new(m))));
                }
                {
                    let mut d = me.data.write().unwrap_or_else(|e| e.into_inner());
                    for (h, m) in got {
                        d.meta.insert(h, m);
                    }
                }
                let mut busy = me.loading_meta.lock().unwrap_or_else(|e| e.into_inner());
                for h in batch {
                    busy.remove(h);
                }
                drop(busy);
                me.bump();
            }
        });
    }

    pub fn info(&self) -> Value {
        let d = self.data.read().unwrap_or_else(|e| e.into_inner());
        json!({
            "id": self.id,
            "generation": self.generation(),
            "rows": d.graph.len(),
            "loading": d.loading,
            "stage": d.stage,
            "complete": d.complete,
            "error": d.error,
            "graph": d.graph_shown,
            "width": d.width,
            "firstMs": d.first_ms,
            "fullMs": d.full_ms,
            "branch": d.refs.branch,
        })
    }

    /// The row of a commit by (a prefix of) its hash, or a ref name.
    pub fn find(&self, query: &str) -> Option<u32> {
        let d = self.data.read().unwrap_or_else(|e| e.into_inner());
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        for (h, labels) in &d.refs.by_commit {
            if labels.iter().any(|l| l.name.to_lowercase() == q) {
                return d.graph.row_of(h);
            }
        }
        if q.len() >= 4 && q.chars().all(|c| c.is_ascii_hexdigit()) {
            return (0..d.graph.len() as u32).find(|&r| to_hex(d.graph.hash(r)).starts_with(&q));
        }
        None
    }

    fn refresh_if_refs_moved(self: &Arc<Self>) {
        let moved = self.data.read().is_ok_and(|d| !d.loading && d.refs_generation != self.repo.refs_generation());
        if moved {
            self.start(true);
        }
    }

    /// Loads (or refreshes) in the background.
    pub fn start(self: &Arc<Self>, refresh: bool) {
        {
            let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
            if d.loading {
                return;
            }
            d.loading = true;
            d.refs_generation = self.repo.refs_generation();
        }
        let cancel = Cancel::new();
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = cancel.clone();
        self.bump();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("yuzora-git-log".into()).spawn(move || {
            let r = if !me.filter.is_empty() {
                me.load_filtered(&cancel)
            } else if refresh && me.try_join(&cancel).unwrap_or(false) {
                Ok(())
            } else {
                me.load_all(&cancel)
            };
            let mut d = me.data.write().unwrap_or_else(|e| e.into_inner());
            d.loading = false;
            if let Err(e) = r {
                if e != GitError::Cancelled {
                    d.error = Some(e.to_string());
                }
            } else {
                d.error = None;
            }
            drop(d);
            me.bump();
        });
    }

    fn load_all(self: &Arc<Self>, cancel: &Cancel) -> GitResult<()> {
        let started = Instant::now();
        let refs = read_refs(&self.repo)?;
        if refs.head.is_none() && refs.by_commit.is_empty() {
            let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
            d.refs = refs;
            self.set_graph(&mut d, Graph::default());
            d.complete = true;
            d.stage = "done";
            d.graph_shown = true;
            return Ok(());
        }
        let revs = start_revs(&refs, false);
        // 1. The first block, with metadata.
        let n = (crate::settings::number("logRecentCount") as usize).clamp(100, 100_000);
        let mut commits = Vec::new();
        let mut meta = HashMap::new();
        let mut p = MetaParser::default();
        let mut sink = |b: &[u8]| {
            p.feed(b, |c, m| {
                meta.insert(c.hash.clone(), Arc::new(m));
                commits.push(c);
            });
            !cancel.is_cancelled()
        };
        let cmd = Git::read(["log", "--encoding=UTF-8", META_FORMAT, &format!("--max-count={n}")]).args(revs.clone()).arg("--").cancel(Some(cancel.clone()));
        crate::exec::run_streaming(&self.repo.place, cmd, Some(&mut sink))?.into_result()?;
        let first_complete = commits.len() < n;
        let sorted = topo_sort(commits);
        let graph = Graph::build(&sorted, &refs.kinds(), refs.head.as_deref());
        {
            let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
            d.meta.extend(meta);
            d.refs = refs.clone();
            self.set_graph(&mut d, graph);
            d.graph_shown = true;
            d.stage = "first";
            d.first_ms = Some(started.elapsed().as_millis() as u64);
        }
        self.bump();
        if first_complete && !self.repo.place.is_remote() && self.tags_only_reachable(&refs) {
            let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
            d.complete = true;
            d.stage = "done";
            d.full_ms = d.first_ms;
            return Ok(());
        }
        // 2. Every commit, hash, parents and time only.
        let mut all = Vec::new();
        let mut hp = HashParser::default();
        let mut sink = |b: &[u8]| {
            hp.feed(b, |c| all.push(c));
            !cancel.is_cancelled()
        };
        let cmd = Git::read(["log", "--date-order", "--format=%H%x00%P%x00%ct"]).args(start_revs(&refs, true)).arg("--").cancel(Some(cancel.clone())).timeout(crate::exec::LONG_TIMEOUT);
        crate::exec::run_streaming(&self.repo.place, cmd, Some(&mut sink))?.into_result()?;
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled);
        }
        let graph = Graph::build(&all, &refs.kinds(), refs.head.as_deref());
        let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
        self.set_graph(&mut d, graph);
        d.complete = true;
        d.stage = "done";
        d.full_ms = Some(started.elapsed().as_millis() as u64);
        Ok(())
    }

    /// Whether the first block already holds everything tags point at.
    fn tags_only_reachable(&self, refs: &Refs) -> bool {
        let d = self.data.read().unwrap_or_else(|e| e.into_inner());
        refs.by_commit.keys().all(|h| d.graph.row_of(h).is_some())
    }

    /// Reads the recent commits and joins them onto the loaded graph
    /// (IntelliJ's `VcsLogJoiner`). False when it can't be done.
    fn try_join(self: &Arc<Self>, cancel: &Cancel) -> GitResult<bool> {
        let old = self.data.read().unwrap_or_else(|e| e.into_inner()).graph.clone();
        if old.is_empty() || !self.data.read().unwrap_or_else(|e| e.into_inner()).complete {
            return Ok(false);
        }
        let refs = read_refs(&self.repo)?;
        let mut recent = Vec::new();
        let mut meta = HashMap::new();
        let mut p = MetaParser::default();
        let out = self.repo.run(Git::read(["log", "--encoding=UTF-8", META_FORMAT, &format!("--max-count={RECENT}")]).args(start_revs(&refs, true)).arg("--").cancel(Some(cancel.clone())))?.into_result()?;
        p.feed(&out, |c, m| {
            meta.insert(c.hash.clone(), Arc::new(m));
            recent.push(c);
        });
        let fresh: Vec<RawCommit> = recent.into_iter().filter(|c| old.row_of(&c.hash).is_none()).collect();
        let fresh_set: HashSet<&[u8]> = fresh.iter().map(|c| c.hash.as_slice()).collect();
        // Every new commit's parents are known.
        if fresh.iter().any(|c| c.parents.iter().any(|p| !fresh_set.contains(p.as_slice()) && old.row_of(p).is_none())) {
            return Ok(false);
        }
        // Every ref points into the joined graph.
        let targets: HashSet<&[u8]> = refs.by_commit.keys().map(Vec::as_slice).chain(refs.head.as_deref()).collect();
        if targets.iter().any(|t| !fresh_set.contains(t) && old.row_of(t).is_none()) {
            return Ok(false);
        }
        // Every old head is still a ref, or now has a new child: nothing was dropped.
        let new_parents: HashSet<&[u8]> = fresh.iter().flat_map(|c| c.parents.iter().map(Vec::as_slice)).collect();
        for r in 0..old.len() as u32 {
            if old.children_of(r).is_empty() {
                let h = old.hash(r);
                if !targets.contains(h) && !new_parents.contains(h) {
                    return Ok(false);
                }
            }
        }
        let mut rows = topo_sort(fresh);
        rows.reserve(old.len());
        // The old rows keep their order; their parents come from the old graph.
        for r in 0..old.len() as u32 {
            let parents = old.parents_of(r).map(|p| if p == crate::graph::NOT_LOADED { Vec::new() } else { old.hash(p).to_vec() }).filter(|h| !h.is_empty()).collect();
            rows.push(RawCommit { hash: old.hash(r).to_vec(), parents, time: old.times[r as usize] });
        }
        let graph = Graph::build(&rows, &refs.kinds(), refs.head.as_deref());
        let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
        d.meta.extend(meta);
        d.refs = refs;
        self.set_graph(&mut d, graph);
        d.stage = "done";
        Ok(true)
    }

    fn load_filtered(self: &Arc<Self>, cancel: &Cancel) -> GitResult<()> {
        let started = Instant::now();
        let refs = read_refs(&self.repo)?;
        let keeps = self.filter.keeps_graph();
        for (i, &stage) in STAGES.iter().enumerate() {
            let mut commits = Vec::new();
            let mut meta = HashMap::new();
            let mut p = MetaParser::default();
            let mut sink = |b: &[u8]| {
                p.feed(b, |c, m| {
                    meta.insert(c.hash.clone(), Arc::new(m));
                    commits.push(c);
                });
                !cancel.is_cancelled()
            };
            let mut cmd = Git::read(["log", "--encoding=UTF-8", META_FORMAT]).cancel(Some(cancel.clone())).timeout(crate::exec::LONG_TIMEOUT);
            if stage != usize::MAX {
                cmd = cmd.arg(format!("--max-count={stage}"));
            }
            let mut args = self.filter.args();
            if !args.contains(&"--".to_string()) {
                args.push("--".into());
            }
            cmd = cmd.args(args);
            let out = crate::exec::run_streaming(&self.repo.place, cmd, Some(&mut sink))?;
            if !out.ok() {
                return Err(GitError::Failed { code: out.code, message: out.stderr_text().trim().to_owned() });
            }
            let done = commits.len() < stage;
            if !keeps {
                // A list: every row its own, no edges.
                for c in &mut commits {
                    c.parents.clear();
                }
            }
            let graph = Graph::build(&commits, &refs.kinds(), refs.head.as_deref());
            {
                let mut d = self.data.write().unwrap_or_else(|e| e.into_inner());
                d.meta.extend(meta);
                d.refs = refs.clone();
                self.set_graph(&mut d, graph);
                d.graph_shown = keeps;
                d.stage = if done { "done" } else { ["5", "100", "2000", "all"][i] };
                d.complete = done;
                if i == 0 {
                    d.first_ms = Some(started.elapsed().as_millis() as u64);
                }
                if done {
                    d.full_ms = Some(started.elapsed().as_millis() as u64);
                }
            }
            self.bump();
            if done {
                break;
            }
        }
        Ok(())
    }
}

/// HEAD (when there is one), branches, remotes, and on the full read, tags.
fn start_revs(refs: &Refs, tags: bool) -> Vec<String> {
    let mut a = Vec::new();
    if refs.head.is_some() {
        a.push("HEAD".to_string());
    }
    a.push("--branches".into());
    if crate::settings::get("logShowRemotes").as_bool().unwrap_or(true) {
        a.push("--remotes".into());
    }
    if tags {
        a.push("--tags".into());
    }
    a
}

pub fn kind_name(k: RefKind) -> &'static str {
    match k {
        RefKind::Current => "head",
        RefKind::Main | RefKind::Local => "local",
        RefKind::Remote => "remote",
        RefKind::Tag => "tag",
    }
}

enum Either<A, B> {
    Left(A),
    Right(B),
}

/// The first of two futures to finish.
fn futures_or<A, B>(a: A, b: B) -> impl std::future::Future<Output = Either<A::Output, B::Output>>
where
    A: std::future::Future,
    B: std::future::Future,
{
    let mut a = Box::pin(a);
    let mut b = Box::pin(b);
    std::future::poll_fn(move |cx| {
        if let std::task::Poll::Ready(v) = a.as_mut().poll(cx) {
            return std::task::Poll::Ready(Either::Left(v));
        }
        if let std::task::Poll::Ready(v) = b.as_mut().poll(cx) {
            return std::task::Poll::Ready(Either::Right(v));
        }
        std::task::Poll::Pending
    })
}

fn sessions() -> &'static Mutex<HashMap<u64, Arc<LogSession>>> {
    static S: OnceLock<Mutex<HashMap<u64, Arc<LogSession>>>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Opens a log of `repo` with `filter` and starts loading it.
pub fn open(repo: Arc<Repo>, filter: Filter) -> Arc<LogSession> {
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let s = Arc::new(LogSession {
        id,
        repo,
        filter,
        data: RwLock::default(),
        generation: AtomicU64::new(1),
        waiters: Mutex::default(),
        rows: Mutex::new(Lru::new(400)),
        loading_meta: Mutex::default(),
        cancel: Mutex::default(),
    });
    sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(id, s.clone());
    s.start(false);
    s
}

pub fn get(id: u64) -> GitResult<Arc<LogSession>> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| GitError::Other(format!("no log {id}")))
}

/// For the graph component, mid-paint (the registry lock is only held to
/// insert or remove a session).
pub fn try_get(id: u64) -> Option<Arc<LogSession>> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
}

pub fn close(id: u64) {
    if let Some(s) = sessions().lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        s.cancel.lock().unwrap_or_else(|e| e.into_inner()).cancel();
    }
}

/// One commit's details: message, people, signature and changed files.
pub fn details(repo: &Repo, hash: &str) -> GitResult<Value> {
    static CACHE: OnceLock<Mutex<Lru<String, Value>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Lru::new(1000)));
    let key = format!("{}\0{hash}", repo.id);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(v);
    }
    let fmt = "--format=%x01%x01%H%x02%x02%P%x02%x02%an%x02%x02%ae%x02%x02%at%x02%x02%cn%x02%x02%ce%x02%x02%ct%x02%x02%G?%x02%x02%GS%x02%x02%B%x03%x03";
    let out = repo
        .run(Git::read(["log", "--no-walk=unsorted", "--stdin", "--encoding=UTF-8", "--name-status", "-M", "-z", "--diff-merges=first-parent", fmt]).stdin(format!("{hash}\n").into_bytes()))?
        .into_result()?;
    let text = String::from_utf8_lossy(&out).into_owned();
    let start = text.find(RECORD_START).ok_or_else(|| GitError::Other(format!("no commit {hash}")))?;
    let end = text[start..].find(RECORD_END).map(|e| e + start).ok_or_else(|| GitError::Other("unreadable commit".into()))?;
    let f: Vec<&str> = text[start + 2..end].split(FIELD).collect();
    if f.len() < 11 {
        return Err(GitError::Other("unreadable commit".into()));
    }
    let files = parse_name_status(&text[end + 2..]);
    let message = f[10].trim_end().to_owned();
    let (subject, body) = message.split_once('\n').map(|(s, b)| (s.to_owned(), b.trim_start_matches('\n').to_owned())).unwrap_or((message.clone(), String::new()));
    let v = json!({
        "hash": f[0],
        "parents": f[1].split_whitespace().collect::<Vec<_>>(),
        "author": f[2], "email": f[3], "authorTime": f[4].parse::<i64>().unwrap_or(0),
        "committer": f[5], "committerEmail": f[6], "commitTime": f[7].parse::<i64>().unwrap_or(0),
        "signature": f[8], "signer": f[9],
        "subject": subject, "body": body,
        "files": files,
    });
    cache.lock().unwrap_or_else(|e| e.into_inner()).put(key, v.clone());
    Ok(v)
}

/// `--name-status -z` entries: `M\0path\0`, `R100\0old\0new\0`.
pub fn parse_name_status(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut it = text.split('\0').map(|s| s.trim_start_matches('\n'));
    while let Some(status) = it.next() {
        if status.is_empty() {
            continue;
        }
        let kind = &status[..1];
        if !"ACDMRTUX".contains(kind) {
            continue;
        }
        if kind == "R" || kind == "C" {
            let (Some(old), Some(new)) = (it.next(), it.next()) else { break };
            out.push(json!({ "kind": kind, "path": new, "orig": old }));
        } else if let Some(path) = it.next() {
            out.push(json!({ "kind": kind, "path": path }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::scratch;

    #[test]
    fn records_parse_across_chunk_boundaries() {
        let rec = format!("{RECORD_START}abcd{FIELD}0102 0304{FIELD}100{FIELD}90{FIELD}A U Thor{FIELD}a@x{FIELD}fix: the {{thing}}{RECORD_END}\n");
        let twice = rec.repeat(2);
        let mut p = MetaParser::default();
        let mut got = Vec::new();
        for chunk in twice.as_bytes().chunks(7) {
            p.feed(chunk, |c, m| got.push((c, m)));
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0.hash, [0xab, 0xcd]);
        assert_eq!(got[0].0.parents, [vec![1, 2], vec![3, 4]]);
        assert_eq!(got[0].1.subject, "fix: the {thing}");
        assert_eq!(got[0].1.time, 90);
        let mut hp = HashParser::default();
        let mut hashes = Vec::new();
        for chunk in b"ab\0cd ef\0100\n12\0\05\n".chunks(3) {
            hp.feed(chunk, |c| hashes.push(c));
        }
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[1].parents.len(), 0);
    }

    #[test]
    fn filters_become_git_arguments() {
        let f = Filter::from_json(&json!({ "users": ["yuzu"], "text": "fix", "paths": ["src"], "noMerges": true }));
        let a = f.args();
        assert!(a.contains(&"--author=yuzu".to_string()) && a.contains(&"--grep=fix".to_string()) && a.contains(&"--fixed-strings".to_string()));
        assert!(a.contains(&"--regexp-ignore-case".to_string()) && a.contains(&"--no-merges".to_string()));
        assert_eq!(&a[a.len() - 2..], ["--", "src"]);
        assert!(!f.keeps_graph());
        let b = Filter::from_json(&json!({ "branches": ["main"], "paths": ["a.txt"], "follow": true }));
        assert!(b.keeps_graph());
        assert!(b.args().contains(&"--follow".to_string()) && b.args().contains(&"--parents".to_string()));
        assert!(Filter::from_json(&json!({})).is_empty());
    }

    #[test]
    fn name_status_reads_renames() {
        let v = parse_name_status("\nM\0a.txt\0R087\0old.rs\0new.rs\0A\0b c.txt\0");
        assert_eq!(v.len(), 3);
        assert_eq!(v[1]["orig"], "old.rs");
        assert_eq!(v[2]["path"], "b c.txt");
    }

    fn wait_loaded(s: &Arc<LogSession>) {
        for _ in 0..500 {
            if s.data.read().is_ok_and(|d| !d.loading) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("log never finished");
    }

    #[test]
    fn a_log_loads_then_joins_new_commits_on_refresh() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        s.commit("a.txt", "2\n", "second");
        s.git(&["switch", "-q", "-c", "feature"]);
        s.commit("b.txt", "b\n", "on feature");
        s.git(&["switch", "-q", "main"]);
        s.git(&["tag", "v1"]);
        let log = open(s.repo.clone(), Filter::default());
        wait_loaded(&log);
        let info = log.info();
        assert_eq!(info["rows"], 3, "{info}");
        assert_eq!(info["complete"], true);
        let rows = log.rows(0, 10);
        assert_eq!(rows[0]["subject"], "on feature");
        assert!(rows[1]["refs"].as_array().unwrap().iter().any(|r| r["name"] == "main" && r["kind"] == "head"));
        assert!(rows[1]["refs"].as_array().unwrap().iter().any(|r| r["name"] == "v1" && r["kind"] == "tag"));
        assert!(log.row_print(0).is_some());
        // A new commit joins on top without a full reload.
        s.commit("a.txt", "3\n", "third");
        s.repo.invalidate_refs();
        log.start(true);
        wait_loaded(&log);
        let rows = log.rows(0, 10);
        assert_eq!(log.info()["rows"], 4);
        assert_eq!(rows[0]["subject"], "third");
        assert_eq!(log.find("v1"), log.find(&rows[2]["hash"].as_str().unwrap()[..7]));
        // A reset drops a commit: the join refuses and everything is read again.
        s.git(&["reset", "-q", "--hard", "HEAD~1"]);
        log.start(true);
        wait_loaded(&log);
        assert_eq!(log.info()["rows"], 3);
        // Details.
        let d = details(&s.repo, rows[0]["hash"].as_str().unwrap()).unwrap();
        assert_eq!(d["subject"], "third");
        assert_eq!(d["files"][0]["path"], "a.txt");
        close(log.id);
    }

    #[test]
    fn filtered_logs_read_in_stages() {
        let s = scratch();
        s.commit("a.txt", "1\n", "fix: one");
        s.commit("b.txt", "1\n", "feat: two");
        s.commit("a.txt", "2\n", "fix: three");
        let log = open(s.repo.clone(), Filter::from_json(&json!({ "text": "FIX" })));
        wait_loaded(&log);
        assert_eq!(log.info()["rows"], 2);
        assert_eq!(log.info()["graph"], false);
        let path = open(s.repo.clone(), Filter::from_json(&json!({ "paths": ["a.txt"] })));
        wait_loaded(&path);
        assert_eq!(path.info()["rows"], 2);
        assert_eq!(path.info()["graph"], true);
        // The rewritten parents connect the two a.txt commits.
        assert_eq!(path.data.read().unwrap().graph.parents_of(0).collect::<Vec<_>>(), [1]);
        close(log.id);
        close(path.id);
    }

    /// Timings on a real repository (YUZORA_GIT_PERF_REPO, read only):
    /// `cargo test -p yuzora-git --lib -- --ignored --nocapture perf`.
    #[test]
    #[ignore = "needs a large repository: YUZORA_GIT_PERF_REPO"]
    fn perf_on_a_large_repository() {
        use std::time::Instant;
        let Ok(path) = std::env::var("YUZORA_GIT_PERF_REPO") else { return };
        let repo = crate::repo::open(&path, None).unwrap();
        let t = Instant::now();
        let log = open(repo.clone(), Filter::default());
        // The first screen: rows and their metadata.
        let first = loop {
            if log.info()["rows"].as_u64().unwrap_or(0) > 0 && log.rows(0, 40).as_array().unwrap().iter().all(|r| !r["subject"].is_null()) {
                break t.elapsed();
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        for _ in 0..30_000 {
            if log.info()["complete"] == true {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let info = log.info();
        println!("log: {} commits, first screen {first:?} (firstMs {}), whole graph {:?} (fullMs {}), {} lanes", info["rows"], info["firstMs"], t.elapsed(), info["fullMs"], info["width"]);
        // Scrolling: one frame is ~40 rows of print elements (cold cache).
        let rows = info["rows"].as_u64().unwrap() as u32;
        let mut worst = std::time::Duration::ZERO;
        let mut total = std::time::Duration::ZERO;
        let frames = 200;
        for f in 0..frames {
            let start = (f as u32 * (rows / frames as u32)).min(rows.saturating_sub(40));
            let ft = Instant::now();
            for r in start..start + 40 {
                assert!(log.row_print(r).is_some());
            }
            let e = ft.elapsed();
            worst = worst.max(e);
            total += e;
        }
        println!("graph: 40-row frame avg {:?}, worst {:?} over {frames} frames spread across the history", total / frames, worst);
        // Deep rows: metadata fetched on demand.
        let deep = rows / 2;
        let t = Instant::now();
        let mut since = log.generation();
        log.rows(deep as usize, deep as usize + 40);
        while log.rows(deep as usize, deep as usize + 40).as_array().unwrap().iter().any(|r| r["subject"].is_null()) {
            since = crate::repo::tests::futures_lite_block(log.wait(since));
        }
        println!("metadata: 40 rows at {deep} in {:?}", t.elapsed());
        // Status of the work tree.
        let t = Instant::now();
        crate::status::refresh(&repo).unwrap();
        println!("status: full refresh {:?}", t.elapsed());
        close(log.id);
    }
}
