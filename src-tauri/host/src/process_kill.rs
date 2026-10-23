use std::io;
use std::process::{Child, Command};
#[cfg(any(unix, windows, test))]
use std::time::{Duration, Instant};

#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_millis(300);
#[cfg(unix)]
const EXIT_POLL: Duration = Duration::from_secs(3);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(unix)]
const UNIX_REAP_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(windows)]
const WINDOWS_CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(windows)]
const WINDOWS_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(windows)]
const WINDOWS_TASKKILL_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(windows)]
const WINDOWS_TASKKILL_REAP_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(windows)]
const WINDOWS_JOB_EXIT_TIMEOUT_MS: u32 = 3_000;

#[cfg(any(windows, test))]
const WINDOWS_CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(any(windows, test))]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(any(windows, test))]
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsJobLifecycleStep {
    Create,
    ConfigureKillOnClose,
    AssignChild,
    RetainForChildLifetime,
    TerminateAndWait,
    Close,
    ReapDirectChild,
}

#[cfg(test)]
const WINDOWS_JOB_LIFECYCLE_PLAN: [WindowsJobLifecycleStep; 7] = [
    WindowsJobLifecycleStep::Create,
    WindowsJobLifecycleStep::ConfigureKillOnClose,
    WindowsJobLifecycleStep::AssignChild,
    WindowsJobLifecycleStep::RetainForChildLifetime,
    WindowsJobLifecycleStep::TerminateAndWait,
    WindowsJobLifecycleStep::Close,
    WindowsJobLifecycleStep::ReapDirectChild,
];

#[cfg(any(windows, test))]
const fn windows_job_limit_flags() -> u32 {
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
}

#[cfg(any(windows, test))]
const fn windows_creation_flags() -> u32 {
    WINDOWS_CREATE_NEW_PROCESS_GROUP | WINDOWS_CREATE_NO_WINDOW
}

#[cfg(any(windows, test))]
const fn windows_hidden_creation_flags() -> u32 {
    WINDOWS_CREATE_NO_WINDOW
}

#[cfg(unix)]
type GroupId = libc::pid_t;

#[cfg(unix)]
pub fn configure_new_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
pub fn configure_new_group(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;

    cmd.creation_flags(windows_creation_flags());
}

#[cfg(not(any(unix, windows)))]
pub fn configure_new_group(_cmd: &mut Command) {}

#[cfg(unix)]
pub fn configure_background_process(cmd: &mut Command) {
    configure_new_group(cmd);
}

#[cfg(windows)]
pub fn configure_background_process(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;

    cmd.creation_flags(windows_creation_flags());
}

#[cfg(not(any(unix, windows)))]
pub fn configure_background_process(_cmd: &mut Command) {}

#[cfg(windows)]
pub fn configure_hidden_process(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;

    cmd.creation_flags(windows_hidden_creation_flags());
}

#[cfg(not(windows))]
pub fn configure_hidden_process(_cmd: &mut Command) {}

/// Longest a spawn waits for the process-wide spawn lock before it goes ahead
/// without it, so one stalled exec cannot freeze every other spawn forever.
const SPAWN_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Serializes process creation inside one process. macOS std creates every
/// `Stdio::piped()` with `pipe()` and only then marks both ends close-on-exec,
/// and `openpty` does the same for a PTY; a spawn on another thread inside
/// that gap hands the child those descriptors for its whole life, and the
/// fork path can even stall `spawn()` itself on the exec-status pipe. Holding
/// this lock across each synchronous spawn keeps the gaps from overlapping.
/// It does not cover descriptors created outside a spawn (sockets, `accept`,
/// `socketpair`) or spawns made by third-party code.
///
/// A held flag plus a condition variable rather than a bare `Mutex<()>`, so
/// the wait can be bounded.
struct SpawnLock {
    held: std::sync::Mutex<bool>,
    released: std::sync::Condvar,
}

/// Dropping it releases the lock when this guard actually owns it.
struct SpawnGuard<'a> {
    lock: &'a SpawnLock,
    owned: bool,
}

