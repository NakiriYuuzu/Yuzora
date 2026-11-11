//! Starting a child and holding it to its [`Limits`]: one request in, one
//! answer out, killed with what it started when it oversteps.
//!
//! The child's memory, by platform: macOS has no limit a parent can set that
//! holds, so it is polled ([`TICK`]) for its `phys_footprint` and killed past
//! the limit; Linux sets `RLIMIT_AS` in the child before it runs; Windows
//! puts it in a Job Object (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`, killed when the
//! job closes) before it is sent anything. On Linux and Windows an allocation
//! past the limit fails and Rust aborts the child, saying so on stderr.
//!
//! What it started goes with it: on Unix it leads a process group of its own,
//! killed whole; on Windows the job holds them all.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::wire::{self, Answer, Raster};
use super::{CANCELLED, CRASHED, Cancel, Limits, NOT_STARTED, OUT_OF_MEMORY, TIMED_OUT};

/// How often the child is looked at: its exit, the time, its memory, the
/// request's cancel.
const TICK: Duration = Duration::from_millis(10);
/// The child's error output kept, to tell why it ended.
const STDERR_KEPT: usize = 4096;
/// What Rust writes before it aborts on an allocation that failed.
const ALLOCATION_FAILED: &[u8] = b"memory allocation of";

/// Children start one at a time. On macOS std makes a pipe, then marks it
/// close-on-exec: a child started in between would hold the other's end and
/// keep it from ever ending. std closes this process's copies of the child's
/// ends inside `spawn`, so within this lock. (Neither side waits for an end
/// of file anyway: see [`wire`].)
static SPAWN: Mutex<()> = Mutex::new(());

/// The environment the child keeps: where the system's fonts are.
#[cfg(target_os = "macos")]
const KEEP_ENV: &[&str] = &["HOME"];
#[cfg(all(unix, not(target_os = "macos")))]
const KEEP_ENV: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
];
#[cfg(windows)]
const KEEP_ENV: &[&str] = &["SystemRoot", "USERPROFILE"];

/// Draws `svg` in a child of `program args…`. Err: [`CANCELLED`] or why it
/// isn't drawn.
pub(super) fn run(
    program: &Path,
    args: &[OsString],
    limits: &Limits,
    svg: &[u8],
    cancel: &Cancel,
) -> Result<Raster, &'static str> {
    let child = Running::start(program, args, limits).map_err(|_| NOT_STARTED)?;
    supervise(child, limits, svg, cancel)
}

/// A child, killed (and waited for) unless it ended on its own.
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    ended: bool,
    #[cfg(windows)]
    job: job::Job,
}

impl Running {
    fn start(program: &Path, args: &[OsString], limits: &Limits) -> std::io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in KEEP_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        prepare(&mut command, limits);
        let mut child = {
            let _one = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
            command.spawn()?
        };
        // Confined before it is sent anything: until then it only waits.
        #[cfg(windows)]
        let job = match job::Job::confine(&child, limits.memory) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            let fds = [
                stdin.as_ref().map(|p| p.as_raw_fd()),
                stdout.as_ref().map(|p| p.as_raw_fd()),
                stderr.as_ref().map(|p| p.as_raw_fd()),
            ];
            for fd in fds.into_iter().flatten() {
                unsafe {
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                }
            }
        }
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
            ended: false,
            #[cfg(windows)]
            job,
        })
    }

    /// Kills it and what it started; `why` back.
    fn kill(&mut self, why: &'static str) -> &'static str {
        if !self.ended {
            // Before it is waited for, while its process group can't be another's.
            #[cfg(unix)]
            unsafe {
                libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL);
            }
            #[cfg(windows)]
            self.job.terminate();
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.ended = true;
        }
        why
    }

    /// How it ended, if it has. What it started goes with it.
    fn ended(&mut self) -> Option<ExitStatus> {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) => return None,
            Err(_) => {
                self.kill(CRASHED);
                return Some(failed());
            }
        };
        self.ended = true;
        // Whatever of its group is left: the group keeps its number (a pid
        // isn't reused while a group has it), and it was waited for just now.
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
        Some(status)
    }

    /// Whether it took more memory than `limit` (macOS: here polled).
    fn over_memory(&self, limit: u64) -> bool {
        #[cfg(target_os = "macos")]
        {
            footprint(self.child.id()).is_some_and(|bytes| bytes > limit)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = limit;
            false
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.kill(CANCELLED);
    }
}

