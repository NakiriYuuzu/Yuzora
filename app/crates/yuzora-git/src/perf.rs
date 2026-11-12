//! What every `yuzora:git` call costs on a real repository, measured where a script calls it: from the call to its
//! answer, with the git processes it started in between (`exec::trace`).
//!
//! `scripts/git-bench/make-perf-repo.sh <source> /tmp/<dest>` makes the clone this runs on (dirty, with branches,
//! stashes, a conflict to merge into and a remote that is a folder beside it). Writes are timed too, so the harness
//! refuses any repository that script did not make. `scripts/git-bench/run-ops-bench.sh` runs it:
//!
//! - `perf_every_operation` (ignored): YUZORA_GIT_PERF_REPO=<dest>/work, YUZORA_GIT_PERF_RUNS (5),
//!   YUZORA_GIT_PERF_OUT=<file> for the samples (one line each: row, ms, git processes, ms a git was running, ok);
//! - `perf_table` (ignored): YUZORA_GIT_PERF_SAMPLES=`<label>=<file>,…` prints the samples side by side.
//!
//! A row is a call, or what a pane does for one action (`→`). Each run does its own setup, times the call, checks
//! with plain git that the work was done, and puts the repository back. A call that failed, or whose work is not
//! there, is an error and not a timing. Every function of the module has to be timed or the test fails.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yuzora_core::host::{from_host, to_host};
use yuzora_core::{HostArguments, HostModule};

use crate::exec::trace;
use crate::repo;

/// One timed call: its milliseconds, the git processes it started, and how long at least one of them was running.
struct Sample {
    ms: f64,
    git: usize,
    span: f64,
}

#[derive(Default)]
struct Row {
    samples: Vec<Sample>,
    errors: Vec<String>,
    /// The git calls of the last run, for the trace file.
    calls: Vec<String>,
}

struct Bench {
    m: HostModule,
    root: String,
    origin: String,
    worktree: String,
    id: String,
    branch: String,
    files: Vec<String>,
    dir: String,
    /// `git status --porcelain` as the script left it: every run has to end here.
    clean: String,
    rows: RefCell<Vec<(String, Row)>>,
    timed: RefCell<BTreeSet<String>>,
    alone: bool,
}

/// Drives a host future on this thread, woken as the app's executor would be.
fn block<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// How long at least one of the calls was running.
fn span(calls: &[trace::Call]) -> f64 {
    let mut parts: Vec<(Instant, Instant)> = calls.iter().map(|c| (c.began, c.began + c.took)).collect();
    parts.sort();
    let mut total = Duration::ZERO;
    let mut open: Option<(Instant, Instant)> = None;
    for (s, e) in parts {
        match open {
            Some((os, oe)) if s <= oe => open = Some((os, oe.max(e))),
            Some((os, oe)) => {
                total += oe - os;
                open = Some((s, e));
            }
            None => open = Some((s, e)),
        }
    }
    ms(total + open.map_or(Duration::ZERO, |(s, e)| e - s))
}

impl Bench {
    fn new(root: &str) -> Bench {
        let real = std::fs::canonicalize(root).expect("YUZORA_GIT_PERF_REPO").to_string_lossy().into_owned();
        // Writes are timed: only ever on a clone the script made, under /tmp.
        assert!(real.starts_with("/private/tmp/") || real.starts_with("/tmp/"), "{real} is not under /tmp");
        assert!(Path::new(&real).join(".git/yuzora-perf-clone").exists(), "{real} was not made by make-perf-repo.sh");
        let read = |name: &str| std::fs::read_to_string(Path::new(&real).join(".git").join(name)).unwrap();
        let above = Path::new(&real).parent().unwrap();
        let mut b = Bench {
            m: crate::host_module(),
            origin: above.join("origin.git").to_string_lossy().into_owned(),
            worktree: above.join("worktree").to_string_lossy().into_owned(),
            id: repo::repo_id(None, &real),
            files: read("yuzora-perf-files").lines().map(str::to_owned).collect(),
            dir: read("yuzora-perf-dir").trim().to_owned(),
            root: real.clone(),
            branch: String::new(),
            clean: String::new(),
            rows: RefCell::default(),
            timed: RefCell::default(),
            alone: std::env::var_os("YUZORA_GIT_PERF_ALONE").is_some(),
        };
        assert!(b.git(&["remote", "get-url", "origin"]).trim() == b.origin, "origin is not the folder beside the clone");
        b.branch = b.git(&["symbolic-ref", "--short", "HEAD"]).trim().to_owned();
        b.clean = b.git(&["status", "--porcelain"]);
        assert_eq!(b.git(&["stash", "list"]).lines().count(), 3, "the clone is not as the script left it");
        b
    }

