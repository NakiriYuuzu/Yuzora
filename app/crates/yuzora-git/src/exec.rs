//! Running git: the one place that decides git's environment, the repo's
//! write lock, the timeout and cancellation, here or on another machine.
//!
//! As IntelliJ (`GitHandler`, `GitImplBase`):
//! - every call carries `-c core.quotepath=false -c log.showSignature=false`
//!   and a UTF-8 locale with English messages, so output parses the same way;
//! - reads set `GIT_OPTIONAL_LOCKS=0` (and don't refresh the index from
//!   `diff`), so a status never fights a rebase in a terminal for
//!   `index.lock`, and never writes under `.git` for the watcher to see;
//! - reads turn `core.fsmonitor` off: the sidebar runs status on its own as
//!   soon as a folder is focused, and a repo's own config must not get a
//!   program run that way (there is no trusted-folder check yet);
//! - writes take the repo's write lock first, so two of ours never race;
//! - a call can be cancelled or time out: everything it started is killed
//!   (its process group; on Windows, the job object it runs in);
//! - long pathspecs go through `--pathspec-from-file` (see [`pathspec`]).
//!
//! Another machine goes through herdr-core: `hc exec --json --on <machine>
//! --cwd <root> --env … [-i] -- git …` (stdin piped through `-i`).
//!
//! Blocking: call from a worker thread ([`spawn`] makes one per call).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use yuzora_core::remote::{ExecRequest, hc_program, parse_hc_exec};

/// Reads end here unless the caller asks for longer.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Network and history-rewriting commands may take a while.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(600);
/// What a call keeps of each stream unless it asks for more.
pub const DEFAULT_MAX_OUTPUT: usize = 64 << 20;
/// IntelliJ's `VcsFileUtil.FILE_PATH_LIMIT`: longer pathspecs go through a file.
pub const PATHSPEC_LIMIT: usize = 7600;
/// After git exits, how long its output may take to arrive (a background
/// `gc --auto` can hold the pipes).
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// How long past a remote timeout the local `hc` may take to answer.
const REMOTE_GRACE: Duration = Duration::from_secs(5);
/// hc refuses longer timeouts.
const REMOTE_MAX_TIMEOUT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(20);

/// Set once to stop a running call (and anything waiting for the lock).
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Whether a command changes the repo (and so takes its write lock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
}

/// Where git runs: a work tree here or on a machine, with its write lock.
#[derive(Clone, Debug)]
pub struct Place {
    pub root: String,
    /// A machine from `hc machine ls`; `None` is this computer.
    pub on: Option<String>,
    /// The program to run; empty: the `git.executable` setting (else `git`)
    /// at each call, so a change applies to the repos already open.
    pub git: String,
    /// `root` is a work tree's top level (an open repo): git looks for the
    /// repository there only, never in a folder above ([`Place::confine`]).
    /// With the work tree's `.git` gone, git then fails rather than reading
    /// and writing the repo around it under this one's name. Off where a repo
    /// is looked for from a folder in it (`open`).
    pub top: bool,
    /// For a [`top`](Self::top) place: the repo's git dir as `open` had git say it
    /// (`--absolute-git-dir`: a linked worktree's `.git/worktrees/<n>`, a submodule's
    /// `.git/modules/<n>`). [`check_top`] compares it with git's answer, so a repo
    /// that only reaches this folder through its `core.worktree` is not this one.
    pub git_dir: String,
    pub write_lock: Arc<Mutex<()>>,
}

impl Place {
    pub fn local(root: impl Into<String>) -> Self {
        Self { root: root.into(), on: None, git: "git".into(), top: false, git_dir: String::new(), write_lock: Arc::default() }
    }

    /// What keeps git in this place's repo, for a [`top`](Self::top) one: the
    /// folder above `root` as `GIT_CEILING_DIRECTORIES` (none above `/` or a
    /// drive), and only that: a hook inherits it, and a hook must never get
    /// `GIT_DIR`/`GIT_WORK_TREE` (git itself sets neither for a plain repo; a
    /// hook that `cd`s elsewhere would run git in this repo, with that folder
    /// as its work tree). The ceiling is a list git splits at `:`, or at `;`
    /// on Windows; which git runs isn't told here (a machine may be either
    /// kind), so a folder above with either in its path (but a drive's `C:`)
    /// can't be in it: nothing is set then, and [`Place::needs_git_check`]
    /// has every command ask git which repository it finds first.
    fn confine(&self) -> Vec<(String, String)> {
        match above(&self.root).filter(|a| self.top && !has_list_separator(a)) {
            Some(above) => vec![("GIT_CEILING_DIRECTORIES".into(), above)],
            None => Vec::new(),
        }
    }

    /// A top-level place that [`Place::confine`] can't hold in its repo (the
    /// folder above has a list separator in its path): every command, read or
    /// write, first has git say which repository it finds at `root`
    /// ([`check_top`]).
    fn needs_git_check(&self) -> bool {
        self.top && above(&self.root).is_some_and(|a| has_list_separator(&a))
    }

    pub fn program(&self) -> String {
        if self.git.is_empty() { crate::settings::executable() } else { self.git.clone() }
    }
    pub fn is_remote(&self) -> bool {
        self.on.is_some()
    }
}

/// The folder above `root`, as git wrote it (`/` between names, also on Windows), whichever kind of machine's
/// path it is (`/srv/repo` → `/srv`, `C:/x/repo` → `C:/x`, `C:/repo` → `C:/`);
/// none above `/` or a drive.
fn above(root: &str) -> Option<String> {
    // Only `/`: git writes every root so, and a unix name may hold a `\`.
    let path = root.trim_end_matches('/');
    let up = &path[..path.rfind('/')?];
    Some(match up {
        "" => "/".to_owned(),
        drive if drive.len() == 2 && drive.ends_with(':') => format!("{drive}/"),
        up => up.to_owned(),
    })
}

/// Whether a path has a character `GIT_CEILING_DIRECTORIES` is split at (`:`,
/// `;` on Windows): a drive's own `C:` is not one.
fn has_list_separator(path: &str) -> bool {
    let drive = path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':';
    path[if drive { 2 } else { 0 }..].contains([':', ';'])
}

/// One git command.
#[derive(Clone, Debug)]
pub struct Git {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(String, String)>,
    pub kind: Kind,
    pub timeout: Duration,
    pub max_output: usize,
    pub cancel: Option<Cancel>,
}

impl Git {
    pub fn read<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Self {
        Self::new(Kind::Read, args)
    }
    pub fn write<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Self {
        Self::new(Kind::Write, args)
    }
    fn new<S: Into<String>>(kind: Kind, args: impl IntoIterator<Item = S>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            env: Vec::new(),
            kind,
            timeout: if kind == Kind::Read { READ_TIMEOUT } else { LONG_TIMEOUT },
            max_output: DEFAULT_MAX_OUTPUT,
            cancel: None,
        }
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn args<S: Into<String>>(mut self, a: impl IntoIterator<Item = S>) -> Self {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
    pub fn max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }
    pub fn cancel(mut self, c: Option<Cancel>) -> Self {
        self.cancel = c;
        self
    }

    /// Adds `-- <paths>`, or `--pathspec-from-file=- --pathspec-file-nul` with
    /// the paths on stdin when they are longer than [`PATHSPEC_LIMIT`]. Only
    /// for commands that take it (add, reset, restore, checkout, commit, rm,
    /// stash push); everything else uses [`Git::paths_or_all`].
    pub fn pathspec(mut self, paths: &[String]) -> Self {
        if paths.is_empty() {
            return self;
        }
        if pathspec_len(paths) > PATHSPEC_LIMIT && self.stdin.is_none() {
            let mut input = Vec::new();
            for p in paths {
                input.extend_from_slice(p.as_bytes());
                input.push(0);
            }
            self.args.extend(["--pathspec-from-file=-".into(), "--pathspec-file-nul".into()]);
            self.stdin = Some(input);
        } else {
            self.args.push("--".into());
            self.args.extend(paths.iter().cloned());
        }
        self
    }

    /// Adds `-- <paths>` when they fit; past [`PATHSPEC_LIMIT`] adds nothing
    /// (the whole root), as IntelliJ's `createHandlerWithPaths`. Returns
    /// whether the paths were kept.
    pub fn paths_or_all(mut self, paths: &[String]) -> (Self, bool) {
        if paths.is_empty() || pathspec_len(paths) > PATHSPEC_LIMIT {
            return (self, false);
        }
        self.args.push("--".into());
        self.args.extend(paths.iter().cloned());
        (self, true)
    }
}

pub fn pathspec_len(paths: &[String]) -> usize {
    paths.iter().map(|p| p.len() + 1).sum()
}

/// `paths` in runs that each fit in [`PATHSPEC_LIMIT`], for commands that
/// take no `--pathspec-from-file` (clean) and must never fall back to all.
pub fn pathspec_runs(paths: &[String]) -> Vec<&[String]> {
    let mut runs = Vec::new();
    let (mut start, mut len) = (0, 0);
    for (i, p) in paths.iter().enumerate() {
        if i > start && len + p.len() + 1 > PATHSPEC_LIMIT {
            runs.push(&paths[start..i]);
            (start, len) = (i, 0);
        }
        len += p.len() + 1;
    }
    if start < paths.len() {
        runs.push(&paths[start..]);
    }
    runs
}

