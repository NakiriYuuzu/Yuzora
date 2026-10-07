//! Bounded Git execution. Cancellation is scoped to the calling workspace job.
use crate::{git_service::GitOutput, process_kill};
use std::io::{Read, Write};
use std::process::{Child, Command};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const STDOUT_LIMIT: usize = 8 * 1024 * 1024;
const STDERR_LIMIT: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(5);

pub use crate::cancellation::with_cancellation;

struct OwnedChild {
    child: Child,
    tree: process_kill::ProcessTreeGuard,
    stop: Arc<AtomicBool>,
    finished: bool,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if !self.finished {
            let _ = process_kill::terminate_process_tree(&mut self.child, &mut self.tree);
        }
        crate::git_service::unregister_process(self.child.id());
    }
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<(), String> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn read_pipe(
    mut pipe: impl Read,
    limit: usize,
    limit_error: &'static str,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err("git-cancelled-or-timeout".into());
        }
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(n) => {
                if bytes.len() + n > limit {
                    return Err(limit_error.into());
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub(crate) fn execute(
    cmd: Command,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
) -> Result<GitOutput, String> {
    execute_with_stdout_limit(cmd, args, timeout, stdin, on_spawn, STDOUT_LIMIT)
}

/// `execute` with a caller-chosen stdout bound, for reads whose own contract
/// (e.g. blob grading up to the file hard cap) exceeds the default limit.
pub(crate) fn execute_with_stdout_limit(
    mut cmd: Command,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
    stdout_limit: usize,
) -> Result<GitOutput, String> {
    let cancelled = crate::cancellation::token();
    if cancelled.load(Ordering::Acquire) {
        return Err("git-cancelled".into());
    }
    if stdin.is_some_and(|data| data.len() > STDOUT_LIMIT) {
        return Err("git-input-limit".into());
    }
    let deadline = Instant::now() + timeout;
    let mut child = cmd.spawn().map_err(|e| format!("git spawn failed: {e}"))?;
    let tree = process_kill::attach_process_tree(&mut child).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut owned = OwnedChild {
        child,
        tree,
        stop: stop.clone(),
        finished: false,
    };
    crate::git_service::register_process(owned.child.id());
    if let Some(hook) = on_spawn {
        hook(owned.child.id());
    }
    let stdout = owned.child.stdout.take().ok_or("git-stdout-missing")?;
    let stderr = owned.child.stderr.take().ok_or("git-stderr-missing")?;
    let input = owned.child.stdin.take();
    #[cfg(unix)]
    {
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        if let Some(input) = &input {
            nonblocking(input)?;
        }
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(3);
    let output_worker = |pipe, limit, which| {
        let stop = stop.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            // Distinct stderr error: callers treat a stdout overflow as an oversized
            // object, which must never absorb an unrelated stderr flood.
            let limit_error = if which == 0 {
                "git-output-limit"
            } else {
                "git-stderr-limit"
            };
            let _ = tx.send((which, read_pipe(pipe, limit, limit_error, &stop, deadline)));
        })
    };
    // Boxed Read gives both pipe types one bounded worker implementation.
    let out_thread = output_worker(Box::new(stdout) as Box<dyn Read + Send>, stdout_limit, 0);
    let err_thread = output_worker(Box::new(stderr) as Box<dyn Read + Send>, STDERR_LIMIT, 1);
    let data = stdin.unwrap_or_default().to_vec();
    let tx_input = tx.clone();
    let stop_input = stop.clone();
    let in_thread = std::thread::spawn(move || {
        let result = (|| {
            if let Some(mut input) = input {
                let mut remaining = data.as_slice();
                while !remaining.is_empty() {
                    if stop_input.load(Ordering::Acquire) || Instant::now() >= deadline {
                        return Err("git-cancelled-or-timeout".into());
                    }
                    match input.write(remaining) {
                        Ok(0) => return Err("git-stdin-closed".into()),
                        Ok(n) => remaining = &remaining[n..],
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(POLL)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
            Ok(Vec::new())
        })();
        let _ = tx_input.send((2, result));
    });
    drop(tx);
    let mut outputs = [None, None, None];
    let mut pending_output = None;
    let result = (|| loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("git-cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "git {} timed out after {timeout:?}",
                args.first().unwrap_or(&"")
            ));
        }
        if let Some((which, result)) = pending_output.take() {
            outputs[which] = Some(result?);
        }
        while let Ok((which, result)) = rx.try_recv() {
            outputs[which] = Some(result?);
        }
        if let Some(status) = owned.child.try_wait().map_err(|e| e.to_string())? {
            if outputs.iter().all(Option::is_some) {
                return Ok(status.code().unwrap_or(-1));
            }
        }
        // Wake on completion, but process it after the cancellation/deadline
        // checks above so those keep their existing precedence.
        match rx.recv_timeout(POLL) {
            Ok(message) => pending_output = Some(message),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(POLL),
        }
    })();
    if result.is_err() {
        stop.store(true, Ordering::Release);
        let _ = process_kill::terminate_process_tree(&mut owned.child, &mut owned.tree);
    } else {
        process_kill::reap_process_tree(&mut owned.child, &mut owned.tree)
            .map_err(|e| e.to_string())?;
    }
    owned.finished = true;
    // Unix readers are nonblocking, so even a descendant that detached its own
    // process group cannot keep the pipe workers alive after cancellation.
    #[cfg(unix)]
    {
        let _ = out_thread.join();
        let _ = err_thread.join();
        let _ = in_thread.join();
    }
    #[cfg(not(unix))]
    {
        drop((out_thread, err_thread, in_thread));
    }
    let output = GitOutput {
        code: result?,
        stdout: outputs[0].take().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&outputs[1].take().unwrap_or_default()).into_owned(),
    };
    crate::git_service::log_git_call(args, output.code, &output.stderr);
    Ok(output)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::git_service::run_git;

    #[cfg(unix)]
    fn fixture_command(
        root: &std::path::Path,
        program: &str,
        args: &[&str],
        pipe_input: bool,
    ) -> Command {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .current_dir(root)
            .process_group(0);
        command.stdin(if pipe_input {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command
    }

    #[test]
    fn owned_empty_input_delivers_eof() {
        let root = tempfile::tempdir().unwrap();
        for input in [None, Some(&[][..])] {
            let result = execute(
                fixture_command(root.path(), "/bin/cat", &[], true),
                &["owned-eof"],
                Duration::from_secs(2),
                input,
                None,
            )
            .unwrap();
            assert_eq!(result.code, 0);
            assert!(result.stdout.is_empty());
            assert!(result.stderr.is_empty());
        }
    }

    #[test]
    fn owned_input_and_stdout_limit_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let data = "中文😀\n".repeat(4096).into_bytes();
        let result = execute(
            fixture_command(root.path(), "/bin/cat", &[], true),
            &["owned-payload"],
            Duration::from_secs(2),
            Some(&data),
            None,
        )
        .unwrap();
        assert_eq!(result.code, 0);
        assert_eq!(result.stdout, data);
        assert!(result.stderr.is_empty());
        let error = execute_with_stdout_limit(
            fixture_command(root.path(), "/bin/cat", &[], true),
            &["owned-limit"],
            Duration::from_secs(2),
            Some(&data),
            None,
            4,
        )
        .err()
        .unwrap();
        assert_eq!(error, "git-output-limit");
    }

    #[test]
    fn owned_cancel_retains_precedence_and_does_not_cancel_the_next_job() {
        let root = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let hook = move |_pid| signal.store(true, Ordering::Release);
        let started = Instant::now();
        let error = with_cancellation(cancelled, || {
            execute(
                fixture_command(root.path(), "/bin/sh", &["-c", "exec /bin/sleep 30"], false),
                &["owned-cancel"],
                Duration::from_secs(5),
                None,
                Some(&hook),
            )
        })
        .err()
        .unwrap();
        assert_eq!(error, "git-cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
        let next = execute(
            fixture_command(root.path(), "/usr/bin/printf", &["ok"], false),
            &["owned-next"],
            Duration::from_secs(2),
            None,
            None,
        )
        .unwrap();
        assert_eq!(next.code, 0);
        assert_eq!(next.stdout, b"ok");
    }

    #[test]
    fn owned_timeout_remains_bounded() {
        let root = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let error = execute(
            fixture_command(root.path(), "/bin/sh", &["-c", "exec /bin/sleep 30"], true),
            &["owned-timeout"],
            Duration::from_millis(50),
            Some(&[]),
            None,
        )
        .err()
        .unwrap();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn closed_output_channels_still_wait_for_the_real_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let result = execute(
            fixture_command(
                root.path(),
                "/bin/sh",
                &["-c", "exec 1>&- 2>&-; /bin/sleep 0.05; exit 23"],
                false,
            ),
            &["owned-closed-pipes"],
            Duration::from_secs(2),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.code, 23);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }

    #[cfg(unix)]
    #[allow(
        clippy::assertions_on_constants,
        reason = "This manual comparison requires release binaries"
    )]
    #[test]
    #[ignore = "manual owned process runner performance measurement"]
    fn performance_empty_stdin_worker() {
        use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System};
        assert!(!cfg!(debug_assertions), "Use --release");
        fn cpu_ms(kind: libc::c_int) -> f64 {
            let usage = unsafe {
                let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
                assert_eq!(libc::getrusage(kind, value.as_mut_ptr()), 0);
                value.assume_init()
            };
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
        }
        let root = tempfile::tempdir().unwrap();
        let payload = "fixture 中文😀\n".repeat(4096).into_bytes();
        let mut memory = System::new();
        let pid = get_current_pid().unwrap();
        for scenario in ["no-input", "empty-input", "unicode-input", "stderr-exit"] {
            for pass in 0..8 {
                let iterations = if pass == 0 { 100 } else { 40 };
                let mut durations = Vec::with_capacity(iterations);
                let before = cpu_ms(libc::RUSAGE_SELF);
                let children_before = cpu_ms(libc::RUSAGE_CHILDREN);
                for _ in 0..iterations {
                    let (command, input) = match scenario {
                        "no-input" => (
                            fixture_command(root.path(), "/usr/bin/printf", &["fixture"], false),
                            None,
                        ),
                        "empty-input" => (
                            fixture_command(root.path(), "/bin/cat", &[], true),
                            Some(&[][..]),
                        ),
                        "unicode-input" => (
                            fixture_command(root.path(), "/bin/cat", &[], true),
                            Some(payload.as_slice()),
                        ),
                        _ => (
                            fixture_command(root.path(), "/usr/bin/printf", &["%"], false),
                            None,
                        ),
                    };
                    let started = Instant::now();
                    let result = execute(
                        command,
                        &["owned-fixture"],
                        Duration::from_secs(5),
                        input,
                        None,
                    )
                    .unwrap();
                    durations.push(started.elapsed().as_secs_f64() * 1000.0);
                    if scenario == "stderr-exit" {
                        assert_ne!(result.code, 0);
                        assert!(!result.stderr.is_empty());
                    } else {
                        assert_eq!(result.code, 0);
                        assert!(result.stderr.is_empty());
                        let expected = match scenario {
                            "no-input" => b"fixture".as_slice(),
                            "unicode-input" => payload.as_slice(),
                            _ => &[],
                        };
                        assert_eq!(result.stdout, expected);
                    }
                }
                let cpu = cpu_ms(libc::RUSAGE_SELF) - before;
                let children = cpu_ms(libc::RUSAGE_CHILDREN) - children_before;
                memory.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::nothing().with_memory(),
                );
                let fd_path = if cfg!(target_os = "linux") {
                    "/proc/self/fd"
                } else {
                    "/dev/fd"
                };
                println!(
                    "EMPTY_STDIN_MEASUREMENT {}",
                    serde_json::json!({
                        "scenario": scenario, "pass": pass, "warmup": pass == 0, "iterations": iterations,
                        "parentCpuMs": cpu, "childCpuMs": children, "totalCpuMs": cpu + children,
                        "durationsMs": durations, "descriptors": std::fs::read_dir(fd_path).unwrap().count(),
                        "rssBytes": memory.process(pid).unwrap().memory(), "profile": "release"
                    })
                );
            }
        }
    }

    #[test]
    fn output_is_bounded_and_cancellation_does_not_affect_other_jobs() {
        let root = tempfile::tempdir().unwrap();
        let error = run_git(
            root.path(),
            &["-c", "alias.flood=!yes", "flood"],
            Duration::from_secs(5),
            &[],
        )
        .err()
        .unwrap();
        assert_eq!(error, "git-output-limit");
        // A stderr flood is reported separately so blob readers never mistake it
        // for an oversized object.
        let error = run_git(
            root.path(),
            &["-c", "alias.flood=!yes >&2", "flood"],
            Duration::from_secs(5),
            &[],
        )
        .err()
        .unwrap();
        assert_eq!(error, "git-stderr-limit");
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.store(true, Ordering::Release);
        });
        let error = with_cancellation(cancelled, || {
            run_git(
                root.path(),
                &["-c", "alias.hang=!sleep 30", "hang"],
                Duration::from_secs(30),
                &[],
            )
        })
        .err()
        .unwrap();
        worker.join().unwrap();
        assert_eq!(error, "git-cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            run_git(root.path(), &["--version"], Duration::from_secs(5), &[])
                .unwrap()
                .code,
            0
        );
    }
}
