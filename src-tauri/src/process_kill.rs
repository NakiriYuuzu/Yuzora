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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
fn windows_shell_raw_args(command: &str) -> [std::ffi::OsString; 2] {
    [
        std::ffi::OsString::from("/C"),
        std::ffi::OsString::from(format!("\"{command}\"")),
    ]
}

/// Windows: hand the whole command line to cmd.exe verbatim.
/// `Command::args` applies MSVC-style argv escaping (`"` -> `\"`), which
/// cmd.exe's parser does not understand. `raw_arg` bypasses that escaping (#35).
///
/// This API is for trusted command strings assembled by the caller. When the
/// executable or arguments come from filesystem/user-controlled strings, use
/// [`windows_batch_command`] so those values never enter the shell program text.
#[cfg(windows)]
pub fn windows_shell_command(shell: &std::ffi::OsStr, command: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let [flags, command] = windows_shell_raw_args(command);
    let mut cmd = Command::new(shell);
    cmd.raw_arg(flags);
    cmd.raw_arg(command);
    cmd
}

/// A shell program containing only fixed environment-variable references.
/// User-controlled values are carried in the child environment instead of
/// being concatenated into cmd.exe syntax. cmd expands each variable once;
/// metacharacters introduced by that expansion stay inside the surrounding
/// quotes and are not re-expanded as `%OTHER_VARIABLE%` references.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsBatchCommandPlan {
    command_line: String,
    environment: Vec<(String, String)>,
}

#[cfg(any(windows, test))]
fn windows_batch_environment_value(value: &str) -> String {
    // A doubled quote is the cmd/batch representation of one literal quote
    // inside an already quoted argument. All other characters, including
    // &, %, ^, |, <, > and parentheses, remain environment data.
    value.replace('"', "\"\"")
}

