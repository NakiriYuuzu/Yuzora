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
    pub write_lock: Arc<Mutex<()>>,
}

impl Place {
    pub fn local(root: impl Into<String>) -> Self {
        Self { root: root.into(), on: None, git: "git".into(), write_lock: Arc::default() }
    }

    pub fn program(&self) -> String {
        if self.git.is_empty() { crate::settings::executable() } else { self.git.clone() }
    }
    pub fn is_remote(&self) -> bool {
        self.on.is_some()
    }
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
    let _guard = match cmd.kind {
        Kind::Write => Some(lock(&place.write_lock, cmd.cancel.as_ref())?),
        Kind::Read => None,
    };
    if place.is_remote() { run_remote(place, &cmd, sink) } else { run_local(place, &cmd, sink) }
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
    for (k, v) in environment(cmd.kind, false, &cmd.env) {
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
    for (k, v) in environment(cmd.kind, true, &cmd.env) {
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
    let local = Place { root: yuzora_core::paths::home_dir().to_string_lossy().into_owned(), on: None, git: hc, write_lock: Arc::default() };
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

    #[test]
    fn remote_commands_go_through_hc_exec() {
        let place = Place { root: "/srv/repo".into(), on: Some("lab-01".into()), git: "git".into(), write_lock: Arc::default() };
        let a = hc_args(&place, &Git::read(["status", "-z"]).stdin(b"x".to_vec()).timeout(Duration::from_secs(9000)));
        assert_eq!(&a[..8], ["exec", "--json", "--on", "lab-01", "--timeout", "600", "--cwd", "/srv/repo"]);
        assert!(a.windows(2).any(|w| w == ["--env", "GIT_OPTIONAL_LOCKS=0"]));
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
