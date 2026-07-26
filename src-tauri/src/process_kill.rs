use std::io;
use std::process::{Child, Command};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_millis(300);
#[cfg(unix)]
const EXIT_POLL: Duration = Duration::from_secs(3);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[cfg(any(windows, test))]
const WINDOWS_CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(any(windows, test))]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
#[cfg(windows)]
pub fn windows_shell_command(shell: &std::ffi::OsStr, command: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let [flags, command] = windows_shell_raw_args(command);
    let mut cmd = Command::new(shell);
    cmd.raw_arg(flags);
    cmd.raw_arg(command);
    cmd
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
    // [SPIKE 驗證] The plan calls for assigning the child to a Windows Job
    // Object with KILL_ON_JOB_CLOSE. std::process::Command has no portable hook
    // to retain that Job handle here; the exact windows crate features and
    // handle ownership model need verification on a Windows machine.
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

#[cfg(unix)]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    let pgid = child.id() as GroupId;
    let pid = pgid;
    terminate_process_group(pgid)?;

    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if leader_exited_without_reaping(pid)? {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    signal_process_group(pgid, libc::SIGKILL)
        .or_else(|err| tolerate_sigkill_permission_denied_after_leader_exit(err, pid))?;
    let _ = child.wait();
    wait_for_process_group_exit(pgid)
}

#[cfg(windows)]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    // [SPIKE 驗證] A retained Job Object with KILL_ON_JOB_CLOSE is the robust
    // path still needing Windows verification. `taskkill /T` is a best-effort
    // snapshot tree kill and may miss already re-parented processes.
    let _ = kill_tree_pid(child.id());
    let _ = child.wait();
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn kill_tree(child: &mut Child) -> io::Result<()> {
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
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
    // [SPIKE 驗證] Terminating a process tree by pid on Windows should resolve
    // the Job Object for that pid, or use a verified platform fallback.
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    configure_hidden_process(&mut command);
    command.status().map(|_| ())
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
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    fn windows_background_flags() {
        let background = super::windows_creation_flags();
        assert_eq!(background & 0x0000_0200, 0x0000_0200);
        assert_eq!(background & 0x0800_0000, 0x0800_0000);
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
    fn windows_shell_callers_use_the_shared_launcher() {
        for (name, source) in [
            ("agent_process.rs", include_str!("agent_process.rs")),
            ("process_service.rs", include_str!("process_service.rs")),
        ] {
            assert!(
                !source.contains(r#"args(["/C""#),
                "{name} 又出現未經 raw_arg 的 cmd.exe launcher（#35）"
            );
            assert!(
                source.contains("windows_shell_command("),
                "{name} 必須走共用的 windows_shell_command（#35）"
            );
        }
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