/// An exit status that isn't success, for a child whose wait failed.
fn failed() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        ExitStatus::from_raw(1 << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(1)
    }
}

/// Its own process group, and on Linux its address space capped.
#[cfg(unix)]
fn prepare(command: &mut Command, limits: &Limits) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
    #[cfg(target_os = "linux")]
    {
        let limit = limits.memory as libc::rlim_t;
        // Async-signal-safe: between fork and exec.
        unsafe {
            command.pre_exec(move || {
                let cap = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &cap) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = limits;
}

/// No window of its own.
#[cfg(windows)]
fn prepare(command: &mut Command, _: &Limits) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// The memory macOS counts against a process (what Activity Monitor shows).
#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    let ok = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    (ok == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
}

/// The answer, given how the child ended and what it said.
fn judge(status: ExitStatus, answer: Answer, stderr: &[u8]) -> Result<Raster, &'static str> {
    if status.success() {
        return answer.finish();
    }
    if stderr
        .windows(ALLOCATION_FAILED.len())
        .any(|w| w == ALLOCATION_FAILED)
    {
        return Err(OUT_OF_MEMORY);
    }
    Err(CRASHED)
}

/// The request still to send: its header, then the SVG.
struct Request<'a> {
    head: [u8; wire::REQUEST_HEAD],
    svg: &'a [u8],
    sent: usize,
}

impl Request<'_> {
    fn rest(&self) -> &[u8] {
        match self.head.get(self.sent..) {
            Some(head) if !head.is_empty() => head,
            _ => &self.svg[self.sent - self.head.len()..],
        }
    }

    fn done(&self) -> bool {
        self.sent == self.head.len() + self.svg.len()
    }
}

/// Keeps the first [`STDERR_KEPT`] bytes of `data`.
fn keep(kept: &mut Vec<u8>, data: &[u8]) {
    let room = STDERR_KEPT.saturating_sub(kept.len());
    kept.extend_from_slice(&data[..data.len().min(room)]);
}

/// One thread: the pipes are non-blocking, and `poll` waits on them a tick at
/// a time between looks at the child.
#[cfg(unix)]
fn supervise(
    mut child: Running,
    limits: &Limits,
    svg: &[u8],
    cancel: &Cancel,
) -> Result<Raster, &'static str> {
    use std::os::fd::AsRawFd as _;
    let deadline = Instant::now() + limits.timeout;
    let mut request = Request {
        head: wire::request_head(limits.max_edge, svg.len()),
        svg,
        sent: 0,
    };
    let mut answer = Answer::new(limits.max_edge);
    let mut stderr = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.is_cancelled() {
            return Err(child.kill(CANCELLED));
        }
        if Instant::now() >= deadline {
            return Err(child.kill(TIMED_OUT));
        }
        if child.over_memory(limits.memory) {
            return Err(child.kill(OUT_OF_MEMORY));
        }
        if let Some(status) = child.ended() {
            // What it wrote before it ended (what something else may still
            // write isn't waited for).
            while let Some(n) = read_some(&mut child.stdout, &mut buf) {
                answer.take(&buf[..n])?;
            }
            while let Some(n) = read_some(&mut child.stderr, &mut buf) {
                keep(&mut stderr, &buf[..n]);
            }
            return judge(status, answer, &stderr);
        }
        // A pipe closed is -1, which `poll` skips.
        let polled = |fd: Option<i32>, events| libc::pollfd {
            fd: fd.unwrap_or(-1),
            events,
            revents: 0,
        };
        let mut fds = [
            polled(child.stdin.as_ref().map(|p| p.as_raw_fd()), libc::POLLOUT),
            polled(child.stdout.as_ref().map(|p| p.as_raw_fd()), libc::POLLIN),
            polled(child.stderr.as_ref().map(|p| p.as_raw_fd()), libc::POLLIN),
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 3, TICK.as_millis() as libc::c_int) };
        if fds[0].revents != 0
            && let Some(stdin) = child.stdin.as_mut()
        {
            match stdin.write(request.rest()) {
                Ok(n) => request.sent += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                // It stopped reading: what it says now tells.
                Err(_) => child.stdin = None,
            }
            if request.done() {
                // Its end of file, though it reads only what the header names.
                child.stdin = None;
            }
        }
        if fds[1].revents != 0
            && let Some(n) = read_some(&mut child.stdout, &mut buf)
            && let Err(why) = answer.take(&buf[..n])
        {
            return Err(child.kill(why));
        }
        if fds[2].revents != 0
            && let Some(n) = read_some(&mut child.stderr, &mut buf)
        {
            keep(&mut stderr, &buf[..n]);
        }
    }
}

