//! The repos scripts have opened: one [`Repo`] per work tree (here or on a
//! machine), shared by every mod that asks for it, with its write lock, its
//! caches and a generation that moves whenever something in it changed.
//!
//! A script holds the repo's id (`"<machine>:<root>"`, machine empty here)
//! and passes it back; the Rust side keeps the rest.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::exec::{self, Git, GitError, GitResult, Place};

/// How long a waiter waits before it is answered anyway, so a view that went
/// away leaves nothing behind for longer than this.
pub const WAIT_LIMIT: Duration = Duration::from_secs(25);

/// A repo no view has called or waited on for this long stops being watched
/// until the next call (views wait on `changed` all the time they are open).
pub const IDLE: Duration = Duration::from_secs(120);

/// How many generations' changes a repo keeps ([`Repo::changed_paths`]).
pub const KEEP_CHANGES: usize = 256;

/// A change of more paths than this is kept as one that could be anything.
pub const KEEP_PATHS: usize = 1000;

pub struct Repo {
    pub id: String,
    pub place: Place,
    /// Absolute `.git` of this work tree (a worktree's own, under the common one).
    pub git_dir: String,
    /// Absolute common git dir (refs, objects, config).
    pub common_dir: String,
    generation: AtomicU64,
    /// What each of the latest generations changed: its paths, or none when
    /// it could be anything (all of the status, refs, the index git wrote).
    changes: Mutex<std::collections::VecDeque<(u64, Option<Vec<String>>)>>,
    /// Moves when refs, HEAD or the operation in progress changed (the log reloads).
    refs_generation: AtomicU64,
    /// Each with the time it is answered by.
    waiters: Mutex<Vec<(Instant, async_channel::Sender<u64>)>>,
    /// How long a wait lasts at most (ms, [`WAIT_LIMIT`]).
    pub wait_limit: AtomicU64,
    pub status: Mutex<crate::status::StatusCache>,
    /// One status refresh at a time.
    pub refreshing: Mutex<()>,
    pub watch: Mutex<Option<crate::watch::Watch>>,
    /// How long no view may call or wait before the watcher lets go (ms).
    pub idle_after: AtomicU64,
    /// When a view last called.
    used: Mutex<Instant>,
    /// The watcher let go for that: the next call starts it again.
    unwatched: AtomicBool,
    /// Here, but its watcher couldn't start: a wait that runs out counts as
    /// a change, as a machine's does for the sidebar.
    polled: AtomicBool,
    pub blame: Mutex<crate::lru::Lru<String, Arc<serde_json::Value>>>,
}

impl Repo {
    pub fn new(id: String, place: Place, git_dir: String, common_dir: String) -> Self {
        Self {
            id,
            place,
            git_dir,
            common_dir,
            generation: AtomicU64::new(1),
            changes: Mutex::default(),
            refs_generation: AtomicU64::new(1),
            waiters: Mutex::default(),
            wait_limit: AtomicU64::new(WAIT_LIMIT.as_millis() as u64),
            status: Mutex::default(),
            refreshing: Mutex::default(),
            watch: Mutex::default(),
            idle_after: AtomicU64::new(IDLE.as_millis() as u64),
            used: Mutex::new(Instant::now()),
            unwatched: AtomicBool::new(false),
            polled: AtomicBool::new(false),
            blame: Mutex::new(crate::lru::Lru::new(10)),
        }
    }

    pub fn run(&self, cmd: Git) -> GitResult<exec::Output> {
        exec::run(&self.place, cmd)
    }

