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
                    return Err("git-output-limit".into());
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
    mut cmd: Command,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
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
            let _ = tx.send((which, read_pipe(pipe, limit, &stop, deadline)));
        })
    };
    // Boxed Read gives both pipe types one bounded worker implementation.
    let out_thread = output_worker(Box::new(stdout) as Box<dyn Read + Send>, STDOUT_LIMIT, 0);
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
        while let Ok((which, result)) = rx.try_recv() {
            outputs[which] = Some(result?);
        }
        if let Some(status) = owned.child.try_wait().map_err(|e| e.to_string())? {
            if outputs.iter().all(Option::is_some) {
                return Ok(status.code().unwrap_or(-1));
            }
        }
        std::thread::sleep(POLL);
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