/// What a non-blocking pipe has now: `None` when nothing, closing it at its
/// end or on an error.
#[cfg(unix)]
fn read_some(pipe: &mut Option<impl std::io::Read>, buf: &mut [u8]) -> Option<usize> {
    let p = pipe.as_mut()?;
    match p.read(buf) {
        Ok(0) => {
            *pipe = None;
            None
        }
        Ok(n) => Some(n),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Some(0),
        Err(_) => {
            *pipe = None;
            None
        }
    }
}

/// Windows: a thread a pipe, this one looking at the child a tick at a time.
#[cfg(windows)]
fn supervise(
    mut child: Running,
    limits: &Limits,
    svg: &[u8],
    cancel: &Cancel,
) -> Result<Raster, &'static str> {
    use std::io::Read as _;
    use std::sync::mpsc;
    enum Said {
        Answer(Result<Answer, &'static str>),
        Errors(Vec<u8>),
    }
    let deadline = Instant::now() + limits.timeout;
    let (tx, rx) = mpsc::channel();
    if let Some(mut stdin) = child.stdin.take() {
        let mut request = wire::request_head(limits.max_edge, svg.len()).to_vec();
        request.extend_from_slice(svg);
        std::thread::spawn(move || {
            let _ = stdin.write_all(&request);
        });
    }
    if let Some(mut stdout) = child.stdout.take() {
        let (tx, max_edge) = (tx.clone(), limits.max_edge);
        std::thread::spawn(move || {
            let mut answer = Answer::new(max_edge);
            let mut buf = vec![0u8; 1 << 16];
            let said = loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break Ok(answer),
                    Ok(n) => {
                        if let Err(why) = answer.take(&buf[..n]) {
                            break Err(why);
                        }
                    }
                }
            };
            let _ = tx.send(Said::Answer(said));
        });
    }
    if let Some(mut pipe) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let (mut kept, mut buf) = (Vec::new(), vec![0u8; 1 << 12]);
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                keep(&mut kept, &buf[..n]);
            }
            let _ = tx.send(Said::Errors(kept));
        });
    }
    let (mut answer, mut stderr, mut status) = (None, None, None);
    loop {
        if cancel.is_cancelled() {
            return Err(child.kill(CANCELLED));
        }
        if Instant::now() >= deadline {
            return Err(child.kill(TIMED_OUT));
        }
        match rx.recv_timeout(TICK) {
            Ok(Said::Answer(Err(why))) => return Err(child.kill(why)),
            Ok(Said::Answer(Ok(a))) => answer = Some(a),
            Ok(Said::Errors(e)) => stderr = Some(e),
            Err(_) => {}
        }
        if status.is_none() {
            status = child.ended();
        }
        // Once it ended its pipes close, and both readers are done.
        if let Some(status) = status
            && let Some(said) = answer.take_if(|_| stderr.is_some())
        {
            return judge(status, said, &stderr.unwrap_or_default());
        }
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// A Job Object holding one child and what it starts.
    pub(super) struct Job(HANDLE);

    impl Job {
        /// The child in a new job: `memory` at most, a crash without a
        /// dialog, and all of it killed when the job closes.
        pub fn confine(child: &std::process::Child, memory: u64) -> std::io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Job(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            info.ProcessMemoryLimit = memory as usize;
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub fn terminate(&self) {
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}