/// What git printed and how it ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
    /// stdout, or an error carrying stderr when git failed.
    pub fn into_result(self) -> Result<Vec<u8>, GitError> {
        if self.ok() { Ok(self.stdout) } else { Err(GitError::Failed { code: self.code, message: failure_text(&self) }) }
    }
    pub fn text(self) -> Result<String, GitError> {
        self.into_result().map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

fn failure_text(o: &Output) -> String {
    let err = String::from_utf8_lossy(&o.stderr).trim().to_owned();
    if err.is_empty() { String::from_utf8_lossy(&o.stdout).trim().to_owned() } else { err }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitError {
    /// git (or hc) could not start.
    Spawn(String),
    TimedOut { seconds: f64 },
    Cancelled,
    /// herdr-core refused, or could not reach the machine.
    Remote(String),
    /// git ran and failed.
    Failed { code: i32, message: String },
    /// Anything else this crate refuses (bad input, a changed index…).
    Other(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::Spawn(e) | GitError::Remote(e) | GitError::Other(e) => f.write_str(e),
            GitError::TimedOut { seconds } => write!(f, "git took longer than {seconds} s and was stopped"),
            GitError::Cancelled => f.write_str("cancelled"),
            GitError::Failed { message, code } if message.is_empty() => write!(f, "git exited with {code}"),
            GitError::Failed { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for GitError {}

impl GitError {
    pub fn other(e: impl std::fmt::Display) -> Self {
        GitError::Other(e.to_string())
    }
}

pub type GitResult<T> = Result<T, GitError>;

/// The options every call carries.
pub fn base_args(kind: Kind) -> Vec<String> {
    let mut a: Vec<String> = ["-c", "core.quotepath=false", "-c", "log.showSignature=false", "-c", "color.ui=false"].map(String::from).to_vec();
    if kind == Kind::Read {
        // `diff` would otherwise refresh stat info into the index.
        a.extend(["-c".into(), "diff.autoRefreshIndex=false".into()]);
        // A repo's config can name an fsmonitor program; reads run unasked.
        a.extend(["-c".into(), "core.fsmonitor=false".into()]);
    }
    a
}

/// Locale for git's messages: English text, UTF-8 bytes.
fn locale(remote: bool) -> &'static str {
    if !remote && cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" }
}

#[cfg(test)]
thread_local! {
    /// More environment for every command this test thread starts (a PATH
    /// with a remote helper on it), as a person's own environment would have.
    pub static TEST_ENV: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A scoped thread that starts its commands the way the calling thread would
/// (tests: with that thread's [`TEST_ENV`]).
pub fn spawn_scoped<'scope, 'env, T: Send + 'scope>(s: &'scope std::thread::Scope<'scope, 'env>, f: impl FnOnce() -> T + Send + 'scope) -> std::thread::ScopedJoinHandle<'scope, T> {
    #[cfg(test)]
    let env = TEST_ENV.with(|t| t.borrow().clone());
    s.spawn(move || {
        #[cfg(test)]
        TEST_ENV.with(|t| *t.borrow_mut() = env);
        f()
    })
}

/// The environment every call carries; the command's own entries win.
pub fn environment(kind: Kind, remote: bool, own: &[(String, String)]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("LANGUAGE".into(), String::new()),
        ("LC_ALL".into(), locale(remote).into()),
        // No terminal to ask in: fail instead of hanging on a prompt.
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        // Commands that would open an editor take their default message.
        ("GIT_EDITOR".into(), ":".into()),
        ("GIT_MERGE_AUTOEDIT".into(), "no".into()),
        // Hooks can tell they run under Yuzora, as IntelliJ's do.
        ("YUZORA_GIT".into(), "1".into()),
        // Paths are file names, never globs: `pages/[id].tsx` must not
        // also match `pages/i.tsx`.
        ("GIT_LITERAL_PATHSPECS".into(), "1".into()),
    ];
    if kind == Kind::Read {
        env.push(("GIT_OPTIONAL_LOCKS".into(), "0".into()));
    }
    #[cfg(test)]
    TEST_ENV.with(|t| env.extend(t.borrow().iter().cloned()));
    for (k, v) in own {
        env.retain(|(key, _)| key != k);
        env.push((k.clone(), v.clone()));
    }
    env
}

/// Runs one command and waits for it.
pub fn run(place: &Place, cmd: Git) -> GitResult<Output> {
    run_streaming(place, cmd, None)
}

/// Runs one command; `sink` receives stdout as it arrives (here) or at the
/// end (on another machine) instead of it being kept. A sink returning false
/// stops the command.
pub fn run_streaming(place: &Place, cmd: Git, sink: Option<&mut dyn FnMut(&[u8]) -> bool>) -> GitResult<Output> {
    // Every git the engine starts comes through here: offline until the folder is trusted.
    let (cmd, guarded) = crate::trust::guard(place, cmd);
    let _guard = match cmd.kind {
        Kind::Write => Some(lock(&place.write_lock, cmd.cancel.as_ref())?),
        Kind::Read => None,
    };
    // Every command, read or write (a fetch writes, a status runs filters): where no ceiling can
    // hold git in the repo, git is asked first which repository it finds there.
    if place.needs_git_check() {
        check_top(place, &cmd)?;
    }
    let out = if place.is_remote() { run_remote(place, &cmd, sink) } else { run_local(place, &cmd, sink) }?;
    if let Some(g) = guarded {
        crate::trust::heard(g, &out);
    }
    Ok(out)
}

/// Asks git (`rev-parse --show-toplevel --absolute-git-dir`, the same folder and environment,
/// here or on the machine, with the command's own cancel and timeout) which repository it finds
/// at `place.root`, and refuses the command unless that is this one: its work tree is the folder
/// itself and its git dir is the one `open` recorded. git skips a `.git` it doesn't take for a
/// repository (empty, no `HEAD`, no `objects`) and goes on up, so only git's own answer tells;
/// and the top level alone doesn't: a repository around the folder whose `core.worktree` names
/// it (absolute, relative, in any case) answers with the folder, and would run the command with
/// its own refs, index and hooks. `rev-parse` runs no program a repository names: it reads the
/// config for paths and never the index, filters, hooks or a transport.
fn check_top(place: &Place, cmd: &Git) -> GitResult<()> {
    let ask = |flags: &[&str]| -> GitResult<Output> {
        let mut probe = Git::read(std::iter::once("rev-parse").chain(flags.iter().copied()));
        probe.timeout = cmd.timeout.min(READ_TIMEOUT);
        probe.cancel = cmd.cancel.clone();
        probe.max_output = 64 << 10;
        if place.is_remote() { run_remote(place, &probe, None) } else { run_local(place, &probe, None) }
    };
    let out = ask(&["--show-toplevel", "--absolute-git-dir"])?;
    let text = out.stdout_text();
    // A path a line, unless one has a line feed in it (git prints it as it is): then each alone.
    let found: Option<(String, String)> = if !out.ok() {
        None
    } else if text.matches('\n').count() == 2 {
        text.split_once('\n').map(|(top, gd)| (top.to_owned(), gd.strip_suffix('\n').unwrap_or(gd).to_owned()))
    } else {
        let mut each = Vec::new();
        for flag in ["--show-toplevel", "--absolute-git-dir"] {
            let one = ask(&[flag])?;
            if !one.ok() {
                return Err(not_here(place, one.stderr_text().trim()));
            }
            let one = one.stdout_text();
            each.push(one.strip_suffix('\n').unwrap_or(&one).to_owned());
        }
        let git_dir = each.pop().unwrap_or_default();
        Some((each.pop().unwrap_or_default(), git_dir))
    };
    let Some((top, git_dir)) = found else {
        return Err(not_here(place, out.stderr_text().trim()));
    };
    if same_folder(&top, &place.root, place.is_remote()) && same_folder(&git_dir, &place.git_dir, place.is_remote()) {
        return Ok(());
    }
    Err(not_here(place, &format!("the repository at {top}, git dir {git_dir}")))
}

fn not_here(place: &Place, seen: &str) -> GitError {
    GitError::Other(format!("git finds no repository at {} ({seen}): git is not run there (a repository around it would get the change)", place.root))
}

/// Whether git's `found` and `root` are the same folder: by real path here (which also gives a
/// folder the case it has on disk); on a machine as written (git gave both). Drive letters,
/// `\\?\`, `\` and a trailing `/` don't tell folders apart. Case tells them apart for a unix path:
/// `/srv/Repo` and `/srv/repo` are two folders where the file system tells them apart, and
/// nothing here can say whether it does; a Windows path (a drive or `//host`) is the same in any case.
fn same_folder(found: &str, root: &str, remote: bool) -> bool {
    let norm = |p: &str| -> String {
        let p = if remote { p.to_owned() } else { std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_owned()) };
        let p = yuzora_core::paths::plain_str(&p);
        let win = p.starts_with("\\\\") || p.starts_with("//") || (p.len() >= 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':');
        let p = if win { p.replace('\\', "/") } else { p };
        let kept = p.trim_end_matches('/');
        // `/` and `C:/` are their own folder.
        let p = if kept.is_empty() || kept.ends_with(':') { p.as_str() } else { kept };
        if win { p.to_lowercase() } else { p.to_owned() }
    };
    !found.is_empty() && !root.is_empty() && norm(found) == norm(root)
}

fn lock<'a>(m: &'a Mutex<()>, cancel: Option<&Cancel>) -> GitResult<std::sync::MutexGuard<'a, ()>> {
    loop {
        match m.try_lock() {
            Ok(g) => return Ok(g),
            Err(std::sync::TryLockError::Poisoned(p)) => return Ok(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if cancel.is_some_and(Cancel::is_cancelled) {
                    return Err(GitError::Cancelled);
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

fn run_local(place: &Place, cmd: &Git, mut sink: Option<&mut dyn FnMut(&[u8]) -> bool>) -> GitResult<Output> {
    let git = place.program();
    let mut c = Command::new(&git);
    c.args(base_args(cmd.kind)).args(&cmd.args).current_dir(&place.root);
    for (k, v) in environment(cmd.kind, false, &cmd.env).into_iter().chain(place.confine()) {
        c.env(k, v);
    }
    c.stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own group, so a kill reaches the hooks, ssh and helpers it started.
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Suspended until it is in its job (see `job`).
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        c.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let mut child = c.spawn().map_err(|e| GitError::Spawn(format!("cannot run {git}: {e}")))?;
    #[cfg(windows)]
    let tree: Tree = match job::adopt(&child) {
        Ok(job) => job,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Spawn(format!("cannot start {git}: {e}")));
        }
    };
    #[cfg(not(windows))]
    let tree: Tree = ();
    if let Some(input) = cmd.stdin.clone() {
        let mut w = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || {
            let _ = w.write_all(&input);
        });
    }
    let out = Stream::start(child.stdout.take().expect("piped stdout"));
    let err = Stream::start(child.stderr.take().expect("piped stderr"));
    let start = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stopped = false;
    let mut exited: Option<(i32, Instant)> = None;
    loop {
        // Once git has exited and both pipes closed (or the grace is over),
        // this pass takes whatever is left and returns.
        let finishing = exited.is_some_and(|(_, at)| (out.ended.load(Ordering::SeqCst) && err.ended.load(Ordering::SeqCst)) || at.elapsed() > PIPE_GRACE);
        let mut progressed = false;
        while let Ok(chunk) = out.rx.try_recv() {
            progressed = true;
            match sink.as_mut() {
                Some(s) if !stopped => {
                    if !s(&chunk) {
                        stopped = true;
                        kill(&mut child, &tree);
                    }
                }
                Some(_) => {}
                None => {
                    let room = cmd.max_output.saturating_sub(stdout.len());
                    stdout.extend_from_slice(&chunk[..chunk.len().min(room)]);
                }
            }
        }
        while let Ok(chunk) = err.rx.try_recv() {
            progressed = true;
            let room = cmd.max_output.saturating_sub(stderr.len());
            stderr.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        if let Some((code, _)) = exited.filter(|_| finishing) {
            return Ok(Output { code, stdout, stderr });
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = Some((exit_code(status), Instant::now()));
                    continue;
                }
                Ok(None) => {}
                Err(e) => return Err(GitError::Spawn(e.to_string())),
            }
            if cmd.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
                kill(&mut child, &tree);
                return Err(GitError::Cancelled);
            }
            if start.elapsed() > cmd.timeout {
                kill(&mut child, &tree);
                return Err(GitError::TimedOut { seconds: cmd.timeout.as_secs_f64() });
            }
        }
        if !progressed {
            // Wakes on the next chunk; at the latest after POLL.
            let _ = out.wake.recv_timeout(POLL);
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    status.code().unwrap_or(-1)
}

/// What a kill reaches besides git: on Windows the job it runs in (the
/// process group does it elsewhere).
#[cfg(windows)]
type Tree = job::Job;
#[cfg(not(windows))]
type Tree = ();

fn kill(child: &mut std::process::Child, tree: &Tree) {
    #[cfg(unix)]
    {
        // The group first (hooks, ssh, credential helpers), then git itself.
        let pid = child.id() as i32;
        // SAFETY: plain syscall on a process group we created.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    tree.terminate();
    let _ = tree;
    let _ = child.kill();
    let _ = child.wait();
}

/// Windows has no process group to kill: git runs in a job object, so a
/// cancel or a timeout ends the hooks, ssh and helpers it started too. It is
/// started suspended and only runs once it is in the job, so nothing it
/// starts can slip out first. The job is not killed on close: a helper git
/// leaves running on purpose (a credential cache) outlives a normal exit,
/// and one that asks to leave the job (CREATE_BREAKAWAY_FROM_JOB) may, as a
/// process can leave its group on unix. A browser a credential helper
/// starts for a sign-in is in the job: cancelling that fetch closes it.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub struct Job(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub fn terminate(&self) {
            // SAFETY: a job handle we own.
            unsafe { TerminateJobObject(self.0, 1) };
        }

        /// Whether a program that asks to leave the job may (tests).
        #[cfg(test)]
        pub fn lets_breakaway(&self) -> bool {
            use windows_sys::Win32::System::JobObjects::{JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, QueryInformationJobObject};
            // SAFETY: a job handle we own and a struct of the size given.
            unsafe {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let ok = QueryInformationJobObject(self.0, JobObjectExtendedLimitInformation, &mut info as *mut _ as *mut _, std::mem::size_of_val(&info) as u32, std::ptr::null_mut());
                ok != 0 && info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_BREAKAWAY_OK != 0
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closed once, here.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Puts a child started suspended into a new job, then lets it run. Err:
    /// no job (it couldn't be made or joined), or it couldn't be resumed. The
    /// caller kills it then: git never runs where a cancel ends git alone.
    pub fn adopt(child: &std::process::Child) -> std::io::Result<Job> {
        // SAFETY: plain calls on handles we own or that the child owns.
        let job = unsafe {
            let h = if no_job() { std::ptr::null_mut() } else { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // A program that asks to leave (CREATE_BREAKAWAY_FROM_JOB)
            // may: it would fail to start otherwise.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
            if AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                let e = std::io::Error::last_os_error();
                CloseHandle(h);
                return Err(e);
            }
            Job(h)
        };
        if resume(child.id()) { Ok(job) } else { Err(std::io::Error::last_os_error()) }
    }

    #[cfg(test)]
    thread_local! {
        /// Tests: this thread's calls get no job, as if Windows refused one.
        pub static NO_JOB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn no_job() -> bool {
        #[cfg(test)]
        return NO_JOB.with(std::cell::Cell::get);
        #[cfg(not(test))]
        false
    }

    /// Resumes the threads of a process started suspended (std doesn't hand
    /// out its main thread).
    fn resume(pid: u32) -> bool {
        // SAFETY: a snapshot and thread handles we open and close here; the
        // entry is sized as the API asks.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut e: THREADENTRY32 = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = false;
            let mut more = Thread32First(snap, &mut e) != 0;
            while more {
                if e.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, e.th32ThreadID);
                    if !t.is_null() {
                        resumed |= ResumeThread(t) != u32::MAX;
                        CloseHandle(t);
                    }
                }
                more = Thread32Next(snap, &mut e) != 0;
            }
            CloseHandle(snap);
            resumed
        }
    }
}

/// One stream read on its own thread into a channel.
struct Stream {
    rx: mpsc::Receiver<Vec<u8>>,
    wake: mpsc::Receiver<()>,
    ended: Arc<AtomicBool>,
}

impl Stream {
    fn start(mut r: impl Read + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (wake_tx, wake) = mpsc::sync_channel(1);
        let ended = Arc::new(AtomicBool::new(false));
        let done = ended.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 << 10];
            loop {
                match r.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        let _ = wake_tx.try_send(());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            done.store(true, Ordering::SeqCst);
            let _ = wake_tx.try_send(());
        });
        Stream { rx, wake, ended }
    }
}