impl SpawnLock {
    const fn new() -> Self {
        Self {
            held: std::sync::Mutex::new(false),
            released: std::sync::Condvar::new(),
        }
    }

    /// Wait up to `wait` for the lock. On timeout the guard owns nothing and
    /// the caller proceeds unserialized after one diagnostic line.
    fn acquire(&self, wait: std::time::Duration) -> SpawnGuard<'_> {
        self.acquire_logging(wait, |wait| {
            use std::io::Write as _;
            // No command, path or argument here: this line may reach shared
            // logs. A write error (closed stderr) must not panic the spawner.
            let _ = writeln!(
                io::stderr(),
                "process spawn lock still busy after {wait:?}; spawning without it"
            );
        })
    }

    /// `acquire` with the timeout report handed in. It runs after the state
    /// mutex is released, so a report that blocks (a full stderr pipe) can
    /// only hold up its own thread, never another acquire or a release.
    fn acquire_logging(
        &self,
        wait: std::time::Duration,
        on_timeout: impl FnOnce(std::time::Duration),
    ) -> SpawnGuard<'_> {
        let held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut held, _) = self
            .released
            .wait_timeout_while(held, wait, |held| *held)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owned = !*held;
        if owned {
            *held = true;
        }
        drop(held);
        if !owned {
            on_timeout(wait);
        }
        SpawnGuard { lock: self, owned }
    }
}

impl Drop for SpawnGuard<'_> {
    fn drop(&mut self) {
        if self.owned {
            *self
                .lock
                .held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
            self.lock.released.notify_one();
        }
    }
}

static SPAWN_LOCK: SpawnLock = SpawnLock::new();