    /// stdout of a command that has to succeed.
    pub fn text(&self, cmd: Git) -> GitResult<String> {
        self.run(cmd)?.text()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Something changed: every waiter hears the new generation.
    pub fn bump(&self) -> u64 {
        self.bump_for(None)
    }

    /// [`bump`](Self::bump) for a change to these paths (none: anything).
    fn bump_for(&self, paths: Option<Vec<String>>) -> u64 {
        let g = {
            let mut log = self.changes.lock().unwrap_or_else(|e| e.into_inner());
            let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            // A file system often tells of one write more than once.
            let paths = paths.map(|mut p| {
                p.sort_unstable();
                p.dedup();
                p
            });
            log.push_back((g, paths.filter(|p| p.len() <= KEEP_PATHS)));
            while log.len() > KEEP_CHANGES {
                log.pop_front();
            }
            g
        };
        let waiters = std::mem::take(&mut *self.waiters.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, tx) in waiters {
            let _ = tx.try_send(g);
        }
        g
    }

    pub fn refs_generation(&self) -> u64 {
        self.refs_generation.load(Ordering::SeqCst)
    }

    /// Refs or HEAD moved: the log and the whole status are stale.
    pub fn invalidate_refs(&self) {
        self.refs_generation.fetch_add(1, Ordering::SeqCst);
        self.invalidate(crate::status::Scope::All);
    }

    /// The status is stale (all of it, or these paths) and listeners hear it.
    pub fn invalidate(&self, scope: crate::status::Scope) {
        let paths = match &scope {
            crate::status::Scope::Paths(p) => Some(p.clone()),
            crate::status::Scope::All => None,
        };
        self.status.lock().unwrap_or_else(|e| e.into_inner()).dirty(scope);
        self.bump_for(paths);
    }

    /// The paths changed after generation `since`, up to `upto`: none when
    /// one of those changes could be anything, or is no longer kept.
    pub fn changed_paths(&self, since: u64, upto: u64) -> Option<Vec<String>> {
        let log = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        let mut paths = Vec::new();
        let mut kept = 0;
        for (_, p) in log.iter().filter(|(g, _)| *g > since && *g <= upto) {
            paths.extend(p.as_ref()?.iter().cloned());
            kept += 1;
        }
        // Each generation is kept once, in order: all of these are, or the oldest aren't.
        (kept == upto.saturating_sub(since)).then_some(paths)
    }

    /// Resolves once the generation is past `since`, or after `wait_limit`.
    /// Waiting counts as using the repo (a log pane only ever waits).
    pub fn wait(self: &Arc<Self>, since: u64) -> impl std::future::Future<Output = u64> + Send + use<> {
        self.used();
        let (tx, rx) = async_channel::bounded(1);
        let now = self.generation();
        if now > since {
            let _ = tx.try_send(now);
        } else {
            let mut w = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
            // Old waiters leave the list here and on the watcher's tick.
            expire(&mut w);
            let by = Instant::now() + Duration::from_millis(self.wait_limit.load(Ordering::SeqCst));
            w.push((by, tx.clone()));
            answer_by(by, tx);
        }
        let repo = self.clone();
        async move {
            let g = rx.recv().await.unwrap_or(now);
            // Ran out (0) with nothing watching: whatever changed went unseen.
            if g == 0 && repo.polled.load(Ordering::SeqCst) {
                repo.invalidate_refs();
                return repo.generation();
            }
            g
        }
    }

    /// Answers waiters past their time (called on a timer).
    pub fn expire_waiters(&self) {
        expire(&mut self.waiters.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// A view called or waits: if the watcher let go meanwhile, it starts
    /// again and what changed unwatched counts as changed. On its own
    /// thread: calls arrive on the UI thread, and a recursive watch can walk
    /// a whole work tree (inotify).
    fn used(self: &Arc<Self>) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let restart = self.watch.lock().unwrap_or_else(|e| e.into_inner()).is_none() && self.unwatched.swap(false, Ordering::SeqCst);
        if restart {
            let repo = self.clone();
            let spawned = std::thread::Builder::new().name("yuzora-git-watch-start".into()).spawn(move || {
                repo.polled.store(!crate::watch::start(&repo), Ordering::SeqCst);
                repo.invalidate_refs();
            });
            if spawned.is_err() {
                self.unwatched.store(true, Ordering::SeqCst);
            }
        }
    }

    /// The watcher's tick: with no view calling or waiting for
    /// `idle_after`, it lets go (true: it should stop).
    pub fn release_if_idle(&self) -> bool {
        let mut w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        let idle_after = Duration::from_millis(self.idle_after.load(Ordering::SeqCst));
        if w.is_none() || !self.waiters.lock().unwrap_or_else(|e| e.into_inner()).is_empty() || self.used.lock().unwrap_or_else(|e| e.into_inner()).elapsed() < idle_after {
            return false;
        }
        self.unwatched.store(true, Ordering::SeqCst);
        *w = None;
        true
    }

    pub fn git_path(&self, rel: &str) -> String {
        format!("{}/{rel}", self.git_dir.trim_end_matches('/'))
    }

    pub fn common_path(&self, rel: &str) -> String {
        format!("{}/{rel}", self.common_dir.trim_end_matches('/'))
    }

    /// A file's bytes (under `.git` or the work tree), here or on the machine.
    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        read_file(&self.place, path)
    }

    /// At most `limit` bytes of a file, so a huge one is never read whole.
    pub fn read_file_head(&self, path: &str, limit: usize) -> Option<Vec<u8>> {
        read_file_head(&self.place, path, limit)
    }

    pub fn exists(&self, path: &str) -> bool {
        exists(&self.place, path)
    }

    /// What a path is (a link and its target, a program), here or on the machine.
    pub fn kind(&self, path: &str) -> GitResult<PathKind> {
        kind(&self.place, path)
    }

    /// Writes a file (the work tree or `.git`), here or on the machine.
    pub fn write_file(&self, path: &str, bytes: &[u8]) -> GitResult<()> {
        write_file(&self.place, path, bytes)
    }

    /// A path relative to the root, made absolute.
    pub fn abs(&self, rel: &str) -> String {
        format!("{}/{}", self.place.root.trim_end_matches('/'), rel)
    }
}

/// Answers a wait at its time whether or not a watcher ticks: a repo on a
/// machine has none, nor one whose watcher couldn't start. One thread for
/// all of them, there while any wait is.
fn answer_by(by: Instant, tx: async_channel::Sender<u64>) {
    type Waits = (bool, Vec<(Instant, async_channel::Sender<u64>)>);
    static WAITS: Mutex<Waits> = Mutex::new((false, Vec::new()));
    let mut w = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    w.1.push((by, tx));
    if w.0 {
        return;
    }
    let started = std::thread::Builder::new().name("yuzora-git-waits".into()).spawn(|| {
        loop {
            std::thread::sleep(WAIT_TICK);
            let mut w = WAITS.lock().unwrap_or_else(|e| e.into_inner());
            expire(&mut w.1);
            if w.1.is_empty() {
                w.0 = false;
                return;
            }
        }
    });
    w.0 = started.is_ok();
}

/// How late past its time a wait may be answered.
const WAIT_TICK: Duration = Duration::from_secs(1);

fn expire(w: &mut Vec<(Instant, async_channel::Sender<u64>)>) {
    let now = Instant::now();
    w.retain(|(by, tx)| {
        if now >= *by {
            let _ = tx.try_send(0);
            false
        } else {
            !tx.is_closed()
        }
    });
}

/// `sh -c` on another machine, for the few things git has no command for
/// (reading a work tree file, writing one back).
fn remote_sh(place: &Place, script: &str, args: &[&str], stdin: Option<Vec<u8>>) -> GitResult<exec::Output> {
    let mut a = vec!["-c".to_string(), script.to_string(), "yuzora".to_string()];
    a.extend(args.iter().map(|s| s.to_string()));
    let sh = Place { git: "sh".into(), ..place.clone() };
    let mut cmd = Git::read(Vec::<String>::new());
    cmd.args = a;
    cmd.stdin = stdin;
    // `sh` gets git's base options as its own arguments otherwise.
    exec_plain(&sh, cmd)
}

/// Runs `place.git` with exactly `cmd.args` (no git options): used for `sh`.
fn exec_plain(place: &Place, cmd: Git) -> GitResult<exec::Output> {
    let hc = yuzora_core::remote::hc_program().to_string_lossy().into_owned();
    let mut args = vec!["exec".to_string(), "--json".into(), "--on".into(), place.on.clone().unwrap_or_default(), "--timeout".into(), "60".into(), "--cwd".into(), place.root.clone()];
    if cmd.stdin.is_some() {
        args.push("--stdin".into());
    }
    args.push("--".into());
    args.push(place.git.clone());
    args.extend(cmd.args.iter().cloned());
    let mut c = std::process::Command::new(&hc);
    c.args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    yuzora_core::spawn::no_window(&mut c);
    c.stdin(if cmd.stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    let mut child = c.spawn().map_err(|e| GitError::Spawn(format!("cannot run {hc}: {e}")))?;
    if let Some(input) = cmd.stdin {
        use std::io::Write as _;
        let mut w = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || {
            let _ = w.write_all(&input);
        });
    }
    let out = child.wait_with_output().map_err(|e| GitError::Spawn(e.to_string()))?;
    let req = yuzora_core::remote::ExecRequest::new("sh", Vec::<String>::new()).on(place.on.clone()).max_output(64 << 20);
    let parsed = yuzora_core::remote::parse_hc_exec(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr), &req).map_err(|e| GitError::Remote(e.to_string()))?;
    Ok(exec::Output { code: parsed.code, stdout: parsed.stdout, stderr: parsed.stderr })
}