/// The `hc exec` arguments for a remote command.
pub fn hc_args(place: &Place, cmd: &Git) -> Vec<String> {
    let secs = cmd.timeout.min(REMOTE_MAX_TIMEOUT).as_secs_f64().ceil().max(1.0) as u64;
    let mut a: Vec<String> = vec!["exec".into(), "--json".into(), "--on".into(), place.on.clone().unwrap_or_default(), "--timeout".into(), secs.to_string(), "--cwd".into(), place.root.clone()];
    for (k, v) in environment(cmd.kind, true, &cmd.env).into_iter().chain(place.confine()) {
        a.push("--env".into());
        a.push(format!("{k}={v}"));
    }
    if cmd.stdin.is_some() {
        a.push("--stdin".into());
    }
    a.push("--".into());
    a.push(place.program());
    a.extend(base_args(cmd.kind));
    a.extend(cmd.args.iter().cloned());
    a
}

fn run_remote(place: &Place, cmd: &Git, sink: Option<&mut dyn FnMut(&[u8]) -> bool>) -> GitResult<Output> {
    let hc = hc_program().to_string_lossy().into_owned();
    // hc answers with both streams base64'd (4/3 the size), whole.
    let answer = cmd.max_output.saturating_mul(3).saturating_add(64 << 10);
    let local = Place { root: yuzora_core::paths::home_dir().to_string_lossy().into_owned(), on: None, git: hc, top: false, git_dir: String::new(), write_lock: Arc::default() };
    let call = Git {
        args: hc_args(place, cmd),
        stdin: cmd.stdin.clone(),
        env: Vec::new(),
        kind: Kind::Read,
        timeout: cmd.timeout.min(REMOTE_MAX_TIMEOUT) + REMOTE_GRACE,
        max_output: answer,
        cancel: cmd.cancel.clone(),
    };
    let out = run_hc(&local, &call)?;
    let req = ExecRequest::new("git", Vec::<String>::new()).on(place.on.clone()).timeout(cmd.timeout).max_output(cmd.max_output as u64);
    let parsed = parse_hc_exec(&out.stdout_text(), &out.stderr_text(), &req).map_err(|e| match e {
        yuzora_core::remote::ExecError::TimedOut { seconds, .. } => GitError::TimedOut { seconds },
        other => GitError::Remote(other.to_string()),
    })?;
    let mut result = Output { code: parsed.code, stdout: parsed.stdout, stderr: parsed.stderr };
    if let Some(s) = sink {
        s(&result.stdout);
        result.stdout.clear();
    }
    Ok(result)
}