thread_local! {
    /// Set while this thread is inside `with_spawn_lock`.
    static IN_SPAWN_LOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The spawns this thread makes through the lock while `trace_spawns` runs.
    static SPAWN_TRACE: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

struct InSpawnLock;

impl InSpawnLock {
    fn enter() -> Self {
        IN_SPAWN_LOCK.with(|inside| inside.set(true));
        Self
    }
}

impl Drop for InSpawnLock {
    fn drop(&mut self) {
        IN_SPAWN_LOCK.with(|inside| inside.set(false));
    }
}

/// Run `f` while holding the process-wide spawn lock. Every production spawn
/// goes through this (or `spawn_std` / `spawn_tokio`), so no spawn can overlap
/// another's pipe or PTY setup.
///
/// The lock is not reentrant: `f` may only create descriptors and spawn
/// (`openpty`, `try_clone_reader`, `take_writer`, `spawn_command`, dropping the
/// slave). It must not wait, `.await`, take another lock or spawn through the
/// helpers again; failure cleanup and kills belong after this returns. A nested
/// call trips a `debug_assert!`; in release it runs `f` under the outer hold.
pub fn with_spawn_lock<R>(f: impl FnOnce() -> R) -> R {
    trace_spawn(|| "with_spawn_lock".to_owned());
    locked(f)
}

fn locked<R>(f: impl FnOnce() -> R) -> R {
    let nested = IN_SPAWN_LOCK.with(std::cell::Cell::get);
    debug_assert!(!nested, "the process spawn lock is not reentrant");
    if nested {
        return f();
    }
    let _inside = InSpawnLock::enter();
    let _guard = SPAWN_LOCK.acquire(SPAWN_LOCK_WAIT);
    f()
}

/// `Command::spawn` under the process-wide spawn lock. The lock covers only the
/// synchronous spawn, never the child's runtime.
pub fn spawn_std(command: &mut Command) -> io::Result<Child> {
    trace_spawn(|| command_line(command));
    locked(|| command.spawn())
}

/// `tokio::process::Command::spawn` under the process-wide spawn lock. The call
/// is synchronous, so no `.await` is held across the lock.
pub fn spawn_tokio(command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
    trace_spawn(|| command_line(command.as_std()));
    locked(|| command.spawn())
}

fn command_line(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn trace_spawn(describe: impl FnOnce() -> String) {
    SPAWN_TRACE.with(|trace| {
        if let Some(spawns) = trace.borrow_mut().as_mut() {
            spawns.push(describe());
        }
    });
}

/// Test support: run `f` and return what the calling thread spawned through
/// the lock meanwhile, as `program args` for `spawn_std` / `spawn_tokio` and
/// `with_spawn_lock` for a custom closure. A spawn site proves it takes the
/// lock by appearing here, without racing any other thread. Nothing is
/// recorded outside this call.
#[doc(hidden)]
pub fn trace_spawns<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    SPAWN_TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
    let result = f();
    let spawns = SPAWN_TRACE.with(|trace| trace.borrow_mut().take());
    (result, spawns.unwrap_or_default())
}

/// Retained process-tree containment for a spawned child. On Windows this owns
/// the configured Job Object for the child's entire lifetime; other platforms
/// already contain children through the launch-time process group.
pub struct ProcessTreeGuard {
    #[cfg(windows)]
    job: WindowsJob,
}

#[cfg(windows)]
struct WindowsJob {
    handle: Option<usize>,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct IoCounters {
    ReadOperationCount: u64,
    WriteOperationCount: u64,
    OtherOperationCount: u64,
    ReadTransferCount: u64,
    WriteTransferCount: u64,
    OtherTransferCount: u64,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectBasicLimitInformation {
    PerProcessUserTimeLimit: i64,
    PerJobUserTimeLimit: i64,
    LimitFlags: u32,
    MinimumWorkingSetSize: usize,
    MaximumWorkingSetSize: usize,
    ActiveProcessLimit: u32,
    Affinity: usize,
    PriorityClass: u32,
    SchedulingClass: u32,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectExtendedLimitInformation {
    BasicLimitInformation: JobObjectBasicLimitInformation,
    IoInfo: IoCounters,
    ProcessMemoryLimit: usize,
    JobMemoryLimit: usize,
    PeakProcessMemoryUsed: usize,
    PeakJobMemoryUsed: usize,
}

#[cfg(windows)]
impl WindowsJob {
    fn create_and_assign(child: &mut Child) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;

        type Handle = *mut core::ffi::c_void;
        extern "system" {
            fn CreateJobObjectW(attributes: *const core::ffi::c_void, name: *const u16) -> Handle;
            fn SetInformationJobObject(
                job: Handle,
                information_class: i32,
                information: *const core::ffi::c_void,
                information_length: u32,
            ) -> i32;
            fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
            fn CloseHandle(handle: Handle) -> i32;
        }
        const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut information: JobObjectExtendedLimitInformation = unsafe { std::mem::zeroed() };
        information.BasicLimitInformation.LimitFlags = windows_job_limit_flags();
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&information as *const JobObjectExtendedLimitInformation).cast(),
                std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };
        if configured == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                CloseHandle(handle);
            }
            return Err(error);
        }
        let assigned = unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) };
        if assigned == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                CloseHandle(handle);
            }
            return Err(error);
        }
        Ok(Self {
            handle: Some(handle as usize),
        })
    }

    fn terminate_wait_and_close(&mut self) -> io::Result<()> {
        type Handle = *mut core::ffi::c_void;
        extern "system" {
            fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
            fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
            fn CloseHandle(handle: Handle) -> i32;
        }
        const WAIT_OBJECT_0: u32 = 0;
        const WAIT_TIMEOUT: u32 = 0x0000_0102;
        const WAIT_FAILED: u32 = u32::MAX;

        let Some(raw) = self.handle else {
            return Ok(());
        };
        let handle = raw as Handle;
        let terminate_error = if unsafe { TerminateJobObject(handle, 1) } == 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        let wait_result = unsafe { WaitForSingleObject(handle, WINDOWS_JOB_EXIT_TIMEOUT_MS) };
        let wait_error = match wait_result {
            WAIT_OBJECT_0 => None,
            WAIT_TIMEOUT => Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "Windows Job still has live processes after termination",
            )),
            WAIT_FAILED => Some(io::Error::last_os_error()),
            other => Some(io::Error::other(format!(
                "unexpected Windows Job wait result {other}"
            ))),
        };
        let close_error = if unsafe { CloseHandle(handle) } == 0 {
            Some(io::Error::last_os_error())
        } else {
            self.handle = None;
            None
        };

        terminate_error
            .or(wait_error)
            .or(close_error)
            .map_or(Ok(()), Err)
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        type Handle = *mut core::ffi::c_void;
        extern "system" {
            fn CloseHandle(handle: Handle) -> i32;
        }
        if let Some(raw) = self.handle.take() {
            unsafe {
                CloseHandle(raw as Handle);
            }
        }
    }
}

