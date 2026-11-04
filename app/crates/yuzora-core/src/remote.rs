//! Running one program here or on another machine, with a timeout the caller
//! picks. Another machine goes through herdr-core: `hc exec --json --on
//! <machine> --timeout <s> [--cwd <dir>] -- <program> <args…>`, which prints
//! one JSON object with the exit code and base64 stdout/stderr (the same call
//! the app's `exec({on})` makes, there fixed at 30 s and 8 MiB).
//!
//! Blocking: call it from a worker thread (the `yuzora:core` `exec` does).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

/// The default limit, as Herdr's own `exec`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes kept per stream; the rest is drained and dropped.
pub const DEFAULT_MAX_OUTPUT: u64 = 8 << 20;
/// How long past the remote timeout the local `hc` may take to answer.
const REMOTE_GRACE: Duration = Duration::from_secs(5);
/// After the program exits, how long its output may take to arrive. Anything
/// it started in the background can keep the pipes open for good.
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// How often an idle reader looks whether it should let go of its pipe (Unix; on Windows it is cancelled).
#[cfg(unix)]
const READ_CHECK: Duration = Duration::from_millis(50);

type FindHc = Box<dyn Fn() -> PathBuf + Send + Sync>;

static HC: OnceLock<FindHc> = OnceLock::new();

/// How to find the herdr-core the app uses, asked on every call: the setup
/// screen can install or pick one after start. `hc` on PATH until set.
pub fn set_hc_program(find: impl Fn() -> PathBuf + Send + Sync + 'static) {
    let _ = HC.set(Box::new(find));
}

pub fn hc_program() -> PathBuf {
    HC.get().map(|find| find()).unwrap_or_else(|| PathBuf::from("hc"))
}

/// One program to run.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// A machine name from `hc machine ls`; `None` runs it here.
    pub on: Option<String>,
    pub timeout: Duration,
    pub max_output: u64,
    /// Set on top of the environment it starts with (`hc exec --env` on a
    /// machine). Keys are `[A-Z0-9_]`, values hold no NUL: [`run`] refuses
    /// anything else.
    pub env: Vec<(String, String)>,
}

impl ExecRequest {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            on: None,
            timeout: DEFAULT_TIMEOUT,
            max_output: DEFAULT_MAX_OUTPUT,
            env: Vec::new(),
        }
    }

    pub fn on(mut self, machine: Option<impl Into<String>>) -> Self {
        self.on = machine.map(Into::into).filter(|m: &String| !m.is_empty());
        self
    }

    pub fn cwd(mut self, cwd: Option<impl Into<String>>) -> Self {
        self.cwd = cwd.map(Into::into);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_output(mut self, bytes: u64) -> Self {
        self.max_output = bytes;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// The first of the request's variables [`run`] won't set: a key that isn't
/// `[A-Z0-9_]+`, or a value with a NUL.
fn refused_env(req: &ExecRequest) -> Option<&str> {
    let key_ok = |k: &str| !k.is_empty() && k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    req.env.iter().find(|(k, v)| !key_ok(k) || v.contains('\0')).map(|(k, _)| k.as_str())
}

/// What the program printed and how it ended.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecError {
    /// The program (or `hc`) could not start.
    Spawn(String),
    TimedOut { seconds: f64, machine: Option<String> },
    /// herdr-core refused or could not reach the machine.
    Remote(String),
    /// `hc exec` answered with something that isn't its JSON.
    Protocol(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Spawn(e) | ExecError::Remote(e) | ExecError::Protocol(e) => f.write_str(e),
            ExecError::TimedOut { seconds, machine: Some(m) } => write!(f, "took longer than {seconds} s on {m} and was stopped"),
            ExecError::TimedOut { seconds, machine: None } => write!(f, "took longer than {seconds} s and was stopped"),
        }
    }
}

impl std::error::Error for ExecError {}

