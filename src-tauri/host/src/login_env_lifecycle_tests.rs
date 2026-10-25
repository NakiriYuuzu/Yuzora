use super::*;

struct OwnedDescendant {
    record: std::path::PathBuf,
}

impl OwnedDescendant {
    fn ids(&self) -> Option<(i32, i32)> {
        let text = std::fs::read_to_string(&self.record).ok()?;
        let mut values = text.split_whitespace().map(str::parse::<i32>);
        Some((values.next()?.ok()?, values.next()?.ok()?))
    }

    fn alive(&self) -> bool {
        self.ids().is_some_and(|(leader, child)| unsafe {
            libc::getpgid(child) == leader && libc::kill(child, 0) == 0
        })
    }

    fn cleanup(&self) {
        // The record names only our fixture child; require its original group.
        // Do not signal the reaped leader PID or scan other processes.
        if let Some((leader, child)) = self.ids() {
            if unsafe { libc::getpgid(child) } == leader {
                unsafe {
                    libc::kill(child, libc::SIGKILL);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for OwnedDescendant {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[test]
fn exited_login_shell_does_not_leave_a_stdout_holding_descendant() {
    let root = tempfile::tempdir().unwrap();
    let owned = OwnedDescendant {
        record: root.path().join("owned-pids"),
    };
    let mut command = Command::new("/bin/sh");
    command.env_clear().env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path()).env("TMPDIR", root.path())
        .env("PROBE_PID_FILE", &owned.record)
        .args(["-c", "trap '' HUP TERM; sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PROBE_PID_FILE\"; printf '\\000YUZORA_ENV_START\\000FIXTURE=ok\\000\\000YUZORA_ENV_END\\000'; exit 0"]);
    let result = capture(command, Duration::from_secs(1), 4096);
    let leaked = owned.alive();
    owned.cleanup();
    assert!(
        !owned.alive(),
        "test-owned descendant must be cleaned even in red case"
    );
    assert_eq!(
        parse(&result.unwrap()).unwrap(),
        vec![("FIXTURE".into(), "ok".into())]
    );
    assert!(
        !leaked,
        "exited leader left its stdout-holding descendant alive"
    );
}

fn owned_command(root: &Path, owned: &OwnedDescendant, tail: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.env_clear().env("PATH", "/usr/bin:/bin")
        .env("HOME", root).env("TMPDIR", root)
        .env("PROBE_PID_FILE", &owned.record).args(["-c", &format!(
            "trap '' HUP TERM; sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PROBE_PID_FILE\"; {tail}"
        )]);
    command
}

#[test]
fn timeout_overflow_and_nonzero_exit_clean_descendants_and_keep_host_policy() {
    let root = tempfile::tempdir().unwrap();
    let owned = OwnedDescendant {
        record: root.path().join("owned-pids"),
    };
    for (tail, error) in [
        ("sleep 30", "environment-shell-timeout"),
        (
            "while :; do printf '1234567890'; done",
            "environment-output-limit",
        ),
        (
            "printf '\\000YUZORA_ENV_START\\000OK=value\\000\\000YUZORA_ENV_END\\000'; exit 7",
            "environment-shell-failed",
        ),
    ] {
        let command = owned_command(root.path(), &owned, tail);
        assert_eq!(
            capture(command, Duration::from_millis(100), 1024).unwrap_err(),
            error
        );
        assert!(!owned.alive(), "cleanup after {error}");
    }
}

#[test]
fn raw_capture_preserves_bytes_and_status_and_drains_large_finite_output() {
    let mut command = Command::new("/bin/sh");
    command.env_clear().args([
        "-c",
        "printf '\\000YUZORA_ENV_START\\000VALUE=a\\377=b\\000\\000YUZORA_ENV_END\\000'; exit 7",
    ]);
    let captured =
        capture_output(command, Duration::from_secs(1), 4096).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(captured.status.code(), Some(7));
    assert_eq!(parse(&captured.stdout).unwrap()[0].1.as_bytes(), b"a\xff=b");
    let mut command = Command::new("/usr/bin/awk");
    command
        .env_clear()
        .args(["BEGIN { for (i=0; i<262144; i++) printf \"x\" }"]);
    let captured =
        capture_output(command, Duration::from_secs(2), 524288).unwrap_or_else(|e| panic!("{e}"));
    assert!(captured.status.success());
    assert_eq!(&*captured.stdout, &vec![b'x'; 262144]);
}

fn descriptor_count() -> usize {
    #[cfg(target_os = "linux")]
    let path = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let path = "/dev/fd";
    std::fs::read_dir(path).unwrap().count()
}

fn thread_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        Some(std::fs::read_dir("/proc/self/task").unwrap().count())
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/bin/ps")
            .args(["-M", "-p", &std::process::id().to_string(), "-o", "pid="])
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success());
        Some(String::from_utf8_lossy(&output.stdout).lines().count())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[test]
fn repeated_capture_releases_owned_groups_descriptors_and_threads() {
    let root = tempfile::tempdir().unwrap();
    for name in ["home", "tmp", "config", "state", "cache", "run"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    let log_path = root.path().join("lifecycle.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "login_env::lifecycle_tests::owned_capture_lifecycle_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path().join("home"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env("YUZORA_CAPTURE_LIFECYCLE_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    crate::process_kill::configure_new_group(&mut command);
    let mut guard = LifecycleChild {
        child: Some(command.spawn().unwrap()),
        descendant: OwnedDescendant {
            record: root.path().join("owned-pids"),
        },
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let completed = loop {
        if let Some(status) = guard.child.as_mut().unwrap().try_wait().unwrap() {
            guard.child.take();
            break status.success();
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    drop(guard);
    let output = std::fs::read_to_string(log_path).unwrap();
    print!("{output}");
    assert!(completed, "isolated lifecycle child failed or timed out");
    assert_eq!(
        output.matches("LOGIN_CAPTURE_LIFECYCLE cycle=").count(),
        100
    );
}

struct LifecycleChild {
    child: Option<Child>,
    descendant: OwnedDescendant,
}

impl Drop for LifecycleChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Capture groups are separate from the outer test harness. While
            // the harness lives, confirm the recorded leader is still its child.
            if let Some((leader, _)) = self.descendant.ids() {
                let output = Command::new("/bin/ps")
                    .args(["-p", &leader.to_string(), "-o", "ppid="])
                    .env_clear()
                    .output()
                    .unwrap();
                if String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
                    == Some(child.id())
                    && unsafe { libc::getpgid(leader) } == leader
                {
                    unsafe {
                        libc::killpg(leader, libc::SIGKILL);
                    }
                }
            }
            // The outer leader has not been reaped, so its PGID is still pinned.
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGKILL);
            }
            crate::process_kill::reap_bounded(&mut child).unwrap();
        }
        self.descendant.cleanup();
    }
}

#[test]
#[ignore = "parent-only isolated resource-count fixture"]
fn owned_capture_lifecycle_child() {
    let root = std::path::PathBuf::from(
        std::env::var_os("YUZORA_CAPTURE_LIFECYCLE_ROOT")
            .expect("run only through the owning lifecycle parent"),
    );
    let owned = OwnedDescendant {
        record: root.join("owned-pids"),
    };
    let mut baseline = None;
    for cycle in 0..120 {
        let command = owned_command(&root, &owned,
            "printf '\\000YUZORA_ENV_START\\000VALUE=complete\\000\\000YUZORA_ENV_END\\000'; exit 0");
        let bytes = capture(command, Duration::from_secs(1), 4096).unwrap();
        assert_eq!(
            parse(&bytes).unwrap(),
            vec![("VALUE".into(), "complete".into())]
        );
        assert!(!owned.alive());
        if cycle == 19 {
            baseline = Some((descriptor_count(), thread_count()));
        }
        if cycle >= 20 {
            let resources = (descriptor_count(), thread_count());
            assert_eq!(Some(resources), baseline, "cycle {cycle}");
            println!(
                "LOGIN_CAPTURE_LIFECYCLE cycle={} fds={} threads={:?} ownedChildren=0",
                cycle - 19,
                resources.0,
                resources.1
            );
        }
    }
}
