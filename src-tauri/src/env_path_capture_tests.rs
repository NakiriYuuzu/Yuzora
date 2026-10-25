use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ProbeRun {
    root: tempfile::TempDir,
    child: Option<Child>,
}

impl ProbeRun {
    fn cleanup(&mut self) {
        if let Some(mut child) = self.child.take() {
            // The harness leader is still unreaped, so this owned PGID is pinned.
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGKILL);
            }
            yuzora_host::process_kill::reap_bounded(&mut child).unwrap();
        }
        // A fixed capture creates a separate session. The fixture records its
        // descendant explicitly; never discover or kill processes by name.
        if let Ok(text) = std::fs::read_to_string(self.root.path().join("pids")) {
            let ids: Vec<i32> = text
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            assert_eq!(ids.len(), 3);
            let shell = self.root.path().join("fixture-shell");
            if unsafe { libc::getpgid(ids[0]) } == ids[2] {
                let output = Command::new("/bin/ps")
                    .args(["-p", &ids[0].to_string(), "-o", "args="])
                    .env_clear()
                    .output()
                    .unwrap();
                // On watchdog failure a new capture session can outlive the
                // outer harness. Verify this exact fixture shell before cleanup.
                if String::from_utf8_lossy(&output.stdout).contains(shell.to_str().unwrap()) {
                    unsafe {
                        libc::killpg(ids[2], libc::SIGKILL);
                    }
                }
            }
            if ids[1] > 0 && unsafe { libc::getpgid(ids[1]) } == ids[2] {
                unsafe {
                    libc::kill(ids[1], libc::SIGKILL);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            let alive = || {
                (ids[1] > 0 && unsafe { libc::getpgid(ids[1]) } == ids[2])
                    || unsafe { libc::getpgid(ids[0]) } == ids[2]
            };
            while alive() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!alive(), "owned probe group remained alive");
        }
    }
}

impl Drop for ProbeRun {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn run_probe(body: &str) -> (bool, Option<serde_json::Value>) {
    let root = tempfile::tempdir().unwrap();
    for name in ["home", "tmp", "config", "state", "cache", "run"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    let shell = root.path().join("fixture-shell");
    std::fs::write(&shell, format!("#!/bin/sh\nprintf '%s 0 %s\\n' \"$$\" \"$(/bin/ps -o pgid= -p \"$$\")\" > \"$PROBE_PID_FILE\"\n{body}\n")).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let log = std::fs::File::create(root.path().join("probe.log")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "env_path::capture_tests::owned_probe_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", &shell)
        .env("HOME", root.path().join("home"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env("YUZORA_ENV_CAPTURE_FIXTURE", "owned")
        .env("PROBE_PID_FILE", root.path().join("pids"))
        .env("PROBE_RESULT_FILE", root.path().join("result.json"))
        .env("TEST_EXISTING", "original")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    yuzora_host::process_kill::configure_new_group(&mut command);
    let child = command.spawn().unwrap();
    let mut run = ProbeRun {
        root,
        child: Some(child),
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let completed = loop {
        if let Some(status) = run.child.as_mut().unwrap().try_wait().unwrap() {
            run.child.take(); // Reaped: never send this PID/group another signal.
            break status.success();
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let result = std::fs::read(run.root.path().join("result.json"))
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap());
    run.cleanup();
    if !completed {
        eprintln!(
            "owned probe output: {}",
            std::fs::read_to_string(run.root.path().join("probe.log")).unwrap()
        );
    }
    (completed, result)
}

const OUTPUT: &str = "printf '__YZ_PATH_S__/fixture/bin:/usr/bin__YZ_PATH_E____YZ_ENV_S__TEST_CAPTURE=fixture\\000TEST_EXISTING=replacement\\000__YZ_ENV_E__'";

#[test]
fn gui_probe_does_not_wait_for_descendant_stdout_eof() {
    let body = format!("trap '' HUP TERM; sleep 30 & printf '%s %s %s\\n' \"$$\" \"$!\" \"$(/bin/ps -o pgid= -p \"$$\")\" > \"$PROBE_PID_FILE\"; {OUTPUT}; exit 7");
    let (completed, result) = run_probe(&body);
    assert!(
        completed,
        "GUI probe stalled on a descendant retaining stdout"
    );
    let result = result.unwrap();
    assert_eq!(result["path"], "/fixture/bin:/usr/bin:/bin");
    assert_eq!(result["capture"], "fixture");
    assert_eq!(result["existing"], "original");
}

#[test]
fn gui_probe_drains_finite_output_above_pipe_capacity() {
    let body =
        format!("/usr/bin/awk 'BEGIN {{ for (i=0; i<262144; i++) printf \"x\" }}'; {OUTPUT}");
    let (completed, result) = run_probe(&body);
    assert!(
        completed,
        "finite shell output must drain before waiting for exit"
    );
    assert_eq!(result.unwrap()["capture"], "fixture");
}

#[test]
fn gui_probe_preserves_lossy_bytes_and_rejects_oversized_output_without_partial_import() {
    let body = "printf '__YZ_PATH_S__/fixture/bin:/usr/bin__YZ_PATH_E____YZ_ENV_S__TEST_CAPTURE=a\\377b\\000__YZ_ENV_E__'; exit 7";
    let (completed, result) = run_probe(body);
    assert!(completed);
    assert_eq!(result.unwrap()["capture"], "a\u{fffd}b");
    let body =
        format!("{OUTPUT}; /usr/bin/awk 'BEGIN {{ for (i=0; i<2097152; i++) printf \"x\" }}'");
    let (completed, result) = run_probe(&body);
    assert!(completed);
    let result = result.unwrap();
    assert_eq!(result["path"], "/usr/bin:/bin");
    assert!(
        result["capture"].is_null(),
        "never import a truncated environment"
    );
}

#[test]
#[ignore = "parent-only fixture process; never invoke with user shell/environment"]
fn owned_probe_child() {
    assert_eq!(
        std::env::var("YUZORA_ENV_CAPTURE_FIXTURE").as_deref(),
        Ok("owned")
    );
    super::fix_gui_path();
    let result = serde_json::json!({
        "path": std::env::var("PATH").ok(),
        "capture": std::env::var("TEST_CAPTURE").ok(),
        "existing": std::env::var("TEST_EXISTING").ok(),
    });
    std::fs::write(
        std::env::var_os("PROBE_RESULT_FILE").unwrap(),
        result.to_string(),
    )
    .unwrap();
}