pub fn read_file(place: &Place, path: &str) -> Option<Vec<u8>> {
    if place.is_remote() {
        let out = remote_sh(place, r#"cat -- "$1""#, &[path], None).ok()?;
        out.ok().then_some(out.stdout)
    } else {
        std::fs::read(path).ok()
    }
}

pub fn read_file_head(place: &Place, path: &str, limit: usize) -> Option<Vec<u8>> {
    if place.is_remote() {
        let out = remote_sh(place, r#"head -c "$2" -- "$1""#, &[path, &limit.to_string()], None).ok()?;
        out.ok().then_some(out.stdout)
    } else {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path).ok()?.take(limit as u64).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }
}

/// What git would record for a work-tree path.
#[derive(Clone, Debug, PartialEq)]
pub enum PathKind {
    Missing,
    File { exec: bool },
    /// A symbolic link; its target is its content in git.
    Link(Vec<u8>),
}

pub fn kind(place: &Place, path: &str) -> GitResult<PathKind> {
    if place.is_remote() {
        // `cat` there reads through a link, so ask first: `l` and the
        // target, `x` or `f` for a file, nothing when it's gone.
        let out = remote_sh(place, r#"if [ -L "$1" ]; then printf l; readlink -- "$1"; elif [ -f "$1" ] && [ -x "$1" ]; then printf x; elif [ -e "$1" ]; then printf f; fi"#, &[path], None)?;
        if !out.ok() {
            return Err(GitError::Remote(format!("cannot look at {path}: {}", out.stderr_text().trim())));
        }
        let mut b = out.stdout;
        return Ok(match b.first() {
            Some(b'l') => {
                b.remove(0);
                if b.last() == Some(&b'\n') {
                    b.pop();
                }
                PathKind::Link(b)
            }
            Some(b'x') => PathKind::File { exec: true },
            Some(_) => PathKind::File { exec: false },
            None => PathKind::Missing,
        });
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(PathKind::Missing) };
    if meta.file_type().is_symlink() {
        return Ok(std::fs::read_link(path).map_or(PathKind::Missing, |t| PathKind::Link(link_bytes(t))));
    }
    #[cfg(unix)]
    let exec = {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let exec = false;
    Ok(PathKind::File { exec })
}

/// A link's target as git records it: on unix, its bytes as they are.
pub(crate) fn link_bytes(target: std::path::PathBuf) -> Vec<u8> {
    #[cfg(unix)]
    return std::os::unix::ffi::OsStringExt::into_vec(target.into_os_string());
    #[cfg(not(unix))]
    target.to_string_lossy().into_owned().into_bytes()
}

pub fn exists(place: &Place, path: &str) -> bool {
    if place.is_remote() {
        remote_sh(place, r#"test -e "$1""#, &[path], None).is_ok_and(|o| o.ok())
    } else {
        Path::new(path).exists()
    }
}

/// The shell check of [`no_link_above`]: `$1` the path, `$2` the root.
/// `${p%/*}`, not `$(dirname …)`: `$(…)` drops a name's trailing line feeds.
const NO_LINK_ABOVE: &str = r#"case $1 in "$2"/*) p=${1%/*}; while [ "$p" != "$2" ]; do if [ -L "$p" ]; then echo "$p is a symbolic link: not followed" >&2; exit 3; fi; p=${p%/*}; done;; esac; "#;

/// Refuses a path in the work tree below a symlinked folder: git doesn't
/// go through those (`beyond a symbolic link`), and a write or a removal
/// there would land outside the repo.
fn no_link_above(place: &Place, path: &str) -> GitResult<()> {
    let Ok(rel) = Path::new(path).strip_prefix(&place.root) else { return Ok(()) };
    let mut at = PathBuf::from(&place.root);
    let parts: Vec<_> = rel.components().collect();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(GitError::Other(format!("{} is a symbolic link: not followed", at.display())));
        }
    }
    Ok(())
}

/// Writes a file; a symlink in the way is replaced, never written through,
/// and one above it in the work tree refuses the write.
pub fn write_file(place: &Place, path: &str, bytes: &[u8]) -> GitResult<()> {
    if place.is_remote() {
        let script = format!(r#"{NO_LINK_ABOVE}if [ -L "$1" ]; then rm -f -- "$1"; fi; d=${{1%/*}}; mkdir -p -- "${{d:-/}}" && cat > "$1""#);
        let out = remote_sh(place, &script, &[path, &place.root], Some(bytes.to_vec()))?;
        out.into_result().map(|_| ())
    } else {
        no_link_above(place, path)?;
        if let Some(dir) = Path::new(path).parent() {
            std::fs::create_dir_all(dir).map_err(GitError::other)?;
        }
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            std::fs::remove_file(path).map_err(|e| GitError::Other(format!("cannot replace {path}: {e}")))?;
        }
        std::fs::write(path, bytes).map_err(|e| GitError::Other(format!("cannot write {path}: {e}")))
    }
}

pub fn remove_path(place: &Place, path: &str) -> GitResult<()> {
    if place.is_remote() {
        remote_sh(place, &format!(r#"{NO_LINK_ABOVE}rm -rf -- "$1""#), &[path, &place.root], None)?.into_result().map(|_| ())
    } else {
        no_link_above(place, path)?;
        let p = Path::new(path);
        let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
        match r {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(GitError::Other(format!("cannot remove {path}: {e}"))),
        }
    }
}

fn registry() -> &'static Mutex<HashMap<String, Arc<Repo>>> {
    static REPOS: OnceLock<Mutex<HashMap<String, Arc<Repo>>>> = OnceLock::new();
    REPOS.get_or_init(Mutex::default)
}

pub fn repo_id(on: Option<&str>, root: &str) -> String {
    format!("{}:{root}", on.unwrap_or(""))
}

/// The repo with this id (it has to have been opened).
pub fn get(id: &str) -> GitResult<Arc<Repo>> {
    let r = registry().lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned().ok_or_else(|| GitError::Other(format!("no open repository {id}")))?;
    r.used();
    Ok(r)
}

/// The repo holding `cwd` (here, or on machine `on`), opened once and shared.
pub fn open(cwd: &str, on: Option<&str>) -> GitResult<Arc<Repo>> {
    // No program named: each call runs the git the settings name then.
    let probe = Place { root: cwd.to_owned(), on: on.map(str::to_owned).filter(|m| !m.is_empty()), git: String::new(), top: false, git_dir: String::new(), write_lock: Arc::default() };
    let out = exec::run(&probe, Git::read(["rev-parse", "--path-format=absolute", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"]))?;
    if !out.ok() {
        return Err(GitError::Other(not_a_repo(&out.stderr_text())));
    }
    let text = out.stdout_text();
    // A path a line, unless one has a line feed in it (git prints it as it
    // is): then each on its own. Split at line feeds only: `lines()` would
    // take a carriage return off the end of a name too.
    let paths: Vec<String> = if text.matches('\n').count() == 3 {
        text.split_terminator('\n').map(str::to_owned).collect()
    } else {
        let mut each = Vec::new();
        for flag in ["--show-toplevel", "--absolute-git-dir", "--git-common-dir"] {
            let one = exec::run(&probe, Git::read(["rev-parse", "--path-format=absolute", flag]))?.text()?;
            each.push(one.strip_suffix('\n').unwrap_or(&one).to_owned());
        }
        each
    };
    let [root, git_dir, common] = &paths[..] else {
        return Err(GitError::Other(format!("git rev-parse said: {text}")));
    };
    let id = repo_id(probe.on.as_deref(), root);
    let mut all = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = all.get(&id).cloned() {
        drop(all);
        r.used();
        return Ok(r);
    }
    let place = Place { root: root.to_owned(), on: probe.on.clone(), git: String::new(), top: true, git_dir: git_dir.to_owned(), write_lock: Arc::default() };
    let repo = Arc::new(Repo::new(id.clone(), place, git_dir.to_owned(), common.to_owned()));
    all.insert(id, repo.clone());
    drop(all);
    if !repo.place.is_remote() {
        repo.polled.store(!crate::watch::start(&repo), Ordering::SeqCst);
    }
    Ok(repo)
}

fn not_a_repo(stderr: &str) -> String {
    if stderr.contains("not a git repository") { "not a git repository".into() } else { stderr.trim().to_owned() }
}

/// Forgets every repo whose watcher has no listeners left (tests, reloads).
pub fn close(id: &str) {
    if let Some(r) = registry().lock().unwrap_or_else(|e| e.into_inner()).remove(id) {
        *r.watch.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// What the repo is in the middle of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Operation {
    /// `merging`, `rebasing`, `cherry-picking`, `reverting`, `applying`, `bisecting`, or empty
    /// (`none` in the status JSON).
    pub kind: String,
    /// Rebase progress: step `n` of `total`, onto `onto`, rebasing `head`.
    pub step: Option<(u32, u32)>,
    pub onto: Option<String>,
    pub head: Option<String>,
    pub interactive: bool,
}

/// Reads the state files git leaves in `.git` (as IntelliJ's `GitRepositoryReader`).
pub fn operation(repo: &Repo) -> Operation {
    let names = list_git_dir(repo);
    let has = |n: &str| names.iter().any(|x| x == n);
    let read = |rel: &str| repo.read_file(&repo.git_path(rel)).map(|b| String::from_utf8_lossy(&b).trim().to_owned());
    let mut op = Operation::default();
    if has("rebase-merge") {
        op.kind = "rebasing".into();
        op.interactive = repo.exists(&repo.git_path("rebase-merge/interactive"));
        let n = read("rebase-merge/msgnum").and_then(|s| s.parse().ok());
        let t = read("rebase-merge/end").and_then(|s| s.parse().ok());
        op.step = n.zip(t);
        op.onto = read("rebase-merge/onto");
        op.head = read("rebase-merge/head-name").map(|h| h.trim_start_matches("refs/heads/").to_owned());
    } else if has("rebase-apply") {
        op.kind = if repo.exists(&repo.git_path("rebase-apply/applying")) { "applying".into() } else { "rebasing".into() };
        let n = read("rebase-apply/next").and_then(|s| s.parse().ok());
        let t = read("rebase-apply/last").and_then(|s| s.parse().ok());
        op.step = n.zip(t);
        op.onto = read("rebase-apply/onto");
        op.head = read("rebase-apply/head-name").map(|h| h.trim_start_matches("refs/heads/").to_owned());
    } else if has("MERGE_HEAD") {
        op.kind = "merging".into();
    } else if has("CHERRY_PICK_HEAD") {
        op.kind = "cherry-picking".into();
    } else if has("REVERT_HEAD") {
        op.kind = "reverting".into();
    } else if has("BISECT_LOG") {
        op.kind = "bisecting".into();
    }
    op
}

fn list_git_dir(repo: &Repo) -> Vec<String> {
    if repo.place.is_remote() {
        remote_sh(&repo.place, r#"ls -1a -- "$1""#, &[&repo.git_dir], None).ok().filter(|o| o.ok()).map(|o| o.stdout_text().lines().map(str::to_owned).collect()).unwrap_or_default()
    } else {
        std::fs::read_dir(&repo.git_dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }
}

#[cfg(test)]
pub mod testing {
    //! A scratch repository for tests.
    use super::*;

    pub struct Scratch {
        pub dir: tempfile::TempDir,
        pub repo: Arc<Repo>,
    }

    impl Scratch {
        pub fn root(&self) -> &str {
            &self.repo.place.root
        }
        /// git as the person runs it in a terminal: not offline the way git
        /// the engine starts is before the folder is trusted.
        pub fn git(&self, args: &[&str]) -> String {
            let cmd = Git::write(args.iter().map(|s| s.to_string())).env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh").env("GIT_NO_LAZY_FETCH", "0");
            let out = self.repo.run(cmd).unwrap();
            assert!(out.ok(), "git {args:?}: {}", out.stderr_text());
            out.stdout_text()
        }
        pub fn write(&self, rel: &str, text: &str) {
            let p = Path::new(self.root()).join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        pub fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(Path::new(self.root()).join(rel)).unwrap()
        }
        pub fn commit(&self, rel: &str, text: &str, msg: &str) -> String {
            self.write(rel, text);
            self.git(&["add", "--", rel]);
            self.git(&["commit", "-q", "-m", msg]);
            self.git(&["rev-parse", "HEAD"]).trim().to_owned()
        }
    }

    /// A repo whose `git` is a stand-in that notes when each call starts and
    /// ends, takes a moment, and runs the real one (`fail`: exits 7 for that command).
    #[cfg(unix)]
    pub fn timed_git(s: &Scratch, fail: Option<&str>) -> (Arc<Repo>, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = s.dir.path().join("shim");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let real = String::from_utf8(std::process::Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap().trim().to_owned();
        let body = "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = \"FAIL\" ] && exit 7; done\necho s >> 'LOG'\nsleep 0.4\necho e >> 'LOG'\nexec 'REAL' \"$@\"\n".replace("FAIL", fail.unwrap_or("--no-such-argument")).replace("LOG", &log.display().to_string()).replace("REAL", &real);
        let shim = dir.join("git");
        std::fs::write(&shim, body).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let place = Place { git: shim.to_string_lossy().into_owned(), ..s.repo.place.clone() };
        let git_dir = s.repo.git_dir.clone();
        (Arc::new(Repo::new(s.repo.id.clone(), place, git_dir.clone(), git_dir)), log)
    }

    /// How many of the calls in the log ran at the same time at most.
    #[cfg(unix)]
    pub fn most_at_once(log: &std::path::Path) -> usize {
        let (mut now, mut most) = (0usize, 0usize);
        for l in std::fs::read_to_string(log).unwrap_or_default().lines() {
            if l == "s" {
                now += 1;
                most = most.max(now);
            } else {
                now = now.saturating_sub(1);
            }
        }
        most
    }

    /// An empty repo on branch `main` with a fixed identity; not watched.
    pub fn scratch() -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        // The real path: macOS temp dirs sit behind a /var → /private/var link.
        let root = std::fs::canonicalize(dir.path()).unwrap().to_string_lossy().into_owned();
        // As `open` gives it: git stays in this repo.
        let place = Place { top: true, ..Place::local(&root) };
        let init = exec::run(&place, Git::write(["init", "-q", "-b", "main"])).unwrap();
        assert!(init.ok(), "{}", init.stderr_text());
        for (k, v) in [("user.name", "Yuzora Test"), ("user.email", "test@yuzora.dev"), ("commit.gpgSign", "false"), ("core.autocrlf", "false")] {
            exec::run(&place, Git::write(["config", k, v])).unwrap();
        }
        let git_dir = format!("{root}/.git");
        let repo = Arc::new(Repo::new(repo_id(None, &root), place, git_dir.clone(), git_dir));
        Scratch { dir, repo }
    }

    /// The scratch repo as one `on` a machine: every git call and file look
    /// goes through `hc exec`, answered by a stand-in that runs it here.
    #[cfg(unix)]
    pub fn on_machine(s: &Scratch) -> Arc<Repo> {
        static HC: OnceLock<std::path::PathBuf> = OnceLock::new();
        let hc = HC.get_or_init(|| {
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir().join(format!("yuzora-git-test-hc-{}", std::process::id()));
            // `hc exec --json --on M --timeout T --cwd DIR [--env K=V]... [--stdin] -- program args...`
            let script = r#"#!/bin/sh
[ "$1" = exec ] || exit 2
shift
cwd=. stdin=0
while [ $# -gt 0 ] && [ "$1" != -- ]; do
  case "$1" in
    --cwd) cwd=$2; shift 2 ;;
    --env) export "$2"; shift 2 ;;
    --on|--timeout) shift 2 ;;
    --stdin) stdin=1; shift ;;
    *) shift ;;
  esac
done
shift
printf '%s\n' "$cwd" >> "$0.calls"
# A machine that doesn't answer (tests: a `.hc-hang` in the folder).
[ -e "$cwd/.hc-hang" ] && exec sleep 30
out=$(mktemp) err=$(mktemp)
if [ "$stdin" = 1 ]; then (cd "$cwd" && "$@") >"$out" 2>"$err"; else (cd "$cwd" && "$@") </dev/null >"$out" 2>"$err"; fi
code=$?
printf '{"code":%d,"stdout":"%s","stderr":"%s"}\n' "$code" "$(base64 <"$out" | tr -d '\n')" "$(base64 <"$err" | tr -d '\n')"
rm -f "$out" "$err"
"#;
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        });
        let hc = hc.clone();
        yuzora_core::remote::set_hc_program(move || hc.clone());
        let place = Place { on: Some("test-machine".into()), write_lock: Arc::default(), ..s.repo.place.clone() };
        Arc::new(Repo::new(repo_id(Some("test-machine"), &place.root), place, s.repo.git_dir.clone(), s.repo.common_dir.clone()))
    }

    /// How many `hc exec` calls [`on_machine`]'s stand-in has answered in
    /// `cwd` so far (each is a round trip to the machine).
    #[cfg(unix)]
    pub fn hc_calls_in(cwd: &str) -> usize {
        let log = std::env::temp_dir().join(format!("yuzora-git-test-hc-{}.calls", std::process::id()));
        std::fs::read_to_string(log).map(|t| t.lines().filter(|l| *l == cwd).count()).unwrap_or(0)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::testing::scratch;
    use super::*;

    // What a diff pane asks: which paths changed after its generation.
    #[test]
    fn a_repo_keeps_which_paths_each_change_touched() {
        use crate::status::Scope;
        let s = scratch();
        let paths = |p: &[&str]| Some(p.iter().map(|p| p.to_string()).collect::<Vec<_>>());
        let since = s.repo.generation();
        assert_eq!(s.repo.changed_paths(0, since), None, "the repo's first generation isn't a change kept");
        s.repo.invalidate(Scope::Paths(vec!["a.txt".into()]));
        s.repo.invalidate(Scope::Paths(vec!["b/c.txt".into(), "d".into()]));
        let now = s.repo.generation();
        assert_eq!(s.repo.changed_paths(since, now), paths(&["a.txt", "b/c.txt", "d"]));
        assert_eq!(s.repo.changed_paths(since + 1, now), paths(&["b/c.txt", "d"]));
        assert_eq!(s.repo.changed_paths(since, since + 1), paths(&["a.txt"]), "only up to the generation answered");
        assert_eq!(s.repo.changed_paths(now, now), paths(&[]));
        // Yuzora's own staging and unstaging name their paths.
        s.write("a.txt", "1\n");
        for op in [crate::ops::stage, crate::ops::unstage] {
            let since = s.repo.generation();
            op(&s.repo, &["a.txt".into()]).unwrap();
            assert_eq!(s.repo.changed_paths(since, s.repo.generation()), paths(&["a.txt"]));
        }
        // Anything: all of the status, refs (the index or refs git wrote).
        let (everything, refs) = (|| s.repo.invalidate(Scope::All), || s.repo.invalidate_refs());
        for all in [&everything as &dyn Fn(), &refs] {
            let since = s.repo.generation();
            s.repo.invalidate(Scope::Paths(vec!["a.txt".into()]));
            all();
            assert_eq!(s.repo.changed_paths(since, s.repo.generation()), None);
            assert_eq!(s.repo.changed_paths(since, since + 1), paths(&["a.txt"]));
        }
        // Each path once.
        let since = s.repo.generation();
        s.repo.invalidate(Scope::Paths(vec!["b".into(), "a".into(), "b".into()]));
        assert_eq!(s.repo.changed_paths(since, s.repo.generation()), paths(&["a", "b"]));
        let since = s.repo.generation();
        s.repo.invalidate(Scope::Paths(vec!["a".into(); KEEP_PATHS + 1]));
        assert_eq!(s.repo.changed_paths(since, s.repo.generation()), paths(&["a"]), "one path, told often");
        // More paths in one change than are kept.
        let since = s.repo.generation();
        s.repo.invalidate(Scope::Paths((0..=KEEP_PATHS).map(|i| format!("f{i}")).collect()));
        assert_eq!(s.repo.changed_paths(since, s.repo.generation()), None);
        // Changes no longer kept.
        let since = s.repo.generation();
        for _ in 0..=KEEP_CHANGES {
            s.repo.invalidate(Scope::Paths(vec!["x".into()]));
        }
        let now = s.repo.generation();
        assert_eq!(s.repo.changed_paths(since, now), None);
        assert_eq!(s.repo.changed_paths(now - KEEP_CHANGES as u64, now).map(|p| p.len()), Some(KEEP_CHANGES));
    }

    #[test]
    fn opening_finds_the_root_from_a_subfolder_and_shares_the_repo() {
        let s = scratch();
        s.commit("a/b/c.txt", "x\n", "first");
        let sub = format!("{}/a/b", s.root());
        let r = open(&sub, None).unwrap();
        assert_eq!(r.place.root, s.root());
        assert_eq!(r.git_dir, format!("{}/.git", s.root()));
        let again = open(s.root(), Some("")).unwrap();
        assert!(Arc::ptr_eq(&r, &again));
        assert_eq!(get(&r.id).unwrap().id, r.id);
        close(&r.id);
        let not = tempfile::tempdir().unwrap();
        let Err(err) = open(&not.path().to_string_lossy(), None) else { panic!("opened a folder outside git") };
        assert!(err.to_string().contains("not a git repository"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_work_tree_with_a_line_feed_in_its_path_opens_whole() {
        let dir = tempfile::tempdir().unwrap();
        let root = format!("{}/a\nb", std::fs::canonicalize(dir.path()).unwrap().display());
        std::fs::create_dir(&root).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap().success());
        let r = open(&root, None).unwrap();
        assert_eq!(r.place.root, root);
        assert_eq!(r.git_dir, format!("{root}/.git"));
        assert_eq!(r.common_dir, format!("{root}/.git"));
        assert!(r.run(Git::read(["status", "--porcelain"])).unwrap().ok());
        close(&r.id);
    }

    // `str::lines` takes a carriage return before the line feed off too, and
    // the work tree `r` + CR opened as the folder `r` beside it.
    #[cfg(unix)]
    #[test]
    fn a_work_tree_whose_name_ends_in_a_carriage_return_opens_itself() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap().display().to_string();
        let root = format!("{base}/r\r");
        for d in [&root, &format!("{base}/r")] {
            std::fs::create_dir(d).unwrap();
            assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(d).status().unwrap().success());
        }
        let r = open(&root, None).unwrap();
        close(&r.id);
        assert_eq!((r.place.root.as_str(), r.git_dir.as_str(), r.common_dir.as_str()), (root.as_str(), format!("{root}/.git").as_str(), format!("{root}/.git").as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn writes_never_go_through_a_linked_folder() {
        let s = testing::scratch();
        s.commit("dir/f.txt", "1\n", "first");
        // `dir` swapped for a link to a folder outside the repo.
        let out = tempfile::tempdir().unwrap();
        std::fs::create_dir(out.path().join("sub")).unwrap();
        std::fs::write(out.path().join("f.txt"), "outside\n").unwrap();
        std::fs::write(out.path().join("sub/g.txt"), "outside\n").unwrap();
        std::fs::remove_dir_all(format!("{}/dir", s.root())).unwrap();
        std::os::unix::fs::symlink(out.path(), format!("{}/dir", s.root())).unwrap();
        // One whose name ends in a line feed, which `$(…)` would drop.
        std::os::unix::fs::symlink(out.path(), format!("{}/link\n", s.root())).unwrap();
        let remote = testing::on_machine(&s);
        for r in [&s.repo, &remote] {
            assert!(r.write_file(&r.abs("dir/f.txt"), b"x\n").is_err(), "wrote through the link");
            assert!(r.write_file(&r.abs("dir/sub/g.txt"), b"x\n").is_err(), "wrote through the link above");
            assert!(remove_path(&r.place, &r.abs("dir/sub/g.txt")).is_err(), "removed through the link");
            assert!(r.write_file(&r.abs("link\n/f.txt"), b"x\n").is_err(), "wrote through the link named with a line feed");
            assert!(r.write_file(&r.abs("link\n/sub/g.txt"), b"x\n").is_err(), "wrote through the link named with a line feed above");
            assert!(remove_path(&r.place, &r.abs("link\n/sub/g.txt")).is_err(), "removed through the link named with a line feed");
            // Elsewhere writes go on as before.
            r.write_file(&r.abs("new/h.txt"), b"h\n").unwrap();
        }
        assert_eq!(std::fs::read_to_string(out.path().join("f.txt")).unwrap(), "outside\n");
        assert_eq!(std::fs::read_to_string(out.path().join("sub/g.txt")).unwrap(), "outside\n");
    }

    #[test]
    fn waiters_hear_the_next_generation() {
        let s = scratch();
        let g = s.repo.generation();
        let already = s.repo.wait(g - 1);
        assert_eq!(futures_lite_block(already), g);
        let later = s.repo.wait(g);
        let bumped = s.repo.bump();
        assert_eq!(futures_lite_block(later), bumped);
    }

    #[cfg(unix)]
    #[test]
    fn a_wait_on_a_machine_ends_on_time_without_a_watcher() {
        let s = scratch();
        // Over `hc exec`: no watcher, so nothing of the watcher's answers it.
        let r = super::testing::on_machine(&s);
        assert!(r.watch.lock().unwrap().is_none());
        r.wait_limit.store(200, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        let wait = r.wait(r.generation());
        std::thread::spawn(move || {
            let _ = tx.send(futures_lite_block(wait));
        });
        // The sidebar refreshes a machine's repo when its wait ends.
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(0), "a wait on a machine never ended");
    }

    #[test]
    fn a_repo_whose_watcher_cant_start_reads_again_when_a_wait_runs_out() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        crate::watch::NO_WATCH.with(|n| n.set(true));
        let r = open(s.root(), None);
        crate::watch::NO_WATCH.with(|n| n.set(false));
        let r = r.unwrap();
        assert!(r.watch.lock().unwrap().is_none());
        r.wait_limit.store(200, Ordering::SeqCst);
        assert_eq!(crate::status::read(&r, false).unwrap()["counts"]["unstaged"], 0);
        // Changed with no watcher to see it.
        s.write("a.txt", "2\n");
        let g = r.generation();
        let (tx, rx) = std::sync::mpsc::channel();
        let wait = r.wait(g);
        std::thread::spawn(move || {
            let _ = tx.send(futures_lite_block(wait));
        });
        let next = rx.recv_timeout(Duration::from_secs(5));
        let counts = crate::status::read(&r, false).map(|v| v["counts"]["unstaged"].clone());
        close(&r.id);
        // The sidebar reads a local repo again only when the generation moves.
        assert!(next.is_ok_and(|n| n > g), "the wait ran out as if nothing could change: {next:?}");
        assert_eq!(counts.unwrap(), 1);
    }

    #[test]
    fn the_operation_in_progress_is_read_from_git_dir() {
        let s = scratch();
        s.commit("f.txt", "base\n", "base");
        s.git(&["switch", "-q", "-c", "other"]);
        s.commit("f.txt", "other\n", "other");
        s.git(&["switch", "-q", "main"]);
        s.commit("f.txt", "main\n", "main");
        assert_eq!(operation(&s.repo).kind, "");
        let out = s.repo.run(Git::write(["merge", "other"])).unwrap();
        assert!(!out.ok());
        assert_eq!(operation(&s.repo).kind, "merging");
        s.git(&["merge", "--abort"]);
        let out = s.repo.run(Git::write(["rebase", "other"])).unwrap();
        assert!(!out.ok());
        let op = operation(&s.repo);
        assert_eq!(op.kind, "rebasing");
        assert_eq!(op.step, Some((1, 1)));
        assert_eq!(op.head.as_deref(), Some("main"));
    }

    /// Drives a future that is already ready (or becomes ready from this thread).
    pub fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Nop;
        impl Wake for Nop {
            fn wake(self: Arc<Self>) {}
        }
        let waker = Waker::from(Arc::new(Nop));
        let mut cx = Context::from_waker(&waker);
        let mut f = std::pin::pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_repo_no_view_uses_stops_being_watched_until_one_does() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let r = open(s.root(), None).unwrap();
        let watching = |r: &Repo| r.watch.lock().unwrap().is_some();
        assert!(watching(&r));
        r.idle_after.store(100, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(5);
        while watching(&r) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!watching(&r), "nobody uses it, and it is still watched");
        // The next call watches again, and what it missed counts as changed.
        r.idle_after.store(IDLE.as_millis() as u64, Ordering::SeqCst);
        let g = r.generation();
        let again = get(&r.id).unwrap();
        assert!(wait_until(|| watching(&again) && again.generation() > g));
        close(&r.id);
    }

    /// Lets the repo's watcher go as an idle repo's would.
    fn release_now(r: &Repo) {
        r.idle_after.store(100, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(5);
        while r.watch.lock().unwrap().is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(r.watch.lock().unwrap().is_none(), "not released");
        r.idle_after.store(IDLE.as_millis() as u64, Ordering::SeqCst);
    }

    fn wait_until(what: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while !what() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        what()
    }

    #[test]
    fn a_log_view_alone_brings_its_watcher_back_and_hears_commits() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let r = open(s.root(), None).unwrap();
        release_now(&r);
        // Only the log pane looks at the repo: logWait in a loop, never repo::get.
        // A pane goes idle after its first load; during the load a wait can
        // wake before anything is new.
        let log = crate::log::open(r.clone(), crate::log::Filter::default());
        assert!(wait_until(|| log.info()["loading"] == false));
        let wait = |log: &Arc<crate::log::LogSession>| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (l, g) = (log.clone(), log.generation());
            std::thread::spawn(move || {
                let _ = tx.send(futures_lite_block(l.wait(g)) > g);
            });
            rx
        };
        // Waiting counts as using it: the watcher comes back, and what it
        // missed counts as changed.
        assert_eq!(wait(&log).recv_timeout(Duration::from_secs(5)), Ok(true), "a log view alone never got its watcher back");
        assert!(wait_until(|| r.watch.lock().unwrap().is_some()));
        let heard = wait(&log);
        std::thread::sleep(Duration::from_millis(400));
        s.commit("a.txt", "2\n", "second");
        assert_eq!(heard.recv_timeout(Duration::from_secs(5)), Ok(true), "the log missed a commit");
        crate::log::close(log.id);
        close(&r.id);
    }

    #[test]
    fn a_watcher_starts_again_off_the_calling_thread() {
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let r = open(s.root(), None).unwrap();
        release_now(&r);
        // Calls arrive on the UI thread; walking a big work tree must not.
        let me = std::thread::current().id();
        get(&r.id).unwrap();
        assert!(wait_until(|| r.watch.lock().unwrap().is_some()));
        let on = crate::watch::started_on(&r.id);
        assert!(on.is_some() && on != Some(me), "restarted on the caller's thread");
        close(&r.id);
    }

    #[cfg(unix)]
    #[test]
    fn an_open_repo_runs_the_git_the_settings_name_now() {
        use std::os::unix::fs::PermissionsExt;
        let s = scratch();
        s.commit("a.txt", "1\n", "first");
        let r = open(s.root(), None).unwrap();
        // A git that leaves a mark, then runs the one on PATH. It stays for
        // the whole run: other tests may call it while the setting names it.
        let dir = crate::settings::own_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let (mine, mark) = (dir.join("my-git"), dir.join("my-git-ran"));
        std::fs::write(&mine, format!("#!/bin/sh\ntouch '{}'\nexec git \"$@\"\n", mark.display())).unwrap();
        std::fs::set_permissions(&mine, std::fs::Permissions::from_mode(0o755)).unwrap();
        let set = |exe: &str| yuzora_core::settings::set_section_at(&crate::settings::file(), crate::settings::SECTION, serde_json::json!({ "executable": exe })).unwrap();
        set(&mine.to_string_lossy());
        // Opening again finds the root with it (a probe); the open repo's own calls follow.
        let again = open(s.root(), None).unwrap();
        let _ = std::fs::remove_file(&mark);
        let ran = again.run(Git::read(["--version"])).map(|o| o.ok());
        let used = mark.exists();
        set("");
        assert_eq!(ran, Ok(true));
        assert!(Arc::ptr_eq(&r, &again));
        assert!(used, "the open repo kept the git it was opened with");
        close(&r.id);
    }
}