/// The `hc` arguments for a remote request (`None` when it runs here).
pub fn hc_exec_args(req: &ExecRequest) -> Option<Vec<String>> {
    let machine = req.on.as_ref()?;
    // hc takes whole seconds; never round a short timeout down to "none".
    let secs = req.timeout.as_secs_f64().ceil().max(1.0) as u64;
    let mut args = vec!["exec".into(), "--json".into(), "--on".into(), machine.clone(), "--timeout".into(), secs.to_string()];
    if let Some(cwd) = &req.cwd {
        args.extend(["--cwd".into(), cwd.clone()]);
    }
    for (k, v) in &req.env {
        args.extend(["--env".into(), format!("{k}={v}")]);
    }
    args.push("--".into());
    args.push(req.program.clone());
    args.extend(req.args.iter().cloned());
    Some(args)
}

/// Reads `hc exec --json`'s answer (on stdout, or stderr for its own errors).
pub fn parse_hc_exec(stdout: &str, stderr: &str, req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    let v: Value = serde_json::from_str(stdout.trim())
        .or_else(|_| serde_json::from_str(stderr.trim()))
        .map_err(|_| ExecError::Protocol(format!("hc exec: {}", stderr.trim())))?;
    if let Some(e) = v.get("error") {
        let message = e.get("message").and_then(Value::as_str).unwrap_or("hc exec failed").to_owned();
        return Err(ExecError::Remote(message));
    }
    if v.get("timed_out").and_then(Value::as_bool) == Some(true) {
        return Err(ExecError::TimedOut { seconds: req.timeout.as_secs_f64(), machine: req.on.clone() });
    }
    // hc has no output limit: the request's cap applies here, as it does locally.
    let limit = usize::try_from(req.max_output).unwrap_or(usize::MAX);
    let bytes = |k: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(v.get(k).and_then(Value::as_str).unwrap_or_default())
            .map(|mut b| {
                b.truncate(limit);
                b
            })
            .map_err(|e| ExecError::Protocol(format!("hc exec {k}: {e}")))
    };
    let code = v.get("code").and_then(Value::as_i64).map_or(-1, |c| c as i32);
    Ok(ExecOutput { code, stdout: bytes("stdout")?, stderr: bytes("stderr")? })
}

/// Runs the request and waits for it (blocking).
pub fn run(req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    if let Some(key) = refused_env(req) {
        return Err(ExecError::Spawn(format!("cannot run {}: {key:?} is not an environment variable it may set", req.program)));
    }
    match hc_exec_args(req) {
        Some(args) => {
            // hc's JSON carries both streams in base64 (4/3 the size), whole:
            // room for the default 8 MiB of each, or the request's cap if larger.
            let answer = req.max_output.max(DEFAULT_MAX_OUTPUT).saturating_mul(3).saturating_add(64 << 10);
            let local = ExecRequest {
                program: hc_program().to_string_lossy().into_owned(),
                args,
                cwd: None,
                on: None,
                timeout: req.timeout + REMOTE_GRACE,
                max_output: answer,
                env: Vec::new(),
            };
            let out = run_local(&local)?;
            if out.stdout.len() as u64 >= answer {
                return Err(ExecError::Protocol(format!("hc exec: the answer from {} was larger than {answer} bytes", req.on.as_deref().unwrap_or("the machine"))));
            }
            parse_hc_exec(&out.stdout_text(), &out.stderr_text(), req)
        }
        None => run_local(req),
    }
}