/// `hc` itself runs here, without git's options.
fn run_hc(local: &Place, call: &Git) -> GitResult<Output> {
    let mut c = Command::new(&local.git);
    c.args(&call.args).current_dir(&local.root);
    // No console on Windows: without this every call opens a window.
    yuzora_core::spawn::no_window(&mut c);
    c.stdin(if call.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| GitError::Spawn(format!("cannot run {}: {e}", local.git)))?;
    if let Some(input) = call.stdin.clone() {
        let mut w = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || {
            let _ = w.write_all(&input);
        });
    }
    let out = Stream::start(child.stdout.take().expect("piped stdout"));
    let err = Stream::start(child.stderr.take().expect("piped stderr"));
    let start = Instant::now();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    loop {
        while let Ok(c) = out.rx.try_recv() {
            let room = call.max_output.saturating_sub(stdout.len());
            stdout.extend_from_slice(&c[..c.len().min(room)]);
        }
        while let Ok(c) = err.rx.try_recv() {
            stderr.extend_from_slice(&c);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let until = Instant::now() + PIPE_GRACE;
            while Instant::now() < until && !(out.ended.load(Ordering::SeqCst) && err.ended.load(Ordering::SeqCst)) {
                let _ = out.wake.recv_timeout(POLL);
            }
            stdout.extend(out.rx.try_iter().flatten());
            stderr.extend(err.rx.try_iter().flatten());
            return Ok(Output { code: status.code().unwrap_or(-1), stdout, stderr });
        }
        if call.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Cancelled);
        }
        if start.elapsed() > call.timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::TimedOut { seconds: call.timeout.as_secs_f64() });
        }
        let _ = out.wake.recv_timeout(POLL);
    }
}