#[cfg(any(windows, test))]
fn windows_batch_command_plan(program: &str, args: &[String]) -> WindowsBatchCommandPlan {
    const PROGRAM_ENV: &str = "YUZORA_MANAGED_BATCH_PROGRAM";
    const ARG_ENV_PREFIX: &str = "YUZORA_MANAGED_BATCH_ARG_";

    let mut environment = Vec::with_capacity(args.len() + 1);
    environment.push((
        PROGRAM_ENV.to_string(),
        windows_batch_environment_value(program),
    ));

    let mut command_line = format!(r#""%{PROGRAM_ENV}%""#);
    for (index, arg) in args.iter().enumerate() {
        let name = format!("{ARG_ENV_PREFIX}{index:04}");
        environment.push((name.clone(), windows_batch_environment_value(arg)));
        command_line.push_str(&format!(r#" "%{name}%""#));
    }

    WindowsBatchCommandPlan {
        command_line,
        environment,
    }
}

/// Windows: execute a `.cmd`/`.bat` program with argv values transported through
/// per-process environment variables. The cmd.exe program text contains no
/// program path or argument data, preventing shell reinterpretation of paths
/// containing spaces, quotes, &, %, ^, |, <, > or parentheses.
#[cfg(windows)]
pub fn windows_batch_command(shell: &std::ffi::OsStr, program: &str, args: &[String]) -> Command {
    use std::os::windows::process::CommandExt;

    let plan = windows_batch_command_plan(program, args);
    let mut command = Command::new(shell);
    command.raw_arg("/D");
    command.raw_arg("/V:OFF");
    command.raw_arg("/S");
    command.raw_arg("/C");
    command.raw_arg(format!("\"{}\"", plan.command_line));
    command.envs(plan.environment);
    command
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

pub(crate) fn terminate_direct_child_and_reap(child: &mut Child) -> io::Result<()> {
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
    let mut child = command.spawn()?;
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
    fn windows_shell_raw_args_preserve_nested_quotes() {
        let command = r#"node "E:\Applications\Yuzora 測試\adapters\probe.mjs""#;
        let raw_args = super::windows_shell_raw_args(command);
        assert_eq!(
            raw_args.len(),
            2,
            "分隔必須靠兩次 raw_arg，不可併成一個 arg（#35）"
        );
        assert_eq!(raw_args[0], std::ffi::OsStr::new("/C"));
        assert_eq!(raw_args[1], std::ffi::OsStr::new(&format!("\"{command}\"")));
        assert!(!raw_args[1].to_string_lossy().contains(r#"\""#));
    }

    #[test]
    fn windows_shell_callers_use_the_shared_launchers() {
        for (name, source, launcher) in [
            (
                "process_service.rs",
                include_str!("process_service.rs"),
                "windows_shell_command(",
            ),
            (
                "lsp_download.rs",
                include_str!("lsp_download.rs"),
                "windows_batch_command(",
            ),
        ] {
            assert!(
                !source.contains(r#"args(["/C""#),
                "{name} 又出現未經 raw_arg 的 cmd.exe launcher（#35）"
            );
            assert!(
                source.contains(launcher),
                "{name} 必須走共用的 {launcher}（#35）"
            );
        }
    }

    #[test]
    fn windows_batch_plan_keeps_user_values_out_of_shell_syntax() {
        let program = r"C:\Program Files\node&tools\npm(alt).cmd";
        let args = vec![
            "install".to_string(),
            "100% literal".to_string(),
            "caret^pipe|redirect<in>out".to_string(),
            "group(one)&group(two)".to_string(),
            r#"Say "hello""#.to_string(),
        ];

        let plan = super::windows_batch_command_plan(program, &args);

        assert_eq!(
            plan.command_line,
            r#""%YUZORA_MANAGED_BATCH_PROGRAM%" "%YUZORA_MANAGED_BATCH_ARG_0000%" "%YUZORA_MANAGED_BATCH_ARG_0001%" "%YUZORA_MANAGED_BATCH_ARG_0002%" "%YUZORA_MANAGED_BATCH_ARG_0003%" "%YUZORA_MANAGED_BATCH_ARG_0004%""#
        );
        for user_value in std::iter::once(program).chain(args.iter().map(String::as_str)) {
            assert!(!plan.command_line.contains(user_value));
        }
        assert_eq!(plan.environment[0].1, program);
        assert_eq!(plan.environment[1].1, "install");
        assert_eq!(plan.environment[2].1, "100% literal");
        assert_eq!(plan.environment[3].1, "caret^pipe|redirect<in>out");
        assert_eq!(plan.environment[4].1, "group(one)&group(two)");
        assert_eq!(plan.environment[5].1, r#"Say ""hello"""#);
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

    /// #35：`windows_shell_command` 必須拆成**兩次** raw arg，靠 std 在 arg 之間
    /// 補的空格分隔 `/C` 與被引號包住的 command。併成一次
    /// （`format!("{flags}{command}")`）會產出 `/C"node …"`，cmd.exe 會把
    /// `/C"node` 當成無法辨識的開關。純函式測試觀察不到這件事——它只檢查
    /// `windows_shell_raw_args` 的回傳值，從不建構 `Command`；而
    /// `windows_shell_command` 是 `#[cfg(windows)]`，在 macOS 根本不編譯。
    /// needle 用 `concat!` 拼接，否則這支測試自己的原始碼會被算進去。
    #[test]
    fn windows_shell_command_splits_flags_and_command_into_two_raw_args() {
        let source = include_str!("process_kill.rs");
        let needle = concat!("cmd.raw", "_arg(");
        assert_eq!(
            source.matches(needle).count(),
            2,
            "windows_shell_command 必須恰好呼叫兩次 raw_arg（#35）"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires Node.js and must run natively on Windows to exercise cmd.exe quoting"]
    fn windows_shell_command_runs_quoted_node_script() {
        const PROBE_OUTPUT: &str = "YUZORA_WINDOWS_CMD_QUOTING_OK";

        let temp = tempfile::Builder::new()
            .prefix("yuzora 測試 cmd ")
            .tempdir()
            .unwrap();
        let probe = temp.path().join("probe.mjs");
        std::fs::write(&probe, format!("console.log(\"{PROBE_OUTPUT}\");\n")).unwrap();
        assert!(probe.to_string_lossy().contains(' '));
        assert!(probe.to_string_lossy().contains("測試"));

        let command = format!(r#"node "{}""#, probe.display());
        let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let output = super::windows_shell_command(shell.as_os_str(), &command)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cmd.exe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), PROBE_OUTPUT);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires Node.js and native Windows cmd.exe to validate batch argv metacharacters"]
    fn windows_batch_command_preserves_metacharacter_arguments() {
        let temp = tempfile::Builder::new()
            .prefix("yuzora batch & quote test ")
            .tempdir()
            .unwrap();
        let probe_js = temp.path().join("probe script.mjs");
        let probe_cmd = temp.path().join("probe & wrapper.cmd");
        let output_path = temp.path().join("argv output.json");
        std::fs::write(
            &probe_js,
            "import fs from 'node:fs'; fs.writeFileSync(process.env.YUZORA_PROBE_OUTPUT, JSON.stringify(process.argv.slice(2)));\n",
        )
        .unwrap();
        std::fs::write(
            &probe_cmd,
            format!("@echo off\r\nnode \"{}\" %*\r\n", probe_js.display()),
        )
        .unwrap();

        let args = vec![
            "space value".to_string(),
            "amp&pipe|caret^".to_string(),
            "percent%PATH%".to_string(),
            "angles<in>out".to_string(),
            "group(one)".to_string(),
            r#"Say "hello""#.to_string(),
        ];
        let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let mut command =
            super::windows_batch_command(shell.as_os_str(), &probe_cmd.to_string_lossy(), &args);
        command.env("YUZORA_PROBE_OUTPUT", &output_path);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "cmd.exe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: Vec<String> =
            serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
        assert_eq!(actual, args);
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