/// The program `req` names, set up as [`run_local`] starts it here.
fn command(req: &ExecRequest) -> Command {
    let mut cmd = Command::new(&req.program);
    cmd.args(&req.args).envs(req.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.current_dir(req.cwd.as_ref().map(PathBuf::from).unwrap_or_else(crate::paths::home_dir));
    // The app has no console on Windows: without this every call opens a window.
    crate::spawn::no_window(&mut cmd);
    // Its own process group, so a timeout can end everything it started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd
}

fn run_local(req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    let mut cmd = command(req);
    let mut child = cmd.spawn().map_err(|e| ExecError::Spawn(format!("cannot run {}: {e}", req.program)))?;
    let out = Drain::start(child.stdout.take().expect("piped stdout"), req.max_output);
    let err = Drain::start(child.stderr.take().expect("piped stderr"), req.max_output);
    // yuzora(perf): no 10 ms polling: the answer comes the moment it exits. The `Child` stays here
    // the whole time (see `kill_tree` for why).
    let status = match wait_for_exit(&mut child, req.timeout) {
        Ok(Some(s)) => s,
        Ok(None) => {
            kill_tree(&mut child);
            let _ = child.wait();
            return Err(ExecError::TimedOut { seconds: req.timeout.as_secs_f64(), machine: None });
        }
        Err(e) => return Err(ExecError::Spawn(e.to_string())),
    };
    // What the program wrote is already in the pipes; whatever it left running
    // in the background may hold them open, so don't wait for that (the
    // readers let go of the pipes as the Drains drop).
    let until = Instant::now() + PIPE_GRACE;
    Ok(ExecOutput { code: status.code().unwrap_or(-1), stdout: out.finish(until), stderr: err.finish(until) })
}

/// Waits for the program to end, up to `timeout` (None: still running). The program is not
/// reaped until the caller says so, so its id cannot be handed to another process meanwhile.
#[cfg(windows)]
fn wait_for_exit(child: &mut std::process::Child, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
    }
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x102;
    // Just under INFINITE (u32::MAX), in the pieces a `u32` of milliseconds allows.
    let end = Instant::now().checked_add(timeout);
    loop {
        let left = end.map_or(Duration::from_millis(u64::from(u32::MAX - 1)), |e| e.saturating_duration_since(Instant::now()));
        let ms = u32::try_from(left.as_millis().min(u128::from(u32::MAX - 1))).unwrap_or(u32::MAX - 1);
        // SAFETY: the process handle `child` owns, open for the whole call.
        match unsafe { WaitForSingleObject(std::os::windows::io::AsRawHandle::as_raw_handle(child), ms) } {
            WAIT_OBJECT_0 => return child.wait().map(Some),
            WAIT_TIMEOUT if end.is_some_and(|e| Instant::now() >= e) => return Ok(None),
            WAIT_TIMEOUT => {}
            _ => return Err(std::io::Error::last_os_error()),
        }
    }
}

/// Same, on Unix: a thread watches for the exit with `waitid(WNOWAIT)` (it does not reap),
/// so `child` stays here, unreaped, until this thread has what it needs.
#[cfg(unix)]
fn wait_for_exit(child: &mut std::process::Child, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    let pid = child.id() as libc::id_t;
    let (seen, exited) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            // SAFETY: an all-zero siginfo is the "nothing yet" value waitid fills in.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waits for one child of this process without reaping it.
            let r = unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if r == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                let _ = seen.send(r == 0);
                return;
            }
        }
    });
    match exited.recv_timeout(timeout) {
        Ok(true) => child.wait().map(Some),
        // waitid could not watch it: ask the plain way (it ends or the timeout does).
        Ok(false) => child.wait().map(Some),
        Err(_) => Ok(None),
    }
}