fn fail_closed_attachment<R, T>(
    resource: &mut R,
    attach: impl FnOnce(&mut R) -> io::Result<T>,
    cleanup: impl FnOnce(&mut R) -> io::Result<()>,
) -> io::Result<T> {
    match attach(resource) {
        Ok(attached) => Ok(attached),
        Err(attach_error) => match cleanup(resource) {
            Ok(()) => Err(attach_error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "process-tree attachment failed: {attach_error}; direct-child cleanup failed: {cleanup_error}"
            ))),
        },
    }
}

/// Attach immediately after spawn. Any Windows Job setup/assignment failure
/// terminates and reaps the direct child before returning the attachment error.
pub fn attach_process_tree(child: &mut Child) -> io::Result<ProcessTreeGuard> {
    fail_closed_attachment(
        child,
        |child| {
            #[cfg(windows)]
            {
                WindowsJob::create_and_assign(child).map(|job| ProcessTreeGuard { job })
            }
            #[cfg(not(windows))]
            {
                let _ = child;
                Ok(ProcessTreeGuard {})
            }
        },
        terminate_direct_child_and_reap,
    )
}

pub fn terminate_direct_child_and_reap(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_none() {
        if let Err(error) = child.kill() {
            if child.try_wait()?.is_none() {
                return Err(error);
            }
        }
    }
    reap_bounded(child).map(|_| ())
}

/// Primary attached-tree shutdown. Windows terminates and waits on the retained
/// Job first, then uses taskkill/direct-child handling only as defense in depth.
pub fn terminate_process_tree(
    child: &mut Child,
    process_tree: &mut ProcessTreeGuard,
) -> io::Result<()> {
    #[cfg(windows)]
    {
        let job_result = process_tree.job.terminate_wait_and_close();
        let fallback_result = kill_tree(child);
        match (job_result, fallback_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(job_error), Err(fallback_error)) => Err(io::Error::other(format!(
                "Windows Job termination failed: {job_error}; fallback failed: {fallback_error}"
            ))),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = process_tree;
        kill_tree(child)
    }
}

/// Close retained containment and reap a normally exited direct child. On
/// Windows the close is preceded by Job termination/wait so descendants do not
/// survive a successful direct-child exit.
pub fn reap_process_tree(
    child: &mut Child,
    process_tree: &mut ProcessTreeGuard,
) -> io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    process_tree.job.terminate_wait_and_close()?;
    #[cfg(not(windows))]
    let _ = process_tree;
    reap_bounded(child)
}

#[cfg(unix)]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    let pid = child.id() as GroupId;
    let grouped = process_group_exists(pid).unwrap_or(false);
    if grouped {
        let _ = terminate_process_group(pid);
    } else {
        let _ = signal_process(pid, libc::SIGTERM);
    }

    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            if grouped {
                let _ = wait_for_process_group_exit(pid);
            }
            return Ok(());
        }
        if grouped && leader_exited_without_reaping(pid)? {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if grouped {
        let _ = signal_process_group(pid, libc::SIGKILL)
            .or_else(|err| tolerate_sigkill_permission_denied_after_leader_exit(err, pid));
    } else {
        let _ = signal_process(pid, libc::SIGKILL);
    }
    wait_for_unix_child_exit(child)?;
    if grouped {
        wait_for_process_group_exit(pid)
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    // Defense-in-depth fallback for callers without a retained ProcessTreeGuard.
    // Herdr uses terminate_process_tree, where the Job Object is primary.
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    match kill_tree_pid(child.id()) {
        Ok(()) => match wait_for_windows_child_exit(child) {
            Ok(()) => Ok(()),
            Err(_) => terminate_windows_direct_child(child),
        },
        Err(_) => terminate_windows_direct_child(child),
    }
}

#[cfg(not(any(unix, windows)))]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    let _ = child.kill();
    let _ = reap_bounded(child);
    Ok(())
}