/// Runs `work` on its own thread and resolves with its answer: git calls
/// never run on the UI thread, and a long one never holds a pool thread.
pub fn spawn<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Option<T>> + Send + 'static {
    let (tx, rx) = async_channel::bounded(1);
    let started = std::thread::Builder::new().name(format!("yuzora-git-{name}")).spawn(move || {
        let _ = tx.send_blocking(work());
    });
    async move {
        started.ok()?;
        rx.recv().await.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_carry_their_environment() {
        let read = environment(Kind::Read, false, &[]);
        assert!(read.contains(&("GIT_OPTIONAL_LOCKS".into(), "0".into())));
        assert!(read.contains(&("GIT_LITERAL_PATHSPECS".into(), "1".into())));
        assert!(read.contains(&("LANGUAGE".into(), String::new())));
        let write = environment(Kind::Write, true, &[("GIT_EDITOR".into(), "helper".into())]);
        assert!(!write.iter().any(|(k, _)| k == "GIT_OPTIONAL_LOCKS"));
        assert_eq!(write.iter().filter(|(k, _)| k == "GIT_EDITOR").count(), 1);
        assert!(write.contains(&("GIT_EDITOR".into(), "helper".into())));
        assert!(write.contains(&("LC_ALL".into(), "C.UTF-8".into())));
        let base = base_args(Kind::Read);
        assert!(base.windows(2).any(|w| w == ["-c", "core.quotepath=false"]));
        assert!(base.windows(2).any(|w| w == ["-c", "log.showSignature=false"]));
        assert!(base.windows(2).any(|w| w == ["-c", "core.fsmonitor=false"]));
        assert!(!base_args(Kind::Write).iter().any(|a| a.starts_with("core.fsmonitor")));
    }

    #[test]
    fn long_pathspecs_go_through_a_file() {
        let short = Git::write(["add"]).pathspec(&["a.txt".into(), "b c.txt".into()]);
        assert_eq!(short.args, ["add", "--", "a.txt", "b c.txt"]);
        assert!(short.stdin.is_none());
        let many: Vec<String> = (0..2000).map(|i| format!("dir/file-{i}.txt")).collect();
        let long = Git::write(["add"]).pathspec(&many);
        assert_eq!(long.args, ["add", "--pathspec-from-file=-", "--pathspec-file-nul"]);
        let input = long.stdin.unwrap();
        assert_eq!(input.iter().filter(|b| **b == 0).count(), 2000);
        let (all, kept) = Git::read(["status"]).paths_or_all(&many);
        assert!(!kept);
        assert_eq!(all.args, ["status"]);
    }

    // The ceiling is a list git splits at `:`, or `;` on Windows, and either kind of git may run
    // (a machine may be the other kind): a folder above with either in it (but a drive's `C:`)
    // gets no environment at all (never `GIT_DIR`: a hook would inherit it), and every command
    // first asks git which repo it finds there. Windows paths keep the ceiling, here or on a machine.
    #[test]
    fn a_top_place_is_kept_in_its_repo_whichever_git_runs() {
        let at = |root: &str, on: Option<&str>| Place { root: root.into(), on: on.map(Into::into), git: "git".into(), top: true, git_dir: String::new(), write_lock: Arc::default() };
        let ceiling = |above: &str| vec![("GIT_CEILING_DIRECTORIES".to_owned(), above.to_owned())];
        for on in [None, Some("lab")] {
            for (root, above) in [("/srv/repo", "/srv"), ("/repo", "/"), ("C:/Users/x/repo", "C:/Users/x"), ("C:/repo", "C:/"), ("//host/share/repo", "//host/share"), ("/srv/repo/", "/srv"), ("/srv/re\\po", "/srv"), ("/srv/a/re\\po/", "/srv/a")] {
                assert_eq!(at(root, on).confine(), ceiling(above), "{root} {on:?}");
                assert!(!at(root, on).needs_git_check(), "{root} {on:?}");
            }
            for root in ["/srv/a:b/repo", "/srv/a;b/repo", "C:/a;b/repo", "C:/x/a:b/repo", "//host/sh:are/repo"] {
                assert!(at(root, on).confine().is_empty(), "{root} {on:?}: nothing, GIT_DIR least");
                assert!(at(root, on).needs_git_check(), "{root} {on:?}");
            }
            for root in ["/", "C:/"] {
                assert!(at(root, on).confine().is_empty() && !at(root, on).needs_git_check(), "nothing above {root}");
            }
        }
        // A `:` or `;` in the folder itself is no list entry: only the one above is.
        assert_eq!(at("/srv/a:b", None).confine(), ceiling("/srv"));
        assert!(Place::local("/srv/repo").confine().is_empty() && !Place::local("/srv/a:b/repo").needs_git_check(), "not a top level");
        for (p, sep) in [("C:/x", false), ("C:/", false), ("c:\\x", false), ("/a:b", true), ("C:/a;b", true), ("a:b", false), ("ab:c", true), ("/srv", false)] {
            assert_eq!(has_list_separator(p), sep, "{p}");
        }
        // git writes `/` between names, also on Windows: a `\` is part of a unix name.
        for (root, up) in [("C:/repo", Some("C:/")), ("C:\\repo", None), ("/srv/re\\po", Some("/srv")), ("/srv/a:b/re\\po", Some("/srv/a:b")), ("C:/", None), ("/", None), ("/a", Some("/")), ("repo", None)] {
            assert_eq!(above(root).as_deref(), up, "{root}");
        }
    }

    fn plain_git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git").args(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "-c", "protocol.file.allow=always"]).args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// What the check says when git finds no repository at the folder.
    const NOT_THERE: &str = "finds no repository at";

    fn real(p: &std::path::Path) -> String {
        yuzora_core::paths::plain_str(&std::fs::canonicalize(p).unwrap().to_string_lossy())
    }

    fn commit_of(repo: &crate::repo::Repo, msg: &str) -> GitResult<Output> {
        repo.run(Git::write(["-c", "user.name=a", "-c", "user.email=a@b", "-c", "commit.gpgSign=false", "commit", "--allow-empty", "-q", "-m", msg]))
    }

    // An open repo is its top level only, with the git this computer has (Windows CI runs
    // `exec::`): with its `.git` gone and a repo around it, git fails rather than reading or
    // writing that repo under this one's name. Status has none of its branch (the trust gate
    // can't read this repo's config, so it asks, keeping what this one's status said last; or git
    // says there's no repo), a commit fails; while the `.git` is there git works as ever. Opening
    // the folder then finds the repo around it, as git looks. On unix also with a `:` in the
    // folder above and on a machine: no ceiling there (git splits the list at it), so every
    // command, read or write, first asks git which repo it finds there.
    #[test]
    fn an_open_repo_whose_git_is_gone_runs_nothing_in_the_repo_around_it() {
        use crate::repo::{Repo, open};
        let mut below = vec!["inner"];
        if cfg!(unix) {
            below.push("a:b/inner");
        }
        for rel in below {
            let colon = rel.contains(':');
            let dir = tempfile::tempdir().unwrap();
            let outer = real(dir.path());
            plain_git(dir.path(), &["init", "-q", "-b", "main"]);
            plain_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "outer"]);
            let head = plain_git(dir.path(), &["rev-parse", "HEAD"]);
            let inner_path = dir.path().join(rel);
            std::fs::create_dir_all(&inner_path).unwrap();
            let inner = real(&inner_path);
            plain_git(&inner_path, &["init", "-q", "-b", "main"]);
            let repo = open(&inner, None).unwrap();
            assert_eq!(real(std::path::Path::new(&repo.place.root)), inner, "{rel}");
            let mut repos = vec![("here", repo.clone())];
            #[cfg(unix)]
            {
                let _ = crate::repo::testing::on_machine(&crate::repo::testing::scratch());
                let place = Place { on: Some("test-machine".into()), write_lock: Arc::default(), ..repo.place.clone() };
                repos.push(("on a machine", Arc::new(Repo::new(crate::repo::repo_id(Some("test-machine"), &repo.place.root), place, repo.git_dir.clone(), repo.common_dir.clone()))));
            }
            for (where_, repo) in &repos {
                let out = commit_of(repo, "inner").unwrap();
                assert!(out.ok(), "{rel} {where_}: {}", out.stderr_text());
                let st = crate::status::read(repo, true).unwrap();
                assert_eq!(st["branch"]["head"], "main", "{rel} {where_}: {st}");
            }
            std::fs::rename(inner_path.join(".git"), inner_path.join(".git-away")).unwrap();
            for (where_, repo) in &repos {
                let status = crate::status::read(repo, true);
                let none = match &status {
                    Err(e) => e.to_string().contains("not a git repository") || e.to_string().contains(NOT_THERE),
                    Ok(st) => st["branch"]["oid"] != head.as_str() && st["trust"].as_array().is_some_and(|t| !t.is_empty()),
                };
                assert!(none, "{rel} {where_}: status {status:?}");
                let out = commit_of(repo, "inner");
                if colon {
                    assert!(out.as_ref().is_err_and(|e| e.to_string().contains(NOT_THERE)), "{rel} {where_}: {out:?}");
                } else {
                    assert!(out.is_ok_and(|o| !o.ok()), "{rel} {where_}: committed");
                }
            }
            assert_eq!(plain_git(dir.path(), &["rev-parse", "HEAD"]), head, "{rel}: the repo around it got a commit");
            assert_eq!(real(std::path::Path::new(&open(&inner, None).unwrap().place.root)), outer, "{rel}");
        }
    }

    // A hook inherits what the engine sets, and for a plain repo that may be a ceiling and
    // nothing else: git sets no `GIT_DIR` or `GIT_WORK_TREE` there, so a hook that goes
    // somewhere else must not find one (it would run git in this repo, with that folder as the
    // work tree). With a `:` above, none of the three. A hook that does `cd frontend && git add
    // -u` (a formatter's) leaves the commit whole.
    #[cfg(unix)]
    #[test]
    fn a_hook_gets_the_ceiling_and_never_a_git_dir_and_a_monorepo_hook_deletes_nothing() {
        use crate::repo::open;
        use std::os::unix::fs::PermissionsExt;
        for rel in ["mono", "a:b/mono"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.join("frontend")).unwrap();
            std::fs::create_dir_all(path.join("backend")).unwrap();
            plain_git(&path, &["init", "-q", "-b", "main"]);
            for (f, t) in [("README", "r"), ("frontend/a.js", "a"), ("backend/b.py", "b")] {
                std::fs::write(path.join(f), t).unwrap();
            }
            plain_git(&path, &["add", "."]);
            plain_git(&path, &["commit", "-q", "-m", "base"]);
            let seen = path.join("seen-env");
            let hook = path.join(".git/hooks/pre-commit");
            std::fs::write(&hook, format!("#!/bin/sh\nenv | grep '^GIT_' | sort > '{}'\ncd frontend && echo changed > a.js && git add -u\n", seen.display())).unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(path.join("README"), "r2").unwrap();
            plain_git(&path, &["add", "README"]);
            let repo = open(&real(&path), None).unwrap();
            let out = commit_of(&repo, "with hook").unwrap();
            assert!(out.ok(), "{rel}: {}", out.stderr_text());
            let env = std::fs::read_to_string(&seen).unwrap();
            let names: Vec<&str> = env.lines().filter_map(|l| l.split('=').next()).collect();
            for bad in ["GIT_DIR", "GIT_WORK_TREE"] {
                assert!(!names.contains(&bad), "{rel}: the hook got {bad}: {env}");
            }
            assert_eq!(names.contains(&"GIT_CEILING_DIRECTORIES"), !rel.contains(':'), "{rel}: {env}");
            let files = plain_git(&path, &["ls-tree", "-r", "--name-only", "HEAD"]);
            assert_eq!(files.lines().collect::<Vec<_>>(), ["README", "backend/b.py", "frontend/a.js"], "{rel}: the commit lost files");
            assert_eq!(plain_git(&path, &["show", "HEAD:frontend/a.js"]), "changed", "{rel}: the hook's own add");
        }
    }

    // A linked worktree and a submodule (their `.git` is a file) are open repos like any: git
    // works there with the ceiling, and under a `:` with git's own answer to the check.
    #[test]
    fn a_linked_worktree_and_a_submodule_work_with_and_without_a_ceiling() {
        use crate::repo::open;
        // A `:` in a name is no Windows folder (`a:b` there is a drive's relative path).
        let mut below = vec!["x"];
        if cfg!(unix) {
            below.push("a:b/x");
        }
        for rel in below {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().join(rel);
            let (main, other, wt) = (base.join("main"), base.join("other"), base.join("wt"));
            std::fs::create_dir_all(&main).unwrap();
            std::fs::create_dir_all(&other).unwrap();
            for p in [&main, &other] {
                plain_git(p, &["init", "-q", "-b", "main"]);
                plain_git(p, &["commit", "--allow-empty", "-q", "-m", "first"]);
            }
            plain_git(&main, &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()]);
            plain_git(&main, &["submodule", "add", "-q", other.to_str().unwrap(), "sub"]);
            plain_git(&main, &["commit", "-q", "-m", "sub"]);
            for place in [&wt, &main.join("sub")] {
                assert!(place.join(".git").is_file(), "{rel}: {place:?}");
                let repo = open(&real(place), None).unwrap();
                assert_eq!(real(std::path::Path::new(&repo.place.root)), real(place), "{rel}");
                assert!(repo.place.top);
                let out = commit_of(&repo, "here").unwrap();
                assert!(out.ok(), "{rel} {place:?}: {}", out.stderr_text());
                let st = crate::status::read(&repo, true).unwrap();
                assert!(st["branch"]["head"].is_string(), "{rel} {place:?}: {st}");
                // And on a machine: its gitfile is found there too.
                #[cfg(unix)]
                {
                    use crate::repo::Repo;
                    let _ = crate::repo::testing::on_machine(&crate::repo::testing::scratch());
                    let there = Place { on: Some("test-machine".into()), write_lock: Arc::default(), ..repo.place.clone() };
                    let there = Arc::new(Repo::new(crate::repo::repo_id(Some("test-machine"), &there.root), there, repo.git_dir.clone(), repo.common_dir.clone()));
                    let out = commit_of(&there, "there").unwrap();
                    assert!(out.ok(), "{rel} {place:?} on a machine: {}", out.stderr_text());
                }
            }
            let repo = open(&real(&main), None).unwrap();
            let out = repo.run(Git::read(["status", "--porcelain=v2", "--ignore-submodules=none"])).unwrap();
            assert!(out.ok(), "{rel}: {}", out.stderr_text());
            let out = repo.run(Git::read(["submodule", "status"])).unwrap();
            assert!(out.ok() && out.stdout_text().contains("sub"), "{rel}: {}", out.stderr_text());
            // Its gitfile gone: a command is refused, not run in the repo around it.
            if rel.contains(':') {
                let repo = open(&real(&wt), None).unwrap();
                std::fs::remove_file(wt.join(".git")).unwrap();
                assert!(commit_of(&repo, "gone").is_err_and(|e| e.to_string().contains(NOT_THERE)));
                assert!(repo.run(Git::read(["status"])).is_err_and(|e| e.to_string().contains(NOT_THERE)));
            }
        }
    }

    /// A small executable script (a hook, a filter, an uploadpack).
    #[cfg(unix)]
    fn script(at: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(at, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// `repo` as one on a machine (the stand-in `hc` answers there).
    #[cfg(unix)]
    fn on_a_machine(repo: &Arc<crate::repo::Repo>) -> Arc<crate::repo::Repo> {
        let _ = crate::repo::testing::on_machine(&crate::repo::testing::scratch());
        let place = Place { on: Some("test-machine".into()), write_lock: Arc::default(), ..repo.place.clone() };
        Arc::new(crate::repo::Repo::new(crate::repo::repo_id(Some("test-machine"), &place.root), place, repo.git_dir.clone(), repo.common_dir.clone()))
    }

    // With a `:` above there is no ceiling, so the `.git` gone sends git up to the repo around
    // it: not only a write (a commit) but a fetch, a push, a tag push and a remote tag's deletion
    // (reads to the engine, they write refs, run hooks and the transport of that repo), and a
    // status (which runs that repo's clean filters once this one is trusted). Every command asks
    // git first which repo it finds, so none of them reaches the repo around it.
    #[cfg(unix)]
    #[test]
    fn with_a_colon_above_no_command_reaches_the_repo_around_an_open_one() {
        use crate::repo::open;
        let dir = tempfile::tempdir().unwrap();
        let up_dir = tempfile::tempdir().unwrap();
        let marks = tempfile::tempdir().unwrap();
        let d = std::path::PathBuf::from(real(dir.path()));
        let up = std::path::PathBuf::from(real(up_dir.path())).join("up.git");
        let (m_hook, m_clean, m_upload) = (marks.path().join("hook"), marks.path().join("clean"), marks.path().join("upload"));
        plain_git(&d, &["init", "-q", "--bare", up.to_str().unwrap()]);
        plain_git(&d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join(".gitattributes"), "f filter=evil\n").unwrap();
        std::fs::write(d.join("f"), "x\n").unwrap();
        plain_git(&d, &["add", ".gitattributes", "f"]);
        plain_git(&d, &["commit", "-q", "-m", "outer1"]);
        let outer1 = plain_git(&d, &["rev-parse", "HEAD"]);
        plain_git(&d, &["remote", "add", "origin", up.to_str().unwrap()]);
        plain_git(&d, &["push", "-q", "-u", "origin", "main"]);
        plain_git(&d, &["tag", "t1"]);
        plain_git(&d, &["commit", "--allow-empty", "-q", "-m", "outer2-unpushed"]);
        plain_git(&up, &["update-ref", "refs/heads/feature", &outer1]);
        plain_git(&up, &["tag", "t2", &outer1]);
        script(&d.join(".git/hooks/pre-push"), &format!("touch '{}'", m_hook.display()));
        let upload = marks.path().join("upload.sh");
        script(&upload, &format!("touch '{}'\nexec git-upload-pack \"$@\"", m_upload.display()));
        plain_git(&d, &["config", "filter.evil.clean", &format!("touch '{}'; cat", m_clean.display())]);
        plain_git(&d, &["config", "remote.origin.uploadpack", upload.to_str().unwrap()]);
        // The same content under another time: status reads it through the clean filter.
        std::fs::File::options().write(true).open(d.join("f")).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800)).unwrap();
        let inner = d.join("a:b/inner");
        std::fs::create_dir_all(&inner).unwrap();
        plain_git(&inner, &["init", "-q", "-b", "main"]);
        plain_git(&inner, &["commit", "--allow-empty", "-q", "-m", "inner"]);
        let repo = open(&real(&inner), None).unwrap();
        let repos = [("here", repo.clone()), ("on a machine", on_a_machine(&repo))];
        for (_, r) in &repos {
            crate::trust::trust(r).unwrap();
            assert!(crate::status::read(r, true).unwrap().get("trust").is_none());
        }
        assert!(!m_clean.exists() && !m_upload.exists(), "setup ran a program");
        std::fs::rename(inner.join(".git"), inner.join(".git-away")).unwrap();
        let up_main = plain_git(&up, &["rev-parse", "refs/heads/main"]);
        for (where_, r) in &repos {
            let refused = |out: GitResult<Output>, what: &str| assert!(out.as_ref().is_err_and(|e| e.to_string().contains(NOT_THERE)), "{where_} {what}: {out:?}");
            refused(r.run(Git::read(["fetch", "origin"])), "fetch");
            refused(r.run(Git::read(["fetch", "--all", "--prune"])), "auto fetch");
            refused(r.run(Git::read(["push", "origin", "main"])), "push");
            refused(r.run(Git::read(["push", "origin", "refs/tags/t1"])), "tag push");
            refused(r.run(Git::read(["push", "origin", ":refs/tags/t2"])), "remote tag delete");
            refused(r.run(Git::read(["push", "origin", "--delete", "feature"])), "remote branch delete");
            refused(r.run(Git::read(["status", "--porcelain=v2", "--branch"])), "status");
            refused(commit_of(r, "inner"), "commit");
            assert!(crate::status::read(r.as_ref(), true).is_err(), "{where_}: status shows the repo around it");
            assert!(!m_hook.exists() && !m_clean.exists() && !m_upload.exists(), "{where_}: a program of the repo around it ran");
        }
        assert_eq!(plain_git(&up, &["rev-parse", "refs/heads/main"]), up_main, "the upstream of the repo around it moved");
        assert_eq!(plain_git(&up, &["rev-parse", "refs/heads/feature"]), outer1);
        assert_eq!(plain_git(&up, &["rev-parse", "t2^{commit}"]), outer1, "its tag was deleted");
        let remote_refs = plain_git(&d, &["for-each-ref", "refs/remotes"]);
        assert!(!remote_refs.contains("feature"), "the repo around it fetched: {remote_refs}");
        assert!(plain_git(&d, &["log", "-1", "--format=%s"]).starts_with("outer2"), "the repo around it got a commit");
    }

    // The repo around the folder names it as its work tree (`core.worktree`: absolute, relative
    // to its git dir, in another case): git then answers the folder itself for the top level, and
    // the command would run in the outer repo (its HEAD, its pre-commit hook). So does a `.git`
    // swapped for a gitfile naming the outer git dir. Only the git dir tells them from this repo.
    #[cfg(unix)]
    #[test]
    fn a_repo_around_that_names_the_folder_as_its_work_tree_gets_no_command() {
        use crate::repo::open;
        for how in ["absolute", "relative", "other case", "gitfile"] {
            let dir = tempfile::tempdir().unwrap();
            let marks = tempfile::tempdir().unwrap();
            let d = std::path::PathBuf::from(real(dir.path()));
            let m_hook = marks.path().join("hook");
            plain_git(&d, &["init", "-q", "-b", "main"]);
            plain_git(&d, &["commit", "--allow-empty", "-q", "-m", "outer"]);
            let head = plain_git(&d, &["rev-parse", "HEAD"]);
            script(&d.join(".git/hooks/pre-commit"), &format!("touch '{}'", m_hook.display()));
            let inner = d.join("a:b/inner");
            std::fs::create_dir_all(&inner).unwrap();
            plain_git(&inner, &["init", "-q", "-b", "main"]);
            plain_git(&inner, &["commit", "--allow-empty", "-q", "-m", "inner"]);
            let repo = open(&real(&inner), None).unwrap();
            let there = on_a_machine(&repo);
            assert!(repo.place.needs_git_check() && !repo.place.git_dir.is_empty());
            for r in [&repo, &there] {
                crate::trust::trust(r).unwrap();
                assert!(commit_of(r, "inner").unwrap().ok(), "{how}: the open repo works");
            }
            match how {
                "absolute" => plain_git(&d, &["config", "core.worktree", &repo.place.root]),
                "relative" => plain_git(&d, &["config", "core.worktree", "../a:b/inner"]),
                "other case" => plain_git(&d, &["config", "core.worktree", &repo.place.root.replace("/inner", "/INNER")]),
                _ => String::new(),
            };
            std::fs::rename(inner.join(".git"), inner.join(".git-away")).unwrap();
            if how == "gitfile" {
                std::fs::write(inner.join(".git"), format!("gitdir: {}\n", d.join(".git").display())).unwrap();
            }
            for (where_, r) in [("here", &repo), ("on a machine", &there)] {
                for kind in [Kind::Write, Kind::Read] {
                    let cmd = if kind == Kind::Write { Git::write(["commit", "--allow-empty", "-q", "-m", "landed"]) } else { Git::read(["status", "--porcelain=v2"]) };
                    let out = r.run(cmd);
                    assert!(out.as_ref().is_err_and(|e| e.to_string().contains(NOT_THERE)), "{how} {where_} {kind:?}: {out:?}");
                }
                assert_eq!(plain_git(&d, &["rev-parse", "HEAD"]), head, "{how} {where_}: the repo around it got a commit");
                assert!(!m_hook.exists(), "{how} {where_}: its hook ran");
            }
        }
    }

    // A write asks git which repo it finds only once it holds the repo's write lock: another
    // write (ours) may be what moves the `.git` away, and an answer taken before waiting for it
    // would be stale by the time the command runs. (What the lock can't order: a removal from
    // outside, between the check and the command.)
    #[cfg(unix)]
    #[test]
    fn a_write_asks_git_which_repo_it_finds_only_after_it_has_the_write_lock() {
        use crate::repo::{Repo, open};
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let inner = std::path::PathBuf::from(real(dir.path())).join("a:b/inner");
        std::fs::create_dir_all(&inner).unwrap();
        plain_git(&inner, &["init", "-q", "-b", "main"]);
        let base = open(&real(&inner), None).unwrap();
        crate::trust::trust(&base).unwrap();
        let (log, wrapper) = (tools.path().join("log"), tools.path().join("git"));
        script(&wrapper, &format!("printf '%s\\n' \"$*\" >> '{}'\nexec git \"$@\"", log.display()));
        let place = Place { git: wrapper.to_string_lossy().into_owned(), ..base.place.clone() };
        let repo = Arc::new(Repo::new(base.id.clone(), place.clone(), base.git_dir.clone(), base.common_dir.clone()));
        let asked = || std::fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains("rev-parse")).count();
        let held = place.write_lock.lock().unwrap();
        let r2 = repo.clone();
        let writer = std::thread::spawn(move || r2.run(Git::write(["status"])));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(asked(), 0, "git was asked before the write lock was held");
        drop(held);
        assert!(writer.join().unwrap().unwrap().ok());
        assert_eq!(asked(), 1);
    }

    // The `.git` is there but git doesn't take it for a repository (`HEAD` empty or gone, no
    // `objects` or `refs`, a folder with nothing in it, a half done `rm -rf`) and goes up: only
    // git's own answer says so.
    #[cfg(unix)]
    #[test]
    fn a_git_dir_git_rejects_is_no_repo_here_whatever_is_in_it() {
        use crate::repo::open;
        for how in ["empty HEAD", "no HEAD", "empty .git folder", "no objects", "no refs"] {
            let dir = tempfile::tempdir().unwrap();
            let d = std::path::PathBuf::from(real(dir.path()));
            plain_git(&d, &["init", "-q", "-b", "main"]);
            plain_git(&d, &["commit", "--allow-empty", "-q", "-m", "outer"]);
            let head = plain_git(&d, &["rev-parse", "HEAD"]);
            let inner = d.join("a:b/inner");
            std::fs::create_dir_all(&inner).unwrap();
            plain_git(&inner, &["init", "-q", "-b", "main"]);
            plain_git(&inner, &["commit", "--allow-empty", "-q", "-m", "inner"]);
            let repo = open(&real(&inner), None).unwrap();
            let there = on_a_machine(&repo);
            match how {
                "empty HEAD" => std::fs::write(inner.join(".git/HEAD"), "").unwrap(),
                "no HEAD" => std::fs::remove_file(inner.join(".git/HEAD")).unwrap(),
                "empty .git folder" => {
                    std::fs::remove_dir_all(inner.join(".git")).unwrap();
                    std::fs::create_dir(inner.join(".git")).unwrap();
                }
                "no objects" => std::fs::remove_dir_all(inner.join(".git/objects")).unwrap(),
                _ => std::fs::remove_dir_all(inner.join(".git/refs")).unwrap(),
            }
            for (where_, r) in [("here", &repo), ("on a machine", &there)] {
                let out = commit_of(r, "landed");
                assert!(out.as_ref().is_err_and(|e| e.to_string().contains(NOT_THERE)), "{how} {where_}: {out:?}");
                assert_eq!(plain_git(&d, &["rev-parse", "HEAD"]), head, "{how} {where_}: the repo around it got a commit");
            }
        }
    }

    // A unix name may hold a `\`: the folder above is cut at `/` only, so the ceiling holds
    // (and with a `:` above, the check runs).
    #[cfg(unix)]
    #[test]
    fn a_backslash_in_the_name_of_the_repo_folder_keeps_the_ceiling() {
        use crate::repo::open;
        for rel in ["re\\po", "a:b/re\\po"] {
            let dir = tempfile::tempdir().unwrap();
            let d = std::path::PathBuf::from(real(dir.path()));
            plain_git(&d, &["init", "-q", "-b", "main"]);
            plain_git(&d, &["commit", "--allow-empty", "-q", "-m", "outer"]);
            let head = plain_git(&d, &["rev-parse", "HEAD"]);
            let inner = d.join(rel);
            std::fs::create_dir_all(&inner).unwrap();
            plain_git(&inner, &["init", "-q", "-b", "main"]);
            plain_git(&inner, &["commit", "--allow-empty", "-q", "-m", "inner"]);
            let repo = open(&real(&inner), None).unwrap();
            let there = on_a_machine(&repo);
            let up = real(inner.parent().unwrap());
            assert_eq!(repo.place.confine().first().map(|(_, v)| v.clone()), (!rel.contains(':')).then_some(up), "{rel}");
            std::fs::rename(inner.join(".git"), inner.join(".git-away")).unwrap();
            for (where_, r) in [("here", &repo), ("on a machine", &there)] {
                let out = commit_of(r, "landed");
                assert!(out.is_err() || out.is_ok_and(|o| !o.ok()), "{rel} {where_}: committed");
                assert_eq!(plain_git(&d, &["rev-parse", "HEAD"]), head, "{rel} {where_}: the repo around it got a commit");
            }
        }
    }

    // The check is cancelled with the command, also where the machine doesn't answer.
    #[cfg(unix)]
    #[test]
    fn the_check_on_a_machine_that_does_not_answer_is_cancelled_with_the_command() {
        use crate::repo::open;
        let dir = tempfile::tempdir().unwrap();
        let d = std::path::PathBuf::from(real(dir.path()));
        let inner = d.join("a:b/inner");
        std::fs::create_dir_all(&inner).unwrap();
        plain_git(&inner, &["init", "-q", "-b", "main"]);
        let there = on_a_machine(&open(&real(&inner), None).unwrap());
        assert!(there.place.needs_git_check());
        std::fs::write(inner.join(".hc-hang"), "").unwrap();
        for kind in [Kind::Write, Kind::Read] {
            let cancel = Cancel::new();
            let c2 = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                c2.cancel();
            });
            let started = Instant::now();
            let cmd = if kind == Kind::Write { Git::write(["status"]) } else { Git::read(["status"]) };
            let out = there.run(cmd.cancel(Some(cancel)));
            assert_eq!(out, Err(GitError::Cancelled), "{kind:?}");
            assert!(started.elapsed() < Duration::from_secs(5), "{kind:?}: took {:?}", started.elapsed());
        }
        // A write waiting behind it isn't stuck: the lock is free again.
        std::fs::remove_file(inner.join(".hc-hang")).unwrap();
        assert!(there.run(Git::write(["status"])).unwrap().ok());
    }

    // Which folder git found is the folder: by real path here (a link, a trailing `/`, case),
    // as written on a machine, with Windows' forms (`\\?\`, `\`, case, `C:/`) the same one.
    #[test]
    fn the_folder_git_found_is_the_folder_or_it_is_not() {
        for (found, root) in [("/srv/repo", "/srv/repo"), ("/srv/repo", "/srv/repo/"), ("/", "/"), ("C:/x/repo", "C:/x/repo"), ("C:/x/repo", r"C:\x\repo"), ("c:/x/repo", "C:/X/Repo"), (r"\\?\C:\x\repo", "C:/x/repo"), ("C:/", "C:/"), ("C:/", r"C:\"), ("//host/share/repo", r"\\host\share\repo")] {
            assert!(same_folder(found, root, true), "{found} = {root}");
        }
        for (found, root) in [("/srv", "/srv/repo"), ("", "/srv/repo"), ("", ""), ("/srv/repo/sub", "/srv/repo"), ("C:/", "C:/x"), ("/", "/srv"), ("/srv/re/po", r"/srv/re\po"), ("/srv/repo2", "/srv/repo"), ("/Srv/Repo", "/srv/repo"), ("/srv/a:b/Repo", "/srv/a:b/repo"), ("/srv/a:b/INNER", "/srv/a:b/inner")] {
            assert!(!same_folder(found, root, true), "{found} != {root}");
        }
        // Here: by real path, and a unix path keeps its case.
        let dir = tempfile::tempdir().unwrap();
        let real_dir = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(real_dir.join("repo")).unwrap();
        let (found, root) = (real_dir.join("repo").to_string_lossy().into_owned(), format!("{}/", dir.path().join("repo").display()));
        assert!(same_folder(&found, &root, false), "{found} = {root}");
        assert!(!same_folder(&real_dir.to_string_lossy(), &found, false));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(real_dir.join("repo"), real_dir.join("link")).unwrap();
            assert!(same_folder(&found, &real_dir.join("link").to_string_lossy(), false), "a link to it");
            assert!(!same_folder(&found, &real_dir.join("link").to_string_lossy(), true), "on a machine: as written");
        }
        // A file system that ignores case gives the case it has on disk (what the work tree of
        // a repo around spelled differently comes to); one that doesn't, two folders.
        std::fs::create_dir(real_dir.join("Case")).unwrap();
        let other = real_dir.join("case");
        if other.exists() {
            assert_eq!(std::fs::canonicalize(&other).unwrap(), real_dir.join("Case"));
            assert!(same_folder(&other.to_string_lossy(), &real_dir.join("Case").to_string_lossy(), false), "one folder, spelled in two cases");
        } else {
            assert!(!same_folder(&other.to_string_lossy(), &real_dir.join("Case").to_string_lossy(), false), "two folders");
        }
    }

    #[test]
    fn remote_commands_go_through_hc_exec() {
        let place = Place { root: "/srv/repo".into(), on: Some("lab-01".into()), git: "git".into(), top: false, git_dir: String::new(), write_lock: Arc::default() };
        let a = hc_args(&place, &Git::read(["status", "-z"]).stdin(b"x".to_vec()).timeout(Duration::from_secs(9000)));
        assert_eq!(&a[..8], ["exec", "--json", "--on", "lab-01", "--timeout", "600", "--cwd", "/srv/repo"]);
        assert!(a.windows(2).any(|w| w == ["--env", "GIT_OPTIONAL_LOCKS=0"]));
        // Looking for a repo from a folder: up as far as git goes. In an open one: its top only.
        assert!(!a.iter().any(|x| x.starts_with("GIT_CEILING_DIRECTORIES=")));
        let top = hc_args(&Place { top: true, ..place.clone() }, &Git::read(["status"]));
        assert!(top.windows(2).any(|w| w == ["--env", "GIT_CEILING_DIRECTORIES=/srv"]), "{top:?}");
        let root = Place { root: "/".into(), top: true, ..place.clone() };
        assert!(!hc_args(&root, &Git::read(["status"])).iter().any(|x| x.starts_with("GIT_CEILING_DIRECTORIES=")), "nothing above /");
        // A `:` above: no ceiling git would split there, and nothing else in its place.
        let colon = hc_args(&Place { root: "/srv/a:b/repo".into(), top: true, ..place.clone() }, &Git::read(["status"]));
        assert!(!colon.iter().any(|x| x.starts_with("GIT_CEILING_DIRECTORIES=") || x.starts_with("GIT_DIR=") || x.starts_with("GIT_WORK_TREE=")), "{colon:?}");
        let dashes = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(a[dashes - 1], "--stdin");
        assert_eq!(a[dashes + 1], "git");
        assert_eq!(&a[a.len() - 2..], ["status", "-z"]);
    }

    #[cfg(unix)]
    #[test]
    fn local_calls_capture_stream_cancel_and_time_out() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::local(dir.path().to_string_lossy());
        let out = run(&place, Git::read(["--version"])).unwrap();
        assert!(out.ok());
        assert!(out.stdout_text().starts_with("git version"));
        let mut seen = Vec::new();
        let mut sink = |b: &[u8]| {
            seen.extend_from_slice(b);
            true
        };
        run_streaming(&place, Git::read(["--version"]), Some(&mut sink)).unwrap();
        assert!(String::from_utf8_lossy(&seen).starts_with("git version"));
        let fails = run(&place, Git::read(["rev-parse", "--verify", "no-such-ref"])).unwrap();
        assert!(!fails.ok());
        assert!(matches!(fails.into_result(), Err(GitError::Failed { .. })));
        // stdin reaches git.
        let hashed = run(&place, Git::read(["hash-object", "--stdin"]).stdin(b"hello\n".to_vec())).unwrap().text().unwrap();
        assert_eq!(hashed.trim(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_or_cancelled_call_is_killed_with_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut place = Place::local(dir.path().to_string_lossy());
        // An alias that sleeps stands in for a slow git (a hook, a fetch).
        let started = Instant::now();
        let slow = run(&place, Git::read(["-c", "alias.nap=!sleep 5", "nap"]).timeout(Duration::from_millis(300)));
        assert!(matches!(slow, Err(GitError::TimedOut { .. })), "{slow:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c2.cancel();
        });
        let started = Instant::now();
        let stopped = run(&place, Git::read(["-c", "alias.nap=!sleep 5", "nap"]).cancel(Some(cancel)));
        assert_eq!(stopped, Err(GitError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(3));
        // A write waits for the lock, and gives up when cancelled.
        place.write_lock = Arc::new(Mutex::new(()));
        let held = place.write_lock.lock().unwrap();
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(run(&place, Git::write(["--version"]).cancel(Some(cancel))), Err(GitError::Cancelled));
        drop(held);
        assert!(run(&place, Git::write(["--version"])).unwrap().ok());
    }

    #[test]
    fn a_cancelled_call_leaves_nothing_it_started_running() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::local(dir.path().to_string_lossy());
        // git starts a shell that writes a file a second later, unless the
        // cancel ended it too (a hook, ssh or a credential helper).
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            c2.cancel();
        });
        let stopped = run(&place, Git::read(["-c", "alias.late=!sleep 1; echo late > late.txt", "late"]).cancel(Some(cancel)));
        assert_eq!(stopped, Err(GitError::Cancelled));
        std::thread::sleep(Duration::from_millis(3500));
        assert!(!dir.path().join("late.txt").exists(), "what git started outlived the cancel");
    }

    /// `run_hc` (a remote repository's git, through `hc`) starts its program
    /// without a console window: here the program is this test binary asking
    /// whether it has one. The control starts it by hand and must see ours.
    #[cfg(windows)]
    #[test]
    fn run_hc_starts_its_program_without_a_console_window() {
        unsafe extern "system" {
            fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            fn AllocConsole() -> i32;
        }
        // The runner may have no console (CI): give it one, to hand on.
        let attached = || {
            let mut list = [0u32; 4];
            // SAFETY: a buffer of the length it is given.
            unsafe { GetConsoleProcessList(list.as_mut_ptr(), 4) != 0 }
        };
        if !attached() {
            // SAFETY: no arguments.
            unsafe { AllocConsole() };
        }
        assert!(attached(), "this process has no console to hand on");
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let args = ["--ignored", "--exact", "exec::tests::console_probe", "--nocapture", "--test-threads=1"];
        let by_hand = Command::new(&exe).args(args).output().unwrap();
        let shared = |out: &str| -> Option<u32> { out.split("CONSOLE_SHARED=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok() };
        assert!(shared(&String::from_utf8_lossy(&by_hand.stdout)).is_some_and(|n| n >= 2), "control: {by_hand:?}");
        let dir = tempfile::tempdir().unwrap();
        let local = Place { git: exe, ..Place::local(dir.path().to_string_lossy()) };
        let out = run_hc(&local, &Git::read(args)).unwrap();
        // A console of its own with only itself on it: not shared, not none.
        assert_eq!(shared(&String::from_utf8_lossy(&out.stdout)), Some(1), "{:?}", String::from_utf8_lossy(&out.stdout));
    }

    /// The child of the test above.
    #[test]
    #[ignore = "a child process of run_hc_starts_its_program_without_a_console_window"]
    fn console_probe() {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            }
            let mut list = [0u32; 4];
            // SAFETY: a buffer of the length it is given.
            // CREATE_NO_WINDOW gives the child a console of its own, with only itself on it.
            let shared = unsafe { GetConsoleProcessList(list.as_mut_ptr(), 4) };
            println!("CONSOLE_SHARED={shared}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_asks_to_leave_the_job_may() {
        use std::os::windows::process::CommandExt as _;
        // Suspended, as run_local starts git.
        let mut child = Command::new("cmd").args(["/c", "exit"]).creation_flags(0x0800_0000 | 0x0000_0004).spawn().unwrap();
        let job = job::adopt(&child).unwrap();
        let _ = child.wait();
        // CREATE_BREAKAWAY_FROM_JOB would fail inside it otherwise; on unix a
        // process can leave its group as well.
        assert!(job.lets_breakaway());
    }

    #[cfg(windows)]
    #[test]
    fn git_never_runs_where_a_cancel_cant_reach_what_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::local(dir.path().to_string_lossy());
        // No job for this thread's calls: a kill would end git alone, not
        // the shell (a hook, ssh, a credential helper) it starts.
        job::NO_JOB.with(|n| n.set(true));
        let r = run(&place, Git::read(["-c", "alias.w=!echo ran > ran.txt", "w"]));
        job::NO_JOB.with(|n| n.set(false));
        assert!(matches!(r, Err(GitError::Spawn(_))), "{r:?}");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!dir.path().join("ran.txt").exists(), "git ran outside a job");
    }

    /// What the job costs per call on Windows (a thread snapshot to resume
    /// git): `cargo test -p yuzora-git --lib -- --ignored --nocapture windows_job_cost`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn windows_job_cost() {
        use std::os::windows::process::CommandExt as _;
        let n = 100;
        let (mut spawn, mut adopt) = (Duration::ZERO, Duration::ZERO);
        for _ in 0..n {
            let t = Instant::now();
            let mut child = Command::new("cmd").args(["/c", "exit"]).creation_flags(0x0800_0000 | 0x0000_0004).spawn().unwrap();
            spawn += t.elapsed();
            let t = Instant::now();
            let _job = job::adopt(&child).unwrap();
            adopt += t.elapsed();
            let _ = child.wait();
        }
        let dir = tempfile::tempdir().unwrap();
        let place = Place::local(dir.path().to_string_lossy());
        let t = Instant::now();
        for _ in 0..n {
            run(&place, Git::read(["--version"])).unwrap();
        }
        let git = t.elapsed();
        eprintln!("JOB COST per call: spawn {:?}, job+snapshot+resume {:?}, whole `git --version` {:?}", spawn / n, adopt / n, git / n);
    }
}
