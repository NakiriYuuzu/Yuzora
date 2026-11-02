//! The repos scripts have opened: one [`Repo`] per work tree (here or on a
//! machine), shared by every mod that asks for it, with its write lock, its
//! caches and a generation that moves whenever something in it changed.
//!
//! A script holds the repo's id (`"<machine>:<root>"`, machine empty here)
//! and passes it back; the Rust side keeps the rest.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::exec::{self, Git, GitError, GitResult, Place};

/// How long a waiter waits before it is answered anyway, so a view that went
/// away leaves nothing behind for longer than this.
pub const WAIT_LIMIT: Duration = Duration::from_secs(25);

pub struct Repo {
    pub id: String,
    pub place: Place,
    /// Absolute `.git` of this work tree (a worktree's own, under the common one).
    pub git_dir: String,
    /// Absolute common git dir (refs, objects, config).
    pub common_dir: String,
    generation: AtomicU64,
    /// Moves when refs, HEAD or the operation in progress changed (the log reloads).
    refs_generation: AtomicU64,
    waiters: Mutex<Vec<(Instant, async_channel::Sender<u64>)>>,
    pub status: Mutex<crate::status::StatusCache>,
    /// One status refresh at a time.
    pub refreshing: Mutex<()>,
    pub watch: Mutex<Option<crate::watch::Watch>>,
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
            refs_generation: AtomicU64::new(1),
            waiters: Mutex::default(),
            status: Mutex::default(),
            refreshing: Mutex::default(),
            watch: Mutex::default(),
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
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
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
        self.status.lock().unwrap_or_else(|e| e.into_inner()).dirty(scope);
        self.bump();
    }

    /// Resolves once the generation is past `since`, or after [`WAIT_LIMIT`].
    pub fn wait(&self, since: u64) -> impl std::future::Future<Output = u64> + Send + use<> {
        let (tx, rx) = async_channel::bounded(1);
        let now = self.generation();
        if now > since {
            let _ = tx.try_send(now);
        } else {
            let mut w = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
            // Old waiters are answered here and by the watcher's tick.
            expire(&mut w);
            w.push((Instant::now(), tx));
        }
        async move { rx.recv().await.unwrap_or(now) }
    }

    /// Answers waiters older than [`WAIT_LIMIT`] (called on a timer).
    pub fn expire_waiters(&self) {
        expire(&mut self.waiters.lock().unwrap_or_else(|e| e.into_inner()));
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

    /// Writes a file (the work tree or `.git`), here or on the machine.
    pub fn write_file(&self, path: &str, bytes: &[u8]) -> GitResult<()> {
        write_file(&self.place, path, bytes)
    }

    /// A path relative to the root, made absolute.
    pub fn abs(&self, rel: &str) -> String {
        format!("{}/{}", self.place.root.trim_end_matches('/'), rel)
    }
}

fn expire(w: &mut Vec<(Instant, async_channel::Sender<u64>)>) {
    w.retain(|(at, tx)| {
        if at.elapsed() >= WAIT_LIMIT {
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

pub fn exists(place: &Place, path: &str) -> bool {
    if place.is_remote() {
        remote_sh(place, r#"test -e "$1""#, &[path], None).is_ok_and(|o| o.ok())
    } else {
        Path::new(path).exists()
    }
}

/// Writes a file; a symlink in the way is replaced, never written through.
pub fn write_file(place: &Place, path: &str, bytes: &[u8]) -> GitResult<()> {
    if place.is_remote() {
        let out = remote_sh(place, r#"if [ -L "$1" ]; then rm -f -- "$1"; fi; mkdir -p "$(dirname -- "$1")" && cat > "$1""#, &[path], Some(bytes.to_vec()))?;
        out.into_result().map(|_| ())
    } else {
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
        remote_sh(place, r#"rm -rf -- "$1""#, &[path], None)?.into_result().map(|_| ())
    } else {
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
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned().ok_or_else(|| GitError::Other(format!("no open repository {id}")))
}

/// The repo holding `cwd` (here, or on machine `on`), opened once and shared.
pub fn open(cwd: &str, on: Option<&str>) -> GitResult<Arc<Repo>> {
    let git = crate::settings::executable();
    let probe = Place { root: cwd.to_owned(), on: on.map(str::to_owned).filter(|m| !m.is_empty()), git: git.clone(), write_lock: Arc::default() };
    let out = exec::run(&probe, Git::read(["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"]))?;
    if !out.ok() {
        return Err(GitError::Other(not_a_repo(&out.stderr_text())));
    }
    let text = out.stdout_text();
    let mut lines = text.lines();
    let (Some(root), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next()) else {
        return Err(GitError::Other(format!("git rev-parse said: {text}")));
    };
    let id = repo_id(probe.on.as_deref(), root);
    let mut all = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = all.get(&id) {
        return Ok(r.clone());
    }
    let place = Place { root: root.to_owned(), on: probe.on.clone(), git, write_lock: Arc::default() };
    let repo = Arc::new(Repo::new(id.clone(), place, git_dir.to_owned(), common.to_owned()));
    all.insert(id, repo.clone());
    drop(all);
    if !repo.place.is_remote() {
        crate::watch::start(&repo);
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
        pub fn git(&self, args: &[&str]) -> String {
            let out = self.repo.run(Git::write(args.iter().map(|s| s.to_string()))).unwrap();
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

    /// An empty repo on branch `main` with a fixed identity; not watched.
    pub fn scratch() -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        // The real path: macOS temp dirs sit behind a /var → /private/var link.
        let root = std::fs::canonicalize(dir.path()).unwrap().to_string_lossy().into_owned();
        let place = Place::local(&root);
        let init = exec::run(&place, Git::write(["init", "-q", "-b", "main"])).unwrap();
        assert!(init.ok(), "{}", init.stderr_text());
        for (k, v) in [("user.name", "Yuzora Test"), ("user.email", "test@yuzora.dev"), ("commit.gpgSign", "false"), ("core.autocrlf", "false")] {
            exec::run(&place, Git::write(["config", k, v])).unwrap();
        }
        let git_dir = format!("{root}/.git");
        let repo = Arc::new(Repo::new(repo_id(None, &root), place, git_dir.clone(), git_dir));
        Scratch { dir, repo }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::testing::scratch;
    use super::*;

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
}