pub fn reap_bounded(child: &mut Child) -> io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        wait_for_unix_child_exit(child)?;
        child
            .try_wait()?
            .ok_or_else(|| io::Error::other("child exit status missing after bounded reap"))
    }
    #[cfg(windows)]
    {
        wait_for_windows_child_exit(child)?;
        child
            .try_wait()?
            .ok_or_else(|| io::Error::other("child exit status missing after bounded reap"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        child.wait()
    }
}

#[cfg(unix)]
/// Kills the process group whose leader is `pid`.
///
/// Safety contract: the caller must guarantee the group leader has not been
/// reaped yet at call time. The caller owns the eventual wait and must call
/// this before reaping, so the leader pid remains reserved while group-directed
/// signals are sent.
pub fn kill_tree_pid(pid: u32) -> io::Result<()> {
    let pgid = pid as GroupId;
    terminate_process_group(pgid)?;

    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if !process_group_exists(pgid)? {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    signal_process_group(pgid, libc::SIGKILL)?;
    wait_for_process_group_exit(pgid)
}

#[cfg(windows)]
pub fn kill_tree_pid(pid: u32) -> io::Result<()> {
    // PID-only callers cannot recover a retained Job handle; taskkill remains a
    // bounded fallback rather than Herdr's primary containment guarantee.
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    configure_hidden_process(&mut command);
    let mut child = spawn_std(&mut command)?;
    match wait_for_child_status_bounded(
        &mut child,
        WINDOWS_TASKKILL_TIMEOUT,
        WINDOWS_CHILD_POLL_INTERVAL,
        "taskkill timed out",
    ) {
        Ok(status) => validate_taskkill_status(status.success(), status.code()),
        Err(error) => {
            let _ = child.kill();
            let _ = wait_for_child_status_bounded(
                &mut child,
                WINDOWS_TASKKILL_REAP_TIMEOUT,
                WINDOWS_CHILD_POLL_INTERVAL,
                "timed-out taskkill did not exit after termination",
            );
            Err(error)
        }
    }
}

#[cfg(any(windows, test))]
fn validate_taskkill_status(success: bool, code: Option<i32>) -> io::Result<()> {
    if success {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "taskkill failed with exit {}",
            code.map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )))
    }
}

#[cfg(any(windows, test))]
fn poll_bounded<T>(
    timeout: Duration,
    poll_interval: Duration,
    timeout_message: &'static str,
    mut poll: impl FnMut() -> io::Result<Option<T>>,
) -> io::Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, timeout_message));
        }
        std::thread::sleep(poll_interval);
    }
}

#[cfg(any(windows, test))]
fn wait_bounded(
    timeout: Duration,
    poll_interval: Duration,
    mut exited: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    poll_bounded(
        timeout,
        poll_interval,
        "process still alive after bounded Windows termination wait",
        || Ok(exited()?.then_some(())),
    )
}

#[cfg(windows)]
fn wait_for_child_status_bounded(
    child: &mut Child,
    timeout: Duration,
    poll_interval: Duration,
    timeout_message: &'static str,
) -> io::Result<std::process::ExitStatus> {
    poll_bounded(timeout, poll_interval, timeout_message, || child.try_wait())
}

#[cfg(windows)]
fn wait_for_windows_child_exit(child: &mut Child) -> io::Result<()> {
    wait_bounded(
        WINDOWS_CHILD_EXIT_TIMEOUT,
        WINDOWS_CHILD_POLL_INTERVAL,
        || Ok(child.try_wait()?.is_some()),
    )
}

#[cfg(windows)]
fn terminate_windows_direct_child(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    wait_for_windows_child_exit(child)
}