/// Ends the program and everything it started: its process group on Unix
/// (`process_group(0)` above), its process tree on Windows.
///
/// It takes the `Child`, not a process id, on purpose. A process id is free to be handed to
/// another process once the program is reaped (Windows: once the last handle is closed), and a
/// kill by a bare id then lands on whatever got it, possibly another git of this app. While
/// `child` is alive here its id is ours: `taskkill /PID` and `child.kill()` (TerminateProcess on
/// the handle) cannot hit anything else.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a plain syscall on the group this call created for the child.
        unsafe { libc::killpg(group, libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    // The program itself, by its handle.
    let _ = child.kill();
}

/// A pipe its reader can wait on without blocking on it for good.
#[cfg(unix)]
trait Pipe: Read + Send + std::os::fd::AsRawFd + 'static {}
#[cfg(unix)]
impl<T: Read + Send + std::os::fd::AsRawFd + 'static> Pipe for T {}
#[cfg(windows)]
trait Pipe: Read + Send + std::os::windows::io::AsRawHandle + 'static {}
#[cfg(windows)]
impl<T: Read + Send + std::os::windows::io::AsRawHandle + 'static> Pipe for T {}

/// Whether `r` has something to read, or has ended, within `wait`.
#[cfg(unix)]
fn readable(r: &impl Pipe, wait: Duration) -> std::io::Result<bool> {
    let mut fd = libc::pollfd { fd: r.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one pollfd, alive for the whole call.
    match unsafe { libc::poll(&mut fd, 1, ms) } {
        0 => Ok(false),
        n if n > 0 => Ok(true),
        _ => {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted { Ok(false) } else { Err(e) }
        }
    }
}

/// yuzora(perf): on Windows the reader blocks in `read` (no polling with a sleep: the bytes
/// are taken the moment they arrive) and is cancelled from outside when it must let go.
#[cfg(windows)]
mod blocking {
    use std::sync::{Arc, Mutex};

    /// A pipe handle that can be told to stop a read pending on it, for as long as it is open.
    #[derive(Clone)]
    pub struct Cancel {
        handle: usize,
        open: Arc<Mutex<bool>>,
    }

    impl Cancel {
        pub fn new(handle: *mut std::ffi::c_void) -> Self {
            Cancel { handle: handle as usize, open: Arc::new(Mutex::new(true)) }
        }

        /// The pipe is about to be closed: no cancel reaches a handle that is not this one any more.
        pub fn closed(&self) {
            *self.open.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }

        pub fn is_open(&self) -> bool {
            *self.open.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Ends a read pending on the pipe (it fails with "aborted").
        pub fn cancel(&self) {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn CancelIoEx(file: *mut std::ffi::c_void, overlapped: *mut std::ffi::c_void) -> i32;
            }
            let open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            if *open {
                // SAFETY: the handle is still open (the reader marks it closed, under this lock, before it drops it).
                unsafe { CancelIoEx(self.handle as *mut std::ffi::c_void, std::ptr::null_mut()) };
            }
        }
    }
}

/// One output stream read on its own thread, up to a limit (the rest is
/// drained and dropped), into a buffer that can be taken before it ends.
/// Dropped, it lets go of the pipe even while something still holds the
/// other end (a program's background child).
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    ended: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
    #[cfg(windows)]
    cancel: blocking::Cancel,
}

impl Drain {
    fn start(mut r: impl Pipe, limit: u64) -> Self {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, ended) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (into, stopped) = (buf.clone(), stop.clone());
        #[cfg(windows)]
        let cancel = blocking::Cancel::new(std::os::windows::io::AsRawHandle::as_raw_handle(&r));
        #[cfg(windows)]
        let reader_cancel = cancel.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 16 << 10];
            // Until the stream ends or the Drain is dropped; the pipe closes with `r`.
            while !stopped.load(Ordering::Relaxed) {
                #[cfg(unix)]
                match readable(&r, READ_CHECK) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => break,
                }
                match r.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut b = into.lock().unwrap_or_else(|e| e.into_inner());
                        let room = limit.saturating_sub(b.len());
                        b.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            #[cfg(windows)]
            reader_cancel.closed();
            let _ = tx.send(());
        });
        Drain {
            buf,
            ended,
            stop,
            #[cfg(windows)]
            cancel,
        }
    }

    /// Everything read once the stream ends, or what was read by `until`.
    fn finish(self, until: Instant) -> Vec<u8> {
        let _ = self.ended.recv_timeout(until.saturating_duration_since(Instant::now()));
        std::mem::take(&mut *self.buf.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for Drain {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // A reader blocked in `read` (a program's background child holds the other end)
        // is told to stop; if it was not in `read` yet, the next tries find it.
        #[cfg(windows)]
        if self.cancel.is_open() {
            let cancel = self.cancel.clone();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    cancel.cancel();
                    std::thread::sleep(Duration::from_millis(20));
                    if !cancel.is_open() {
                        break;
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_requests_become_hc_exec_with_their_timeout() {
        let req = ExecRequest::new("git", ["status", "--short"]).on(Some("lab-01")).cwd(Some("/srv/repo")).timeout(Duration::from_millis(2500));
        assert_eq!(
            hc_exec_args(&req).unwrap(),
            ["exec", "--json", "--on", "lab-01", "--timeout", "3", "--cwd", "/srv/repo", "--", "git", "status", "--short"]
        );
        assert_eq!(hc_exec_args(&ExecRequest::new("ls", Vec::<String>::new())), None);
        assert_eq!(hc_exec_args(&ExecRequest::new("ls", Vec::<String>::new()).on(Some(""))), None);
        let tiny = ExecRequest::new("true", Vec::<String>::new()).on(Some("m")).timeout(Duration::from_millis(1));
        assert_eq!(hc_exec_args(&tiny).unwrap()[5], "1");
    }

    #[test]
    fn variables_are_set_only_when_asked_for() {
        // None asked for: the hc arguments and the local command are as they were.
        let plain = ExecRequest::new("git", ["status"]).on(Some("m")).cwd(Some("/r"));
        assert_eq!(hc_exec_args(&plain).unwrap(), ["exec", "--json", "--on", "m", "--timeout", "30", "--cwd", "/r", "--", "git", "status"]);
        let cmd = command(&plain);
        assert_eq!(cmd.get_envs().count(), 0);
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["status"]);
        // Asked for: on top of the rest, there and here.
        let set = plain.env("GIT_NO_LAZY_FETCH", "1").env("A_9", "x=y");
        assert_eq!(
            hc_exec_args(&set).unwrap(),
            ["exec", "--json", "--on", "m", "--timeout", "30", "--cwd", "/r", "--env", "GIT_NO_LAZY_FETCH=1", "--env", "A_9=x=y", "--", "git", "status"]
        );
        let cmd = command(&set);
        let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_str().unwrap(), v.and_then(|v| v.to_str()))).collect();
        assert_eq!(envs, [("A_9", Some("x=y")), ("GIT_NO_LAZY_FETCH", Some("1"))]);
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["status"]);
    }

    #[test]
    fn variables_it_may_not_set_are_refused() {
        for (k, v) in [("lower", "1"), ("A=B", "1"), ("", "1"), ("A-B", "1"), ("PATH ", "1"), ("\u{c9}", "1"), ("OK", "a\0b")] {
            let req = ExecRequest::new("git", ["status"]).env("OK", "1").env(k, v);
            assert_eq!(refused_env(&req), Some(k), "{k:?}={v:?}");
            assert_eq!(refused_env(&req.on(Some("m"))), Some(k));
        }
        assert_eq!(refused_env(&ExecRequest::new("git", ["status"]).env("GIT_ALLOW_PROTOCOL", "").env("_1", "a b")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_local_run_sees_its_variables_and_runs_none_it_refuses() {
        let out = run(&ExecRequest::new("sh", ["-c", "printf %s \"$YZ_EXEC_T\""]).env("YZ_EXEC_T", "set")).unwrap();
        assert_eq!(out.stdout_text(), "set");
        let mark = std::env::temp_dir().join(format!("yuzora-exec-env-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let script = format!("touch '{}'", mark.display());
        let refused = run(&ExecRequest::new("sh", ["-c", script.as_str()]).env("bad", "1"));
        assert!(matches!(&refused, Err(ExecError::Spawn(e)) if e.contains("\"bad\"")), "{refused:?}");
        assert!(!mark.exists(), "it ran");
    }

    #[test]
    fn hc_exec_answers_are_decoded() {
        let req = ExecRequest::new("echo", ["hi"]).on(Some("vps"));
        let ok = parse_hc_exec(r#"{"code":0,"stdout":"aGkK","stderr":""}"#, "", &req).unwrap();
        assert_eq!((ok.code, ok.stdout_text().as_str()), (0, "hi\n"));
        let refused = parse_hc_exec("", r#"{"error":{"message":"no machine vps"}}"#, &req);
        assert_eq!(refused, Err(ExecError::Remote("no machine vps".into())));
        let slow = parse_hc_exec(r#"{"timed_out":true,"code":null}"#, "", &req).unwrap_err();
        assert!(slow.to_string().contains("on vps"));
        assert!(matches!(parse_hc_exec("nonsense", "boom", &req), Err(ExecError::Protocol(_))));
        // The request's cap holds for remote output too ("aGkK" is "hi\n").
        let capped = parse_hc_exec(r#"{"code":0,"stdout":"aGkK","stderr":"aGkK"}"#, "", &req.clone().max_output(1)).unwrap();
        assert_eq!((capped.stdout_text().as_str(), capped.stderr_text().as_str()), ("h", "h"));
    }

    #[cfg(unix)]
    #[test]
    fn local_runs_capture_output_and_stop_at_the_timeout() {
        let out = run(&ExecRequest::new("sh", ["-c", "printf out; printf err >&2; exit 3"])).unwrap();
        assert_eq!((out.code, out.stdout_text().as_str(), out.stderr_text().as_str()), (3, "out", "err"));
        let capped = run(&ExecRequest::new("sh", ["-c", "printf 0123456789"]).max_output(4)).unwrap();
        assert_eq!(capped.stdout_text(), "0123");
        let started = Instant::now();
        let slow = run(&ExecRequest::new("sleep", ["5"]).timeout(Duration::from_millis(200)));
        assert!(matches!(slow, Err(ExecError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(matches!(run(&ExecRequest::new("yuzora-no-such-program", Vec::<String>::new())), Err(ExecError::Spawn(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_ends_what_the_program_started() {
        let pid_file = std::env::temp_dir().join(format!("yuzora-exec-tree-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());
        let slow = run(&ExecRequest::new("sh", ["-c", script.as_str()]).timeout(Duration::from_millis(300)));
        assert!(matches!(slow, Err(ExecError::TimedOut { .. })));
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        let _ = std::fs::remove_file(&pid_file);
        // Killed with its shell, then reparented and reaped: gone in a moment.
        let gone = (0..100).any(|_| {
            // SAFETY: signal 0 only asks whether the process exists.
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            if alive {
                std::thread::sleep(Duration::from_millis(20));
            }
            !alive
        });
        assert!(gone, "sleep {pid} outlived the timeout");
    }

    #[cfg(unix)]
    #[test]
    fn a_background_child_holding_the_pipes_does_not_hold_the_answer() {
        // `sleep` inherits stdout and stderr and outlives the shell by seconds.
        let started = Instant::now();
        let out = run(&ExecRequest::new("sh", ["-c", "printf done; printf warn >&2; sleep 4 & exit 0"]).timeout(Duration::from_secs(30))).unwrap();
        assert_eq!((out.code, out.stdout_text().as_str(), out.stderr_text().as_str()), (0, "done", "warn"));
        assert!(started.elapsed() < Duration::from_secs(3), "waited {:?}", started.elapsed());
    }

    /// A program that ends only when asked to (about `secs` seconds), on any Windows: `ping`.
    #[cfg(windows)]
    fn waits(ip: &str, secs: u32) -> String {
        format!("ping -n {} {ip} > nul", secs + 1)
    }

    #[cfg(windows)]
    #[test]
    fn windows_local_runs_capture_output_and_stop_at_the_timeout() {
        let out = run(&ExecRequest::new("cmd", ["/c", "echo out& echo err 1>&2& exit 3"])).unwrap();
        assert_eq!((out.code, out.stdout_text().trim(), out.stderr_text().trim()), (3, "out", "err"));
        let capped = run(&ExecRequest::new("cmd", ["/c", "echo 0123456789"]).max_output(4)).unwrap();
        assert_eq!(capped.stdout_text(), "0123");
        let started = Instant::now();
        let slow = run(&ExecRequest::new("cmd", ["/c", waits("127.0.0.71", 20).as_str()]).timeout(Duration::from_millis(300)));
        assert!(matches!(slow, Err(ExecError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(matches!(run(&ExecRequest::new("yuzora-no-such-program", Vec::<String>::new())), Err(ExecError::Spawn(_))));
    }

    #[cfg(windows)]
    #[test]
    fn windows_a_timeout_ends_what_the_program_started() {
        let ip = "127.0.0.77";
        let slow = run(&ExecRequest::new("cmd", ["/c", waits(ip, 60).as_str()]).timeout(Duration::from_millis(500)));
        assert!(matches!(slow, Err(ExecError::TimedOut { .. })));
        let check = format!("@(Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*{ip}*' }}).Count");
        let gone = (0..50).any(|_| {
            let n = run(&ExecRequest::new("powershell", ["-NoProfile", "-Command", check.as_str()])).unwrap().stdout_text();
            let gone = n.trim() == "0";
            if !gone {
                std::thread::sleep(Duration::from_millis(100));
            }
            gone
        });
        assert!(gone, "the ping outlived the timeout");
    }

    #[cfg(windows)]
    #[test]
    fn windows_a_background_child_holding_the_pipes_does_not_hold_the_answer() {
        // `ping` inherits stdout and stderr and outlives the shell by seconds.
        let started = Instant::now();
        let out = run(&ExecRequest::new("cmd", ["/c", "start /b ping -n 6 127.0.0.78 > nul & echo done"]).timeout(Duration::from_secs(30))).unwrap();
        assert_eq!((out.code, out.stdout_text().trim()), (0, "done"));
        assert!(started.elapsed() < Duration::from_secs(3), "waited {:?}", started.elapsed());
    }

    /// A program is ended through the `Child` it was started as, never by a bare process id (which
    /// may belong to another process by then): no `OpenProcess(pid)`, no `kill(pid)` by number.
    #[test]
    fn a_program_is_ended_by_its_child_not_by_a_process_id() {
        let src = include_str!("remote.rs");
        let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        for bare in [concat!("Open", "Process("), concat!("fn kill_tree(", "pid"), concat!("libc::", "kill(")] {
            assert!(!code.contains(bare), "{bare} is back: a process id can be someone else's by now");
        }
        assert!(code.contains("fn kill_tree(child: &mut std::process::Child)"));
    }

    /// The real path of a letting-go: a program's own stdout (what `run` reads) held open by a
    /// program that is still running and says nothing.
    #[cfg(windows)]
    #[test]
    fn windows_a_reader_of_a_childs_stdout_lets_go_while_the_child_still_runs() {
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", "Start-Sleep 20"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        crate::spawn::no_window(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let drain = Drain::start(child.stdout.take().unwrap(), 64);
        let cancel = drain.cancel.clone();
        assert!(cancel.is_open());
        let started = Instant::now();
        assert!(drain.finish(Instant::now() + Duration::from_millis(300)).is_empty());
        // finish gave up waiting and dropped the Drain: the blocked read is cancelled, the pipe closed.
        let let_go = (0..100).any(|_| {
            let closed = !cancel.is_open();
            if !closed {
                std::thread::sleep(Duration::from_millis(20));
            }
            closed
        });
        let took = started.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        assert!(let_go, "the reader still holds the child's stdout");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn a_reader_lets_go_of_a_pipe_still_held_open() {
        use std::io::Write as _;
        let (r, mut w) = std::io::pipe().unwrap();
        w.write_all(b"early").unwrap();
        assert_eq!(Drain::start(r, 64).finish(Instant::now() + Duration::from_millis(200)), b"early");
        // The writer (a background child, in an exec) is still open; the reader
        // closes its end anyway, so writing fails instead of feeding a stuck thread.
        let closed = (0..100).any(|_| {
            let refused = w.write_all(b"late").is_err();
            if !refused {
                std::thread::sleep(Duration::from_millis(20));
            }
            refused
        });
        assert!(closed, "the reader still holds the pipe");
    }

    #[test]
    fn the_core_is_asked_for_on_every_call() {
        static CORE: Mutex<Option<PathBuf>> = Mutex::new(None);
        set_hc_program(|| CORE.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("hc")));
        assert_eq!(hc_program(), PathBuf::from("hc"));
        // The setup screen installs or picks one after start.
        *CORE.lock().unwrap() = Some(PathBuf::from("/opt/herdr/herdr-core"));
        assert_eq!(hc_program(), PathBuf::from("/opt/herdr/herdr-core"));
    }
}