    /// git as a terminal would run it, for the setup and for checking what a call did.
    fn git_in(&self, dir: &str, args: &[&str]) -> (bool, String) {
        // No index written just to refresh it: only what the harness means to change is changed.
        let out = yuzora_core::spawn::no_window(&mut std::process::Command::new("git")).args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0").output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    }

    fn git(&self, args: &[&str]) -> String {
        let (ok, text) = self.git_in(&self.root, args);
        assert!(ok, "git {args:?}: {text}");
        text
    }

    fn try_git(&self, args: &[&str]) -> bool {
        self.git_in(&self.root, args).0
    }

    fn rev(&self, name: &str) -> String {
        self.git(&["rev-parse", name]).trim().to_owned()
    }

    fn file(&self, n: usize) -> &str {
        &self.files[n]
    }

    fn repo(&self) -> std::sync::Arc<repo::Repo> {
        repo::get(&self.id).unwrap()
    }

    /// One call, as a script makes it.
    fn call(&self, f: &str, args: Value) -> Result<Value, String> {
        self.timed.borrow_mut().insert(f.to_owned());
        let args = HostArguments::new(args.as_array().unwrap().iter().map(to_host));
        let answer = if self.m.is_async(f) { self.m.begin(f, &args).and_then(block) } else { self.m.call(f, &args) };
        answer.map(|v| from_host(&v)).map_err(|e| e.message().to_owned())
    }

    /// A call on the repo: its id goes first.
    fn on(&self, f: &str, rest: Value) -> Result<Value, String> {
        let mut args = vec![json!(self.id)];
        args.extend(rest.as_array().unwrap().iter().cloned());
        self.call(f, Value::Array(args))
    }

    /// Times `work` as one sample of `row`; an error is counted and not timed.
    fn time(&self, row: &str, work: impl FnOnce(&Bench) -> Result<Value, String>) -> Value {
        trace::start();
        let began = Instant::now();
        let answer = work(self);
        let took = began.elapsed();
        let calls = trace::take();
        let mut rows = self.rows.borrow_mut();
        let at = rows.iter().position(|(name, _)| name == row).unwrap_or_else(|| {
            rows.push((row.to_owned(), Row::default()));
            rows.len() - 1
        });
        let r = &mut rows[at].1;
        match answer {
            Ok(v) => {
                r.samples.push(Sample { ms: ms(took), git: calls.len(), span: span(&calls) });
                r.calls = calls
                    .iter()
                    .map(|c| {
                        let alone = if self.alone && c.read { format!("\talone {:.1}", self.alone(c)) } else { String::new() };
                        // Past the options every call carries.
                        let own: Vec<&str> = c.args.iter().map(String::as_str).skip_while(|a| *a == "-c" || a.contains('=')).collect();
                        format!("+{:.1}\t{:.1} ms{alone}\t{}\tgit {}", ms(c.began - began), ms(c.took), if c.read { "read" } else { "write" }, own.join(" ").chars().take(150).collect::<String>())
                    })
                    .collect();
                v
            }
            Err(e) => {
                r.errors.push(e);
                Value::Null
            }
        }
    }

    /// The same git command by itself, outside the engine: the fastest of three.
    fn alone(&self, c: &trace::Call) -> f64 {
        (0..3)
            .map(|_| {
                let began = Instant::now();
                let _ = yuzora_core::spawn::no_window(&mut std::process::Command::new(&c.program)).args(&c.args).envs(c.env.iter().cloned()).current_dir(&self.root).stdin(std::process::Stdio::null()).output();
                ms(began.elapsed())
            })
            .fold(f64::MAX, f64::min)
    }

    fn one(&self, row: &str, f: &str, rest: Value) -> Value {
        self.time(row, |b| b.on(f, rest))
    }

    /// The last sample of `row` only counts if the work is there.
    fn check(&self, row: &str, done: bool, what: &str) {
        if done {
            return;
        }
        let mut rows = self.rows.borrow_mut();
        let r = &mut rows.iter_mut().find(|(name, _)| name == row).unwrap_or_else(|| panic!("no row {row}")).1;
        if r.samples.pop().is_some() {
            r.errors.push(format!("not done: {what}"));
        }
    }

    /// Between two things a person does: the watcher has told of the last change and the sidebar has read the status
    /// after it, then nothing happens for a while. "A while" is past the 3 s in which the folder's own programs are
    /// read again by every call (`git_trust::RACY`: an answer read that soon after the index, HEAD or config changed
    /// is not kept), so the next call is the one that reads them and keeps the answer, as after any pause.
    fn settle(&self) {
        let repo = self.repo();
        loop {
            let g = repo.generation();
            std::thread::sleep(crate::watch::DEBOUNCE + Duration::from_millis(150));
            if repo.generation() == g {
                break;
            }
        }
        repo.invalidate_refs();
        self.on("status", json!([{}])).unwrap();
        let pause = Duration::from_millis(3200);
        let age = |name: &str| std::fs::metadata(Path::new(&repo.git_dir).join(name)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).unwrap_or(pause);
        let youngest = ["index", "HEAD", "config"].into_iter().map(age).min().unwrap_or(pause);
        std::thread::sleep(pause.saturating_sub(youngest));
    }

    /// The panes that only read are timed with the folder's programs already read (the sidebar did, when it showed
    /// the repo); `reads` times what the first call after a pause adds.
    fn read_programs(&self) {
        self.on("tree", json!(["HEAD", ""])).unwrap();
    }

    /// The index as the script left it, after a call that changed it.
    fn restage(&self) {
        self.git(&["reset", "-q"]);
        let renamed = format!("{}.yz-perf-renamed", self.file(51));
        let mut args = vec!["add", "-A", "--", self.file(50), self.file(51), &renamed];
        args.extend((40..50).map(|n| self.file(n)));
        self.git(&args);
    }

    fn close(&self) {
        repo::close(&self.id);
    }

    fn open(&self) {
        self.call("open", json!([self.root])).unwrap();
    }
}

fn status_counts(st: &Value) -> (u64, u64, u64) {
    let n = |k: &str| st["counts"][k].as_u64().unwrap_or(0);
    (n("staged"), n("unstaged"), n("untracked"))
}

/// Opening a place, and what the sidebar reads when it does.
fn attach(b: &Bench) {
    b.close();
    let r = b.time("open · closed repo (the watch starts)", |b| b.call("open", json!([b.root])));
    b.check("open · closed repo (the watch starts)", r["root"] == json!(b.root), "the root");
    let st = b.one("status · first read", "status", json!([{}]));
    b.check("status · first read", status_counts(&st) == (12, 40, 27), "12 staged, 40 unstaged, 27 untracked");
    let m = b.one("messages", "messages", json!([]));
    b.check("messages", m["last"].as_str().is_some_and(|l| l.starts_with("perf: one commit ahead")), "the last message");
    let r = b.time("open · open repo", |b| b.call("open", json!([b.root])));
    b.check("open · open repo", r["id"] == json!(b.id), "the id");
    let st = b.one("status · nothing changed", "status", json!([{}]));
    b.check("status · nothing changed", status_counts(&st) == (12, 40, 27), "the counts");
    let st = b.one("status · forced", "status", json!([{ "force": true }]));
    b.check("status · forced", status_counts(&st) == (12, 40, 27), "the counts");
    b.close();
    b.time("attach → open, status, messages (closed repo)", |b| {
        b.call("open", json!([b.root]))?;
        b.on("status", json!([{}]))?;
        b.on("messages", json!([]))
    });
    b.time("attach → open, status, messages (seen repo)", |b| {
        b.call("open", json!([b.root]))?;
        b.on("status", json!([{}]))?;
        b.on("messages", json!([]))
    });
    let above = Path::new(&b.root).parent().unwrap().to_string_lossy().into_owned();
    let r = b.time("repos", |b| b.call("repos", json!([above, null, [b.root]])));
    b.check("repos", r["repos"].as_array().is_some_and(|l| l.iter().any(|x| x["root"] == json!(b.root))), "the clone is listed");
    b.settle();
}

/// An edit made outside Yuzora: when the sidebar hears of it, and the status it then reads.
fn outside_edit(b: &Bench) {
    let path = Path::new(&b.root).join(b.file(2));
    let before = std::fs::read(&path).unwrap();
    let since = b.repo().generation();
    let g = b.time("changed · after a file is edited outside (watcher)", |b| {
        std::fs::write(&path, [&before[..], b"// edited outside\n"].concat()).unwrap();
        b.on("changed", json!([since]))
    });
    b.check("changed · after a file is edited outside (watcher)", g.as_u64().is_some_and(|g| g > since), "a later generation");
    let st = b.one("status · after one file changed", "status", json!([{}]));
    b.check("status · after one file changed", status_counts(&st) == (12, 40, 27), "the counts");
    let since = b.repo().generation();
    let c = b.time("changes · after one of ours", |b| {
        b.repo().invalidate(crate::status::Scope::Paths(vec![b.file(2).to_owned()]));
        b.on("changes", json!([since]))
    });
    b.check("changes · after one of ours", c["paths"] == json!([b.file(2)]), "the path");
    std::fs::write(&path, before).unwrap();
    b.settle();
}

/// The lists and panes that only read.
fn reads(b: &Bench) {
    // The index written (a commit, a `git status` in a terminal), then a pause: the next call, whichever it is, reads
    // the folder's own programs before it runs. The rows after it are without that.
    let index = std::fs::File::options().append(true).open(Path::new(&b.repo().git_dir).join("index")).unwrap();
    index.set_modified(std::time::SystemTime::now()).unwrap();
    b.settle();
    let row = "tree · first call after the index was written (the folder's programs are read)";
    let t = b.one(row, "tree", json!(["HEAD", ""]));
    b.check(row, t.as_array().is_some_and(|l| !l.is_empty()), "entries");
    let checks = b.one("commitChecks", "commitChecks", json!([false]));
    b.check("commitChecks", checks.as_array().is_some_and(Vec::is_empty), "no warning");
    let br = b.one("branches", "branches", json!([]));
    b.check("branches", br["current"] == json!(b.branch) && br["local"].as_array().is_some_and(|l| l.len() == 4), "four local branches");
    let st = b.one("stashes", "stashes", json!([]));
    b.check("stashes", st.as_array().is_some_and(|l| l.len() == 3), "three stashes");
    let r = b.one("remotes", "remotes", json!([]));
    b.check("remotes", r[0]["url"] == json!(b.origin), "origin");
    let w = b.one("worktrees", "worktrees", json!([]));
    b.check("worktrees", w.as_array().is_some_and(|l| l.len() == 2), "two worktrees");
    let s = b.one("submodules", "submodules", json!([]));
    b.check("submodules", s.is_array(), "a list");
    let c = b.one("config", "config", json!([]));
    b.check("config", c["userName"] == json!("Yuzora Perf") && c["version"].as_str().is_some_and(|v| v.starts_with("git version")), "the identity and git's version");
    let p = b.one("pushPreview", "pushPreview", json!([]));
    b.check("pushPreview", p["commits"].as_array().is_some_and(|l| l.len() == 1) && p["remote"] == json!("origin"), "one commit to origin");
    let c = b.one("compare", "compare", json!(["yz-perf/feature", b.branch]));
    let only: usize = b.git(&["rev-list", "--count", &format!("yz-perf/feature..{}", b.branch)]).trim().parse().unwrap();
    b.check("compare", c["onlyA"].as_array().is_some_and(|l| l.len() == 1) && c["onlyB"].as_array().is_some_and(|l| l.len() == only.min(500)), "the commits of each side");
    let t = b.one("tree", "tree", json!(["HEAD", ""]));
    b.check("tree", t.as_array().is_some_and(|l| !l.is_empty()), "entries");
    let big = b.files[60..].iter().max_by_key(|f| std::fs::metadata(Path::new(&b.root).join(f)).map_or(0, |m| m.len())).unwrap().clone();
    let text = b.one("fileAt", "fileAt", json!(["HEAD", big]));
    b.check("fileAt", text.as_str().is_some_and(|t| t.len() > 1000), "the text");
    let lines = std::fs::read_to_string(Path::new(&b.root).join(&big)).unwrap().lines().count();
    let bl = b.one("blame · work tree file", "blame", json!([big, null, {}]));
    b.check("blame · work tree file", bl["lines"].as_array().is_some_and(|l| l.len() == lines), "every line");
    let bl = b.one("blame · at HEAD", "blame", json!([big, "HEAD", {}]));
    b.check("blame · at HEAD", bl["lines"].as_array().is_some_and(|l| l.len() == lines), "every line");
    let bl = b.one("blame · at HEAD again", "blame", json!([big, "HEAD", {}]));
    b.check("blame · at HEAD again", bl["lines"].as_array().is_some_and(|l| l.len() == lines), "every line");
    let todo = b.one("rebaseTodo", "rebaseTodo", json!(["HEAD~5"]));
    b.check("rebaseTodo", todo["commits"].as_array().is_some_and(|l| !l.is_empty()), "commits");
    let patch = b.one("patchCreate · one file", "patchCreate", json!([{ "paths": [b.file(0)] }]));
    b.check("patchCreate · one file", patch.as_str().is_some_and(|p| p.starts_with("diff --git")), "a patch");
    let out = Path::new(&b.root).parent().unwrap().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let saved = b.one("patchSave", "patchSave", json!([{ "paths": [b.file(0)] }, out.to_string_lossy(), "perf.patch"]));
    b.check("patchSave", saved.as_str().is_some_and(|p| std::fs::read_to_string(p).is_ok_and(|t| t.starts_with("diff --git"))), "the file");
    let ok = b.one("patchApply · check only", "patchApply", json!([patch, { "check": true, "reverse": true }]));
    b.check("patchApply · check only", ok["ok"] == json!(true), "it applies");
    let ok = b.one("patchApply · to the work tree", "patchApply", json!([patch, { "reverse": true }]));
    b.check("patchApply · to the work tree", ok["ok"] == json!(true) && b.git(&["diff", "--name-only", "--", b.file(0)]).is_empty(), "the change is gone");
    b.on("patchApply", json!([patch, {}])).unwrap();
    b.time("settings", |b| b.call("settings", json!([])));
    b.time("setSettings", |b| b.call("setSettings", json!([{ "signOff": false }])));
    b.time("checkPattern", |b| b.call("checkPattern", json!(["^(main|release/.*)$"])));
    b.time("version", |b| b.call("version", json!([])));
    b.time("cancel", |b| b.call("cancel", json!(["no such token"])));
    b.settle();
}

/// The diff pane: what opening a file's diff asks, and its other reads.
fn diffs(b: &Bench) {
    b.read_programs();
    for (row, spec) in [
        ("diff → diffOpen, diffView (unstaged file)", json!({ "path": b.file(0), "mode": "unstaged" })),
        ("diff → diffOpen, diffView (staged file)", json!({ "path": b.file(40), "mode": "staged" })),
        ("diff → diffOpen, diffView (a commit's file)", json!({ "path": b.file(56), "mode": "commit", "rev": "HEAD" })),
    ] {
        let v = b.time(row, |b| {
            let d = b.on("diffOpen", json!([spec]))?;
            b.call("diffView", json!([d["id"], { "layout": "side", "context": 3, "whitespace": "none" }]))
        });
        b.check(row, v["hunks"].as_array().is_some_and(|h| !h.is_empty()), "hunks");
    }
    let d = b.one("diffOpen · unstaged file", "diffOpen", json!([{ "path": b.file(0), "mode": "unstaged" }]));
    let id = d["id"].clone();
    let v = b.time("diffView", |b| b.call("diffView", json!([id, { "layout": "unified", "context": -1, "whitespace": "all" }])));
    b.check("diffView", v["rows"].as_array().is_some_and(|r| !r.is_empty()), "rows");
    let p = b.time("diffPatch", |b| b.call("diffPatch", json!([id, [0], "none"])));
    b.check("diffPatch", p.as_str().is_some_and(|p| p.contains("@@")), "a hunk");
    let t = b.time("diffText", |b| b.call("diffText", json!([id, [["r", 0, 5]]])));
    b.check("diffText", t.as_str().is_some_and(|t| t.split('\n').count() == 5), "five lines");

    // A hunk into the index, back out of it, and thrown away.
    let cached = |b: &Bench| b.git(&["diff", "--cached", "--name-only", "--", b.file(0)]);
    b.time("diffStage · one hunk", |b| b.call("diffStage", json!([id, [0]])));
    b.check("diffStage · one hunk", !cached(b).is_empty(), "the file is staged");
    let d = b.on("diffOpen", json!([{ "path": b.file(0), "mode": "staged" }])).unwrap();
    b.time("diffUnstage · one hunk", |b| b.call("diffUnstage", json!([d["id"], [0]])));
    b.check("diffUnstage · one hunk", cached(b).is_empty(), "the file is not staged");
    let path = Path::new(&b.root).join(b.file(0));
    let before = std::fs::read(&path).unwrap();
    let d = b.on("diffOpen", json!([{ "path": b.file(0), "mode": "unstaged" }])).unwrap();
    let hunks = b.call("diffView", json!([d["id"], {}])).unwrap()["hunks"].as_array().map_or(0, Vec::len);
    b.time("diffDiscard · one hunk", |b| b.call("diffDiscard", json!([d["id"], [0]])));
    let d = b.on("diffOpen", json!([{ "path": b.file(0), "mode": "unstaged" }])).unwrap();
    let left = b.call("diffView", json!([d["id"], {}])).unwrap()["hunks"].as_array().map_or(0, Vec::len);
    b.check("diffDiscard · one hunk", left + 1 == hunks, "one hunk fewer");
    std::fs::write(&path, before).unwrap();
    b.settle();
}

/// The log pane: the first screen, the whole graph, a far row, a commit.
fn log(b: &Bench) {
    b.read_programs();
    let head = b.rev("HEAD");
    let screen = |b: &Bench, id: &Value, from: u64| -> Result<Value, String> {
        loop {
            let info = b.call("logInfo", json!([id]))?;
            if let Some(e) = info["error"].as_str() {
                return Err(e.to_owned());
            }
            let rows = b.call("logRows", json!([id, from, from + 40]))?;
            if rows.as_array().is_some_and(|r| r.len() == 40 && r.iter().all(|x| x["subject"].is_string())) {
                return Ok(rows);
            }
            b.call("logWait", json!([id, info["generation"]]))?;
        }
    };
    let whole = |b: &Bench, id: &Value| -> Result<Value, String> {
        loop {
            let info = b.call("logInfo", json!([id]))?;
            if info["complete"] == json!(true) && info["loading"] == json!(false) {
                return Ok(info);
            }
            b.call("logWait", json!([id, info["generation"]]))?;
        }
    };
    let mut id = Value::Null;
    let rows = b.time("log → logOpen to the first 40 rows", |b| {
        id = b.call("logOpen", json!([b.id]))?;
        screen(b, &id, 0)
    });
    b.check("log → logOpen to the first 40 rows", rows[0]["hash"] == json!(head), "HEAD is the first row");
    let info = b.time("log · the whole graph (behind the first screen)", |b| whole(b, &id));
    let all = info["rows"].as_u64().unwrap_or(0);
    b.check("log · the whole graph (behind the first screen)", all > 50_000, "every commit");
    let rows = b.time("log → 40 rows far down", |b| screen(b, &id, all / 2));
    b.check("log → 40 rows far down", rows.as_array().is_some_and(|r| r.len() == 40), "40 rows");
    let far = b.rev("HEAD~3000");
    let at = b.time("logFind", |b| b.call("logFind", json!([id, &far[..12]])));
    b.check("logFind", at.as_u64().is_some_and(|r| r > 0), "the row");
    b.time("logInfo", |b| b.call("logInfo", json!([id])));
    b.time("logRows · 40 loaded rows", |b| b.call("logRows", json!([id, 0, 40])));
    let rows = b.time("log → logRefresh to the first 40 rows", |b| {
        b.call("logRefresh", json!([id]))?;
        screen(b, &id, 0)
    });
    b.check("log → logRefresh to the first 40 rows", rows[0]["hash"] == json!(head), "HEAD is the first row");
    whole(b, &id).unwrap();
    b.time("logClose", |b| b.call("logClose", json!([id])));
    let mut id = Value::Null;
    let rows = b.time("log → a file's history, first rows", |b| {
        id = b.call("logOpen", json!([b.id, { "paths": [b.file(56)], "follow": true }]))?;
        loop {
            let info = b.call("logInfo", json!([id]))?;
            let rows = b.call("logRows", json!([id, 0, 5]))?;
            if rows.as_array().is_some_and(|r| !r.is_empty() && r.iter().all(|x| x["subject"].is_string())) {
                return Ok(rows);
            }
            if info["loading"] == json!(false) {
                return Err("no rows".into());
            }
            b.call("logWait", json!([id, info["generation"]]))?;
        }
    });
    b.check("log → a file's history, first rows", rows[0]["hash"] == json!(head), "HEAD changed the file last");
    b.call("logClose", json!([id])).unwrap();
    // A commit never asked about before (the details are kept), then again.
    let n = b.rows.borrow().iter().find(|(name, _)| name == "commitDetails").map_or(0, |(_, r)| r.samples.len() + r.errors.len());
    let hash = b.rev(&format!("HEAD~{}", n + 1));
    let d = b.one("commitDetails", "commitDetails", json!([hash]));
    b.check("commitDetails", d["hash"] == json!(hash) && d["files"].as_array().is_some_and(|f| !f.is_empty()), "the commit and its files");
    b.one("commitDetails · again", "commitDetails", json!([hash]));
}

/// The sidebar's rows and its commit: each write, then the status the sidebar reads after it.
fn index_and_commit(b: &Bench) {
    let cached = |b: &Bench| b.git(&["diff", "--cached", "--name-only"]);
    let then_status = |b: &Bench, row: &str| b.one(&format!("{row} → status after it"), "status", json!([{}]));

    b.one("stage · one file", "stage", json!([[b.file(0)]]));
    b.check("stage · one file", cached(b).lines().any(|l| l == b.file(0)), "the file is staged");
    let st = then_status(b, "stage · one file");
    b.check("stage · one file → status after it", status_counts(&st) == (13, 39, 27), "13 staged, 39 unstaged");
    b.git(&["reset", "-q", "--", b.file(0)]);
    b.settle();

    b.one("unstage · one file", "unstage", json!([[b.file(40)]]));
    b.check("unstage · one file", !cached(b).lines().any(|l| l == b.file(40)), "the file is not staged");
    let st = then_status(b, "unstage · one file");
    b.check("unstage · one file → status after it", status_counts(&st) == (11, 41, 27), "11 staged, 41 unstaged");
    b.git(&["add", "--", b.file(40)]);
    b.settle();

    b.one("stage · everything", "stage", json!([[]]));
    b.check("stage · everything", b.git(&["status", "--porcelain"]).lines().all(|l| !l.starts_with('?') && l.as_bytes()[1] == b' '), "nothing is left unstaged");
    let st = then_status(b, "stage · everything");
    b.check("stage · everything → status after it", status_counts(&st) == (79, 0, 0), "79 staged");
    b.one("unstage · everything", "unstage", json!([[]]));
    b.check("unstage · everything", cached(b).is_empty(), "nothing is staged");
    let st = then_status(b, "unstage · everything");
    b.check("unstage · everything → status after it", status_counts(&st).0 == 0, "nothing staged");
    b.restage();
    b.settle();

    let path = Path::new(&b.root).join(b.file(1));
    let before = std::fs::read(&path).unwrap();
    b.one("discard · a changed file", "discard", json!([[b.file(1)]]));
    b.check("discard · a changed file", b.git(&["diff", "--name-only", "--", b.file(1)]).is_empty(), "the change is gone");
    let st = then_status(b, "discard · a changed file");
    b.check("discard · a changed file → status after it", status_counts(&st) == (12, 39, 27), "39 unstaged");
    std::fs::write(&path, before).unwrap();
    let new = format!("{}/yz-perf-top-1.txt", b.dir);
    let before = std::fs::read(Path::new(&b.root).join(&new)).unwrap();
    b.one("discard · an untracked file", "discard", json!([[new]]));
    b.check("discard · an untracked file", !Path::new(&b.root).join(&new).exists(), "the file is gone");
    std::fs::write(Path::new(&b.root).join(&new), before).unwrap();
    b.settle();

    // Commit as the sidebar does: the checks, the commit, then the status and the messages again.
    let old = b.rev("HEAD");
    let row = "commit → commitChecks, commit, status, messages";
    b.time(row, |b| {
        b.on("commitChecks", json!([false]))?;
        let out = b.on("commit", json!([{ "message": "perf: a commit from the harness" }]))?;
        b.on("status", json!([{}]))?;
        b.on("messages", json!([]))?;
        Ok(out)
    });
    b.check(row, b.git(&["log", "-1", "--format=%s"]).trim() == "perf: a commit from the harness" && b.rev("HEAD~1") == old, "the commit is on the branch");
    b.git(&["reset", "-q", "--soft", &old]);
    b.settle();
    let out = b.one("commit", "commit", json!([{ "message": "perf: a commit from the harness" }]));
    b.check("commit", out["hash"] == json!(b.rev("HEAD")) && b.rev("HEAD~1") == old, "the commit is on the branch");
    let st = then_status(b, "commit");
    b.check("commit → status after it", status_counts(&st) == (0, 40, 27), "nothing staged");
    let amended = b.one("commit · amend", "commit", json!([{ "message": "perf: amended", "amend": true }]));
    b.check("commit · amend", amended["hash"] == json!(b.rev("HEAD")) && b.rev("HEAD~1") == old, "the commit is replaced");
    let out = b.one("undoCommit", "undoCommit", json!([]));
    b.check("undoCommit", out["ok"] == json!(true) && b.rev("HEAD") == old, "HEAD is back");
    let st = then_status(b, "undoCommit");
    b.check("undoCommit → status after it", status_counts(&st) == (12, 40, 27), "the changes are staged again");
    let out = b.one("reset · mixed, to HEAD", "reset", json!(["HEAD", "mixed"]));
    b.check("reset · mixed, to HEAD", out["ok"] == json!(true) && cached(b).is_empty(), "nothing is staged");
    b.restage();
    b.settle();
}

/// Branches, tags, stashes, remotes and the calls that reach origin (a folder here: no network in the time).
fn refs_and_remote(b: &Bench) {
    let has = |b: &Bench, name: &str| b.try_git(&["show-ref", "--verify", "-q", name]);
    let then_status = |b: &Bench, row: &str| b.one(&format!("{row} → status after it"), "status", json!([{}]));

    b.one("branchCreate", "branchCreate", json!(["yz-perf/tmp", null, false]));
    b.check("branchCreate", has(b, "refs/heads/yz-perf/tmp"), "the branch");
    b.one("branchRename", "branchRename", json!(["yz-perf/tmp", "yz-perf/tmp2"]));
    b.check("branchRename", has(b, "refs/heads/yz-perf/tmp2") && !has(b, "refs/heads/yz-perf/tmp"), "the new name");
    b.one("branchTrack", "branchTrack", json!(["yz-perf/tmp2", format!("origin/{}", b.branch)]));
    b.check("branchTrack", b.git(&["config", "branch.yz-perf/tmp2.remote"]).trim() == "origin", "the upstream");
    b.one("branchDelete", "branchDelete", json!(["yz-perf/tmp2", { "force": true }]));
    b.check("branchDelete", !has(b, "refs/heads/yz-perf/tmp2"), "the branch is gone");

    let row = "new branch → branchCreate with checkout, status";
    b.time(row, |b| {
        b.on("branchCreate", json!(["yz-perf/new", null, true]))?;
        b.on("status", json!([{}]))
    });
    b.check(row, b.git(&["symbolic-ref", "--short", "HEAD"]).trim() == "yz-perf/new", "HEAD is on it");
    let out = b.one("checkout · a branch at the same commit", "checkout", json!([b.branch, {}]));
    b.check("checkout · a branch at the same commit", out["ok"] == json!(true) && b.git(&["symbolic-ref", "--short", "HEAD"]).trim() == b.branch, "HEAD is back");
    then_status(b, "checkout · a branch at the same commit");
    b.git(&["branch", "-q", "-D", "yz-perf/new"]);
    b.settle();

    // 51 commits away, with the changes in the work tree: git moves the files and keeps the changes.
    let out = b.one("checkout · a branch 51 commits away", "checkout", json!(["yz-perf/feature", {}]));
    b.check("checkout · a branch 51 commits away", out["ok"] == json!(true) && out["restored"] != json!(false) && b.git(&["symbolic-ref", "--short", "HEAD"]).trim() == "yz-perf/feature", &format!("HEAD is on it, the changes with it: {out}"));
    then_status(b, "checkout · a branch 51 commits away");
    let out = b.on("checkout", json!([b.branch, {}])).unwrap();
    assert_eq!((b.git(&["status", "--porcelain"]), b.git(&["stash", "list"]).lines().count()), (b.clean.clone(), 3), "the changes did not survive the checkout: {out}");
    b.settle();

    b.one("tagCreate", "tagCreate", json!(["yz-perf-t", null, null]));
    b.check("tagCreate", has(b, "refs/tags/yz-perf-t"), "the tag");
    let in_origin = |b: &Bench, name: &str| b.git_in(&b.origin, &["show-ref", "--verify", "-q", name]).0;
    b.one("tagPush", "tagPush", json!(["origin", "yz-perf-t"]));
    b.check("tagPush", in_origin(b, "refs/tags/yz-perf-t"), "the tag is in origin");
    b.one("tagDelete · in origin", "tagDelete", json!(["yz-perf-t", "origin"]));
    b.check("tagDelete · in origin", !in_origin(b, "refs/tags/yz-perf-t"), "the tag is gone from origin");
    b.one("tagDelete", "tagDelete", json!(["yz-perf-t", null]));
    b.check("tagDelete", !has(b, "refs/tags/yz-perf-t"), "the tag is gone");

    let out = b.one("stashPush · everything, untracked too", "stashPush", json!([{ "message": "perf: from the harness", "untracked": true }]));
    b.check("stashPush · everything, untracked too", out["ok"] == json!(true) && b.git(&["status", "--porcelain"]).is_empty() && b.git(&["stash", "list"]).lines().count() == 4, "a clean tree and a fourth stash");
    then_status(b, "stashPush · everything, untracked too");
    let out = b.one("stashAction · pop", "stashAction", json!(["pop", "stash@{0}", { "index": true }]));
    b.check("stashAction · pop", out["ok"] == json!(true) && b.git(&["status", "--porcelain"]) == b.clean, "the changes are back");
    then_status(b, "stashAction · pop");

    b.one("remoteEdit · add", "remoteEdit", json!(["add", "yz-perf-r", b.origin]));
    b.check("remoteEdit · add", b.git(&["remote"]).lines().any(|l| l == "yz-perf-r"), "the remote");
    b.one("remoteEdit · remove", "remoteEdit", json!(["remove", "yz-perf-r", null]));
    b.check("remoteEdit · remove", !b.git(&["remote"]).lines().any(|l| l == "yz-perf-r"), "the remote is gone");
    // Never the person's own config: `false` is this repo's.
    b.one("setConfig", "setConfig", json!(["user.name", "Yuzora Perf", false]));
    b.check("setConfig", b.git(&["config", "--local", "user.name"]).trim() == "Yuzora Perf", "the value");
    let locked = |b: &Bench| b.git(&["worktree", "list", "--porcelain"]).lines().any(|l| l.starts_with("locked"));
    b.one("worktreeAction · lock", "worktreeAction", json!(["lock", b.worktree, false]));
    b.check("worktreeAction · lock", locked(b), "the worktree is locked");
    b.one("worktreeAction · unlock", "worktreeAction", json!(["unlock", b.worktree, false]));
    b.check("worktreeAction · unlock", !locked(b), "the worktree is unlocked");
    let out = b.one("submoduleUpdate · no submodule", "submoduleUpdate", json!([{}]));
    b.check("submoduleUpdate · no submodule", out["ok"] == json!(true), "git's answer");

    let out = b.one("fetch · nothing new", "fetch", json!([{}]));
    b.check("fetch · nothing new", out["ok"] == json!(true), "git's answer");
    let out = b.one("pull · nothing new", "pull", json!([{}]));
    b.check("pull · nothing new", out["ok"] == json!(true), "git's answer");
    let there = format!("refs/heads/{}", b.branch);
    let before = b.git_in(&b.origin, &["rev-parse", &there]).1.trim().to_owned();
    let out = b.one("push · one commit", "push", json!([{}]));
    b.check("push · one commit", out["ok"] == json!(true) && b.git_in(&b.origin, &["rev-parse", &there]).1.trim() == b.rev("HEAD"), "origin has the commit");
    assert!(b.git_in(&b.origin, &["update-ref", &there, &before]).0);
    b.git(&["update-ref", &format!("refs/remotes/origin/{}", b.branch), &before]);
    b.settle();
}

/// Merge, rebase, cherry-pick, revert, the interactive rebase and a conflict. git refuses most of these over staged
/// changes, so the harness stashes them first (not timed) and brings them back after.
fn history(b: &Bench) {
    b.git(&["stash", "push", "-q", "--include-untracked", "-m", "perf: the harness's own"]);
    b.settle();
    let old = b.rev("HEAD");
    let feature = b.rev("yz-perf/feature");
    let parents = |b: &Bench| b.git(&["rev-list", "--parents", "-n", "1", "HEAD"]).split_whitespace().count() - 1;
    let back = |b: &Bench| {
        b.git(&["reset", "-q", "--hard", &old]);
        b.settle();
    };
    let then_status = |b: &Bench, row: &str| b.one(&format!("{row} → status after it"), "status", json!([{}]));

    let out = b.one("merge · a branch 51 commits apart", "merge", json!(["yz-perf/feature", {}]));
    b.check("merge · a branch 51 commits apart", out["ok"] == json!(true) && parents(b) == 2, "a merge commit");
    then_status(b, "merge · a branch 51 commits apart");
    back(b);

    let out = b.one("rebase · one commit onto 51", "rebase", json!([b.branch, { "branch": "yz-perf/feature" }]));
    b.check("rebase · one commit onto 51", out["ok"] == json!(true) && b.rev("HEAD~1") == old, "the commit is on top");
    b.git(&["checkout", "-q", &b.branch]);
    b.git(&["branch", "-q", "-f", "yz-perf/feature", &feature]);
    b.settle();

    let out = b.one("cherryPick · one commit", "cherryPick", json!([[feature], {}]));
    b.check("cherryPick · one commit", out["ok"] == json!(true) && b.rev("HEAD~1") == old, "a new commit");
    back(b);
    let out = b.one("revert · one commit", "revert", json!([[old], {}]));
    b.check("revert · one commit", out["ok"] == json!(true) && b.rev("HEAD~1") == old, "a new commit");
    back(b);

    let todo = b.on("rebaseTodo", json!(["HEAD~1"])).unwrap();
    let entries: Vec<Value> = todo["commits"].as_array().unwrap().iter().map(|c| json!({ "action": "reword", "hash": c["hash"], "subject": c["subject"], "message": "perf: reworded by the harness" })).collect();
    let out = b.one("rebaseInteractive · the last commit reworded", "rebaseInteractive", json!(["HEAD~1", entries, {}]));
    b.check("rebaseInteractive · the last commit reworded", out["ok"] == json!(true) && b.git(&["log", "-1", "--format=%s"]).trim() == "perf: reworded by the harness", "the message");
    back(b);
    let out = b.one("rewordInMemory", "rewordInMemory", json!([old, "perf: reworded in memory"]));
    b.check("rewordInMemory", out["ok"] == json!(true) && b.git(&["log", "-1", "--format=%s"]).trim() == "perf: reworded in memory", "the message");
    back(b);

    // A merge that stops on a conflict, resolved a chunk at a time and then aborted; again, taking a whole side.
    let conflict = |b: &Bench| {
        assert!(!b.try_git(&["merge", "-q", "yz-perf/conflict"]), "the merge was to conflict");
        b.settle();
    };
    conflict(b);
    let st = b.one("status · in a merge with a conflict", "status", json!([{ "force": true }]));
    b.check("status · in a merge with a conflict", st["counts"]["conflicts"] == json!(1) && st["operation"]["kind"] == json!("merging"), "one conflict, merging");
    let view = b.one("conflictOpen", "conflictOpen", json!([b.file(56)]));
    let chunk = view["chunks"].as_array().and_then(|c| c.iter().find(|c| c["kind"] == json!("conflict"))).map(|c| c["index"].to_string());
    b.check("conflictOpen", chunk.is_some(), "a conflict chunk");
    let unmerged = |b: &Bench| b.git(&["diff", "--name-only", "--diff-filter=U"]);
    b.time("conflictResolve", |b| b.call("conflictResolve", json!([view["id"], { chunk.clone().unwrap_or_default(): "ours" }])));
    b.check("conflictResolve", unmerged(b).is_empty(), "nothing is unmerged");
    let out = b.one("sequencer · abort a merge", "sequencer", json!(["abort"]));
    b.check("sequencer · abort a merge", out["ok"] == json!(true) && !Path::new(&b.root).join(".git/MERGE_HEAD").exists() && b.rev("HEAD") == old, "no merge in progress");
    b.settle();
    conflict(b);
    b.one("conflictAccept · theirs", "conflictAccept", json!([[b.file(56)], "theirs"]));
    b.check("conflictAccept · theirs", unmerged(b).is_empty(), "nothing is unmerged");
    let out = b.one("sequencer · continue a merge", "sequencer", json!(["continue"]));
    b.check("sequencer · continue a merge", out["ok"] == json!(true) && parents(b) == 2, "a merge commit");
    back(b);

    b.git(&["stash", "pop", "-q", "--index"]);
    assert_eq!(b.git(&["status", "--porcelain"]), b.clean, "the harness lost the changes it stashed");
    b.settle();
}

/// Trusting the folder, last: every call before it ran as in a folder nobody was asked about.
fn trust(b: &Bench) {
    let out = b.one("trust", "trust", json!([]));
    b.check("trust", out["ok"] == json!(true) && crate::trust::trusted_folder(&b.repo()), "the folder is trusted");
    let st = b.one("status · forced, trusted folder", "status", json!([{ "force": true }]));
    b.check("status · forced, trusted folder", status_counts(&st) == (12, 40, 27), "the counts");
    let _ = std::fs::remove_file(crate::settings::own_dir().join("git").join("trusted.json"));
    b.settle();
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { f64::NAN } else { (v[(v.len() - 1) / 2] + v[v.len() / 2]) / 2.0 }
}

fn cell(v: &[f64]) -> String {
    if v.is_empty() {
        return "—".into();
    }
    let (lo, hi) = (v.iter().copied().fold(f64::MAX, f64::min), v.iter().copied().fold(f64::MIN, f64::max));
    let digits = if hi < 10.0 { 1 } else { 0 };
    format!("{:.digits$} ({:.digits$}–{:.digits$})", median(v.to_vec()), lo, hi)
}

#[test]
#[ignore = "needs a clone made by scripts/git-bench/make-perf-repo.sh: YUZORA_GIT_PERF_REPO"]
fn perf_every_operation() {
    let Ok(path) = std::env::var("YUZORA_GIT_PERF_REPO") else {
        return;
    };
    let runs: usize = std::env::var("YUZORA_GIT_PERF_RUNS").ok().and_then(|n| n.parse().ok()).unwrap_or(5);
    // Some parts only (their names, with commas): for looking at one; the test then doesn't ask that every function was timed.
    let only = std::env::var("YUZORA_GIT_PERF_ONLY").ok();
    let b = Bench::new(&path);
    b.open();
    b.settle();
    for _ in 0..runs {
        let parts: [(&str, fn(&Bench)); 9] = [("attach", attach), ("outside_edit", outside_edit), ("reads", reads), ("diffs", diffs), ("log", log), ("index_and_commit", index_and_commit), ("refs_and_remote", refs_and_remote), ("history", history), ("trust", trust)];
        for (name, part) in parts {
            if only.as_ref().is_some_and(|o| !o.split(',').any(|o| o == name)) {
                continue;
            }
            part(&b);
            assert_eq!(b.git(&["status", "--porcelain"]), b.clean, "a run left the repository changed");
        }
    }
    let timed = b.timed.borrow();
    let left: Vec<&str> = b.m.function_names().into_iter().filter(|f| !timed.contains(*f)).collect();
    assert!(left.is_empty() || only.is_some(), "never timed: {left:?}");

    let rows = b.rows.borrow();
    let mut samples = String::new();
    let mut calls = String::new();
    println!("| operation | ms: median (range) | runs | git processes | ms a git was running | errors |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (name, r) in rows.iter() {
        let of = |f: fn(&Sample) -> f64| r.samples.iter().map(f).collect::<Vec<f64>>();
        println!("| {name} | {} | {} | {:.0} | {} | {} |", cell(&of(|s| s.ms)), r.samples.len(), median(of(|s| s.git as f64)), cell(&of(|s| s.span)), r.errors.len());
        for s in &r.samples {
            samples.push_str(&format!("{name}\t{:.3}\t{}\t{:.3}\tok\n", s.ms, s.git, s.span));
        }
        for e in &r.errors {
            samples.push_str(&format!("{name}\t0\t0\t0\terror: {}\n", e.replace(['\n', '\t'], " ")));
            println!("error: {name}: {e}");
        }
        calls.push_str(&format!("{name}\n"));
        for c in &r.calls {
            calls.push_str(&format!("\t{c}\n"));
        }
    }
    if let Ok(out) = std::env::var("YUZORA_GIT_PERF_OUT") {
        std::fs::write(&out, samples).unwrap();
        std::fs::write(format!("{out}.calls"), calls).unwrap();
    }
    b.close();
}

/// The samples of several runs side by side: YUZORA_GIT_PERF_SAMPLES=`before=a.tsv,after=b.tsv` (a label may have
/// several files: their samples are pooled).
#[test]
#[ignore = "needs sample files: YUZORA_GIT_PERF_SAMPLES"]
fn perf_table() {
    let Ok(spec) = std::env::var("YUZORA_GIT_PERF_SAMPLES") else {
        return;
    };
    let mut labels: Vec<String> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    // row → label → (ms, git processes, errors)
    let mut rows: BTreeMap<String, BTreeMap<String, (Vec<f64>, Vec<f64>, usize)>> = BTreeMap::new();
    for part in spec.split(',') {
        let (label, file) = part.split_once('=').expect("label=file");
        if !labels.iter().any(|l| l == label) {
            labels.push(label.to_owned());
        }
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if !order.iter().any(|o| o == f[0]) {
                order.push(f[0].to_owned());
            }
            let e = rows.entry(f[0].to_owned()).or_default().entry(label.to_owned()).or_default();
            if f[4] == "ok" {
                e.0.push(f[1].parse().unwrap());
                e.1.push(f[2].parse().unwrap());
            } else {
                e.2 += 1;
            }
        }
    }
    let limit: f64 = std::env::var("YUZORA_GIT_PERF_LIMIT").ok().and_then(|n| n.parse().ok()).unwrap_or(250.0);
    println!("| operation | {} | over {limit:.0} ms |", labels.iter().map(|l| format!("{l}: ms median (range) · runs · git processes · errors")).collect::<Vec<_>>().join(" | "));
    println!("| --- | {} --- |", "--- | ".repeat(labels.len()));
    for name in &order {
        let mut over = "";
        let cells: Vec<String> = labels
            .iter()
            .map(|l| match rows[name].get(l) {
                Some((ms, git, errors)) => {
                    over = if median(ms.clone()) > limit { "over" } else { "" };
                    format!("{} · {} · {:.0} · {errors}", cell(ms), ms.len(), median(git.clone()))
                }
                None => "—".into(),
            })
            .collect();
        println!("| {name} | {} | {over} |", cells.join(" | "));
    }
}