#[cfg(not(any(unix, windows)))]
pub fn kill_tree_pid(_pid: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn terminate_process_group(pgid: GroupId) -> io::Result<()> {
    signal_process_group(pgid, libc::SIGTERM)
}

#[cfg(unix)]
fn signal_process_group(pgid: GroupId, signal: libc::c_int) -> io::Result<()> {
    let rc = unsafe { libc::killpg(pgid, signal) };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn signal_process(pid: GroupId, signal: libc::c_int) -> io::Result<()> {
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn wait_for_unix_child_exit(child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + UNIX_REAP_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child still alive after bounded Unix termination wait",
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn tolerate_sigkill_permission_denied_after_leader_exit(
    err: io::Error,
    pid: GroupId,
) -> io::Result<()> {
    if err.raw_os_error() == Some(libc::EPERM) && leader_exited_without_reaping(pid)? {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn process_group_exists(pgid: GroupId) -> io::Result<bool> {
    let rc = unsafe { libc::killpg(pgid, 0) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(err),
    }
}

#[cfg(unix)]
fn leader_exited_without_reaping(pid: GroupId) -> io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    let info = unsafe { info.assume_init() };
    // libc 在 Linux 將 siginfo_t 的 si_pid 暴露為 accessor method，macOS/BSD 則是欄位
    #[cfg(target_os = "linux")]
    let exited_pid = unsafe { info.si_pid() };
    #[cfg(not(target_os = "linux"))]
    let exited_pid = info.si_pid;
    Ok(exited_pid == pid)
}

#[cfg(unix)]
fn wait_for_process_group_exit(pgid: GroupId) -> io::Result<()> {
    let deadline = Instant::now() + EXIT_POLL;
    while Instant::now() < deadline {
        if !process_group_exists(pgid)? {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("process group {pgid} still exists after kill"),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    #[test]
    fn windows_background_flags() {
        let background = super::windows_creation_flags();
        assert_eq!(background & 0x0000_0200, 0x0000_0200);
        assert_eq!(background & 0x0800_0000, 0x0800_0000);
    }

    #[test]
    fn windows_job_plan_requires_kill_on_close_and_retained_lifetime() {
        assert_eq!(super::windows_job_limit_flags(), 0x0000_2000);
        assert_eq!(
            super::WINDOWS_JOB_LIFECYCLE_PLAN,
            [
                super::WindowsJobLifecycleStep::Create,
                super::WindowsJobLifecycleStep::ConfigureKillOnClose,
                super::WindowsJobLifecycleStep::AssignChild,
                super::WindowsJobLifecycleStep::RetainForChildLifetime,
                super::WindowsJobLifecycleStep::TerminateAndWait,
                super::WindowsJobLifecycleStep::Close,
                super::WindowsJobLifecycleStep::ReapDirectChild,
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_job_cfg_limit_and_lifecycle_plan() {
        let mut information: super::JobObjectExtendedLimitInformation =
            unsafe { std::mem::zeroed() };
        information.BasicLimitInformation.LimitFlags = super::windows_job_limit_flags();
        assert_eq!(
            information.BasicLimitInformation.LimitFlags
                & super::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            super::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        );
        assert_eq!(
            super::WINDOWS_JOB_LIFECYCLE_PLAN.first(),
            Some(&super::WindowsJobLifecycleStep::Create)
        );
        assert_eq!(
            super::WINDOWS_JOB_LIFECYCLE_PLAN.last(),
            Some(&super::WindowsJobLifecycleStep::ReapDirectChild)
        );
    }

    #[test]
    fn attachment_failure_runs_terminate_and_reap_seam() {
        #[derive(Default)]
        struct FakeChild {
            terminated: bool,
            reaped: bool,
        }

        let mut child = FakeChild::default();
        let error = super::fail_closed_attachment(
            &mut child,
            |_| Err::<(), _>(std::io::Error::other("injected attach failure")),
            |child| {
                child.terminated = true;
                child.reaped = true;
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("injected attach failure"));
        assert!(child.terminated);
        assert!(child.reaped);
    }

    #[test]
    fn attachment_cleanup_failure_is_not_silenced() {
        let mut child = ();
        let error = super::fail_closed_attachment(
            &mut child,
            |_| Err::<(), _>(std::io::Error::other("attach failed")),
            |_| Err(std::io::Error::other("reap failed")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("attach failed"));
        assert!(error.to_string().contains("reap failed"));
    }

    #[test]
    fn windows_hidden_process_flags() {
        let hidden = super::windows_hidden_creation_flags();
        assert_eq!(hidden & 0x0800_0000, 0x0800_0000);
        assert_eq!(hidden & 0x0000_0200, 0);
    }

    #[test]
    fn taskkill_status_requires_a_successful_exit() {
        assert!(super::validate_taskkill_status(true, Some(0)).is_ok());
        let error = super::validate_taskkill_status(false, Some(5)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(error.to_string().contains('5'));
    }

    #[test]
    fn bounded_poll_returns_the_observed_value() {
        let mut attempts = 0;
        let value = super::poll_bounded(
            Duration::from_millis(50),
            Duration::from_millis(1),
            "probe timed out",
            || {
                attempts += 1;
                Ok((attempts == 3).then_some(42))
            },
        )
        .unwrap();
        assert_eq!(value, 42);
        assert_eq!(attempts, 3);
    }

    #[test]
    fn bounded_wait_finishes_without_an_unbounded_child_wait() {
        let mut attempts = 0;
        super::wait_bounded(Duration::from_millis(50), Duration::from_millis(1), || {
            attempts += 1;
            Ok(attempts == 3)
        })
        .unwrap();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn bounded_wait_reports_timeout() {
        let started = Instant::now();
        let error = super::wait_bounded(Duration::from_millis(5), Duration::from_millis(1), || {
            Ok(false)
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .expect("pid file exists")
            .trim()
            .parse()
            .expect("pid is numeric")
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(unix)]
    fn wait_until_gone(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("process {pid} still exists after timeout");
    }

    #[cfg(unix)]
    #[test]
    fn kill_tree_reaps_direct_child_and_kills_grandchild() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("grandchild.pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        super::configure_new_group(&mut cmd);
        let mut child = cmd.spawn().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let grandchild_pid = read_pid(&pid_file);

        super::kill_tree(&mut child).unwrap();

        assert!(
            child.try_wait().unwrap().is_some(),
            "direct child should be reaped by kill_tree"
        );
        wait_until_gone(grandchild_pid);
    }

    #[test]
    fn spawn_lock_admits_one_holder_and_hands_over_on_release() {
        let lock = super::SpawnLock::new();
        let first = lock.acquire(Duration::from_secs(5));
        assert!(first.owned);
        let entered = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let second = lock.acquire(Duration::from_secs(5));
                entered.store(true, std::sync::atomic::Ordering::SeqCst);
                second.owned
            });
            std::thread::sleep(Duration::from_millis(150));
            assert!(
                !entered.load(std::sync::atomic::Ordering::SeqCst),
                "a second holder entered while the first still held the lock"
            );
            drop(first);
            assert!(waiter.join().unwrap());
        });
    }

    #[test]
    fn spawn_lock_wait_is_bounded_and_a_timed_out_guard_frees_nothing() {
        let lock = super::SpawnLock::new();
        let (held, wait_until_held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let lock = &lock;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let guard = lock.acquire(Duration::from_secs(5));
                held.send(()).unwrap();
                // Bounded so an unbounded wait fails the test instead of hanging it.
                let _ = released.recv_timeout(Duration::from_secs(5));
                drop(guard);
            });
            wait_until_held.recv().unwrap();
            let started = Instant::now();
            let timed_out = lock.acquire_logging(Duration::from_millis(100), |_| ());
            let waited = started.elapsed();
            assert!(!timed_out.owned);
            assert!(
                waited >= Duration::from_millis(100) && waited < Duration::from_secs(2),
                "waited {waited:?}"
            );
            drop(timed_out);
            // The timed-out guard must not have freed the lock under its real holder.
            assert!(
                !lock
                    .acquire_logging(Duration::from_millis(50), |_| ())
                    .owned
            );
            release.send(()).unwrap();
            assert!(lock.acquire(Duration::from_secs(5)).owned);
        });
    }

    #[test]
    fn a_blocked_timeout_report_does_not_stall_other_acquires_or_the_release() {
        let lock = super::SpawnLock::new();
        let (held, wait_until_held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (reporting, wait_until_reporting) = std::sync::mpsc::channel();
        let (unblock_report, report_unblocked) = std::sync::mpsc::channel::<()>();
        let (holder_done, wait_for_holder) = std::sync::mpsc::channel();
        let lock = &lock;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let guard = lock.acquire(Duration::from_secs(5));
                held.send(()).unwrap();
                let _ = released.recv_timeout(Duration::from_secs(10));
                drop(guard);
                holder_done.send(()).unwrap();
            });
            wait_until_held.recv().unwrap();
            // The stand-in for a stderr that blocks: this waiter times out and
            // then stays inside its report until the test lets it go.
            scope.spawn(move || {
                let guard = lock.acquire_logging(Duration::from_millis(50), |_| {
                    reporting.send(()).unwrap();
                    let _ = report_unblocked.recv_timeout(Duration::from_secs(10));
                });
                assert!(!guard.owned);
            });
            wait_until_reporting.recv().unwrap();
            release.send(()).unwrap();
            let released_in_time = wait_for_holder.recv_timeout(Duration::from_secs(3));
            // Let the stuck report go whatever happened, so the scope can end.
            unblock_report.send(()).unwrap();
            assert!(
                released_in_time.is_ok(),
                "the holder's release stalled behind a blocked timeout report"
            );
            assert!(lock.acquire(Duration::from_secs(5)).owned);
        });
    }

    #[cfg(unix)]
    #[test]
    fn trace_spawns_reports_only_what_this_thread_spawns_through_the_lock() {
        let (_, none) = super::trace_spawns(|| ());
        assert!(none.is_empty());
        let (_, spawns) = super::trace_spawns(|| {
            let mut std_command = std::process::Command::new("/usr/bin/true");
            std_command.arg("std-arg");
            super::spawn_std(&mut std_command).unwrap().wait().unwrap();
            let mut tokio_command = tokio::process::Command::new("/usr/bin/true");
            tokio_command.arg("tokio-arg");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                super::spawn_tokio(&mut tokio_command)
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
            });
            super::with_spawn_lock(|| ());
            // A raw spawn bypasses the lock, so it must not show up.
            std::process::Command::new("/usr/bin/true")
                .spawn()
                .unwrap()
                .wait()
                .unwrap();
            // Another thread's spawn is not this thread's.
            std::thread::spawn(|| {
                super::spawn_std(&mut std::process::Command::new("/usr/bin/true"))
                    .unwrap()
                    .wait()
                    .unwrap()
            })
            .join()
            .unwrap();
        });
        assert_eq!(
            spawns,
            [
                "/usr/bin/true std-arg",
                "/usr/bin/true tokio-arg",
                "with_spawn_lock"
            ]
        );
        let (_, after) = super::trace_spawns(|| ());
        assert!(after.is_empty());
    }

    #[test]
    fn a_panic_inside_with_spawn_lock_frees_the_lock_and_the_nesting_mark() {
        let outcome =
            std::panic::catch_unwind(|| super::with_spawn_lock(|| panic!("spawn failed")));
        assert!(outcome.is_err());
        assert!(super::SPAWN_LOCK.acquire(Duration::from_secs(5)).owned);
        assert_eq!(super::with_spawn_lock(|| 7), 7);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "not reentrant")]
    fn nesting_with_spawn_lock_fails_a_debug_assertion() {
        super::with_spawn_lock(|| super::with_spawn_lock(|| ()));
    }

    #[cfg(unix)]
    #[test]
    fn kill_tree_escalates_to_sigkill_after_grace() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("grandchild.pid");
        let script = format!(
            "trap '' TERM; sleep 30 & echo $! > {}; wait",
            pid_file.display()
        );
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        super::configure_new_group(&mut cmd);
        let mut child = cmd.spawn().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let grandchild_pid = read_pid(&pid_file);
        let started = Instant::now();

        super::kill_tree(&mut child).unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(3),
            "SIGKILL escalation should not wait for the child sleep"
        );
        wait_until_gone(grandchild_pid);
    }
}
