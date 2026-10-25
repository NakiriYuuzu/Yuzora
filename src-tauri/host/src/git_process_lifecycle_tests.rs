use super::*;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::AtomicU32;

struct OwnedDescendant {
    root: tempfile::TempDir,
    leader: Arc<AtomicU32>,
}

impl OwnedDescendant {
    fn descendant(&self) -> Option<i32> {
        std::fs::read_to_string(self.root.path().join("pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
    fn alive(&self) -> bool {
        self.descendant().is_some_and(|pid| unsafe {
            libc::getpgid(pid) == self.leader.load(Ordering::Acquire) as i32
        })
    }
    fn cleanup(&self) {
        if self.alive() {
            unsafe {
                libc::kill(self.descendant().unwrap(), libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!self.alive(), "owned red-case descendant must be cleaned");
    }
}
impl Drop for OwnedDescendant {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn exited_leader_cleanup(cancel: bool) {
    let fixture = OwnedDescendant {
        root: tempfile::tempdir().unwrap(),
        leader: Arc::default(),
    };
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "trap '' TERM HUP; /bin/sleep 30 & printf '%s\\n' \"$!\" > \"$OWNED_PID\"; exit 0",
        ])
        .env_clear()
        .env("HOME", fixture.root.path())
        .env("TMPDIR", fixture.root.path())
        .env("OWNED_PID", fixture.root.path().join("pid"))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let leader = fixture.leader.clone();
    let hook = move |pid| {
        leader.store(pid, Ordering::Release);
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancellation = cancel.then(|| {
        let leader = fixture.leader.clone();
        let token = cancelled.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let pid = leader.load(Ordering::Acquire);
                if pid != 0 {
                    match process_kill::leader_exited_without_reaping(pid as i32) {
                        Ok(true) => break,
                        Err(error) if error.raw_os_error() == Some(libc::ECHILD) => break,
                        Ok(false) => {}
                        Err(error) => panic!("{error}"),
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "leader must exit before cancellation"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            token.store(true, Ordering::Release);
        })
    });
    let result = with_cancellation(cancelled, || {
        execute(
            command,
            &["owned-exited-parent"],
            if cancel {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(100)
            },
            None,
            Some(&hook),
        )
    });
    if let Some(task) = cancellation {
        task.join().unwrap();
    }
    let recorded = fixture.descendant().is_some();
    let remained = fixture.alive();
    fixture.cleanup();
    assert!(recorded, "fixture child must actually have started");
    let error = result.err().unwrap();
    if cancel {
        assert_eq!(error, "git-cancelled");
    } else {
        assert!(error.contains("timed out"));
    }
    assert!(
        !remained,
        "Git timeout returned while its descendant remained alive"
    );
}

#[test]
fn timeout_after_git_leader_exit_cleans_the_stdout_holding_descendant() {
    exited_leader_cleanup(false);
}

#[test]
fn cancel_after_git_leader_exit_cleans_the_stdout_holding_descendant() {
    exited_leader_cleanup(true);
}

#[test]
fn shutdown_rejects_late_and_new_git_commands() {
    const ROOT: &str = "YUZORA_GIT_SHUTDOWN_TEST_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = std::path::PathBuf::from(root);
        assert_eq!(
            std::fs::read_to_string(root.join("owner")).unwrap(),
            unsafe { libc::getppid() }.to_string()
        );
        shutdown_registration_edges();
        return;
    }

    // Closing admission is process-wide. Run the actual executor in a private
    // test process so concurrent Git tests retain their own live registry.
    let root = tempfile::tempdir().unwrap();
    for name in ["home", "config", "data", "state", "cache", "run", "tmp"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    std::fs::write(root.path().join("owner"), std::process::id().to_string()).unwrap();
    let config = root.path().join("herdr.toml");
    std::fs::write(
        &config,
        "onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n",
    )
    .unwrap();
    let log_path = root.path().join("test.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "git_process::lifecycle_tests::shutdown_rejects_late_and_new_git_commands",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path().join("home"))
        .env("CFFIXED_USER_HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("HERDR_CONFIG_PATH", config)
        .env(ROOT, root.path())
        .current_dir(root.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .status()
        .unwrap();
    let output = std::fs::read_to_string(log_path).unwrap();
    print!("{output}");
    assert!(
        status.success(),
        "isolated shutdown fixture failed:\n{output}"
    );
    assert!(output.contains("GIT_SHUTDOWN_EDGES passed"), "{output}");
}

fn shutdown_registration_edges() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    fn command(program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn assert_reaped(pid: i32) {
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    assert_eq!(
        execute(
            command("/usr/bin/true"),
            &["normal"],
            Duration::from_secs(2),
            None,
            None
        )
        .unwrap()
        .code,
        0
    );
    let (registered_tx, registered_rx) = std::sync::mpsc::sync_channel(1);
    let registered = std::thread::spawn(move || {
        let mut child = command("/bin/sleep");
        child.arg("2");
        execute(
            child,
            &["registered"],
            Duration::from_secs(3),
            None,
            Some(&|pid| {
                registered_tx.send(pid).unwrap();
            }),
        )
        .map(|output| output.code)
    });
    let registered_pid = registered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let (mut ready_reader, ready_writer) = UnixStream::pair().unwrap();
    let (mut release_writer, release_reader) = UnixStream::pair().unwrap();
    ready_reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    release_reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut late = command("/usr/bin/true");
    let ready_fd = ready_writer.as_raw_fd();
    let release_fd = release_reader.as_raw_fd();
    // Pause inside spawn, before exec and registration. Only async-signal-safe
    // system calls run in the post-fork child. The socket timeout bounds failure.
    unsafe {
        late.pre_exec(move || {
            let pid = libc::getpid().to_ne_bytes();
            if libc::write(ready_fd, pid.as_ptr().cast(), pid.len()) != pid.len() as isize {
                return Err(std::io::Error::last_os_error());
            }
            let mut release = 0u8;
            if libc::read(release_fd, (&mut release as *mut u8).cast(), 1) != 1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let late = std::thread::spawn(move || {
        execute(late, &["late"], Duration::from_secs(2), None, None).map(|output| output.code)
    });
    let mut pid = [0; 4];
    ready_reader.read_exact(&mut pid).unwrap();
    let late_pid = i32::from_ne_bytes(pid);
    crate::git_service::kill_all_processes();
    release_writer.write_all(&[1]).unwrap();
    let registered_result = registered.join().unwrap();
    let late_result = late.join().unwrap();
    let new_result = execute(
        command("/usr/bin/true"),
        &["new"],
        Duration::from_secs(2),
        None,
        None,
    )
    .map(|output| output.code);
    eprintln!("GIT_SHUTDOWN_RESULTS registered={registered_result:?} late={late_result:?} new={new_result:?}");
    crate::git_service::kill_all_processes();
    assert_reaped(registered_pid as i32);
    assert_reaped(late_pid);
    assert_ne!(registered_result, Ok(0), "registered child must be stopped");
    assert_eq!(
        late_result,
        Err("git-cancelled".to_string()),
        "late registration after shutdown must be rejected and reaped"
    );
    assert_eq!(
        new_result,
        Err("git-cancelled".to_string()),
        "shutdown must reject new commands"
    );
    eprintln!("GIT_SHUTDOWN_EDGES passed");
}
