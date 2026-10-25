use super::*;

struct Fixture {
    root: tempfile::TempDir,
    child: Child,
    tree: process_kill::ProcessTreeGuard,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .env_clear()
            .env("HOME", root.path())
            .env("TMPDIR", root.path())
            .env("XDG_CONFIG_HOME", root.path())
            .env("XDG_STATE_HOME", root.path())
            .env("XDG_CACHE_HOME", root.path())
            .env("XDG_RUNTIME_DIR", root.path())
            .env("OWNED_ROOT", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_background_process(&mut command);
        let mut child = command.spawn().unwrap();
        let tree = process_kill::attach_process_tree(&mut child).unwrap();
        Self { root, child, tree }
    }

    fn capture(
        &mut self,
        timeout: Duration,
        limit: usize,
    ) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), String> {
        wait_bounded_child(&mut self.child, &mut self.tree, timeout, limit)
    }

    fn await_leader_exit(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !process_kill::leader_exited_without_reaping(self.child.id() as i32).unwrap() {
            assert!(Instant::now() < deadline, "owned fixture leader must exit");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = process_kill::terminate_process_tree(&mut self.child, &mut self.tree);
        }
    }
}

fn held_pipe_respects_deadline(redirect: &str) {
    // The finite descendant makes the old implementation fail without hanging
    // the test process. No signal is sent to a reaped leader's numeric PGID.
    let script = format!(
        "trap '' TERM HUP; /bin/sleep 2 {redirect} & printf '%s' $! > \"$OWNED_ROOT/pid\"; printf '{{\"ok\":true}}'; exit 0"
    );
    let mut fixture = Fixture::new(&script);
    fixture.await_leader_exit();
    let descendant: i32 = fs::read_to_string(fixture.root.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let started = Instant::now();
    let result = fixture.capture(Duration::from_millis(80), 1024);
    let elapsed = started.elapsed();
    let child_reaped = fixture.child.try_wait().unwrap().is_some();
    let remaining = unsafe { libc::getpgid(descendant) == fixture.child.id() as i32 };
    println!("HERDR_PIPE_DEADLINE redirect={redirect:?} elapsedMs={} result={result:?} descendantAlive={remaining}", elapsed.as_millis());
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("timeout")),
        "{result:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "configured80ms deadline bypassed: {elapsed:?}"
    );
    assert!(child_reaped);
    assert!(!remaining, "owned descendant should be gone on return");
}

#[test]
fn exited_leader_stdout_holder_still_obeys_deadline() {
    held_pipe_respects_deadline("2>/dev/null");
}

#[test]
fn exited_leader_stderr_holder_still_obeys_deadline() {
    held_pipe_respects_deadline(">/dev/null");
}

#[test]
fn exited_leader_both_pipe_holder_still_obeys_deadline() {
    held_pipe_respects_deadline("");
}

#[test]
fn descendant_tail_and_nonzero_leader_status_are_preserved() {
    let mut fixture =
        Fixture::new("( /bin/sleep 0.1; printf tail; printf errtail >&2 ) & printf head; exit 7");
    fixture.await_leader_exit();
    let (stdout, stderr, status) = fixture.capture(Duration::from_secs(2), 1024).unwrap();
    assert_eq!(stdout, b"headtail");
    assert_eq!(stderr, b"errtail");
    assert_eq!(status.code(), Some(7));
    assert!(fixture.child.try_wait().unwrap().is_some());
}

#[test]
fn both_streams_exceeding_pipe_capacity_are_drained_completely() {
    let mut fixture = Fixture::new(
        "/usr/bin/head -c 524288 /dev/zero & /usr/bin/head -c 524288 /dev/zero >&2 & wait",
    );
    let (stdout, stderr, status) = fixture.capture(Duration::from_secs(3), 524288).unwrap();
    assert!(status.success());
    assert_eq!(stdout, vec![0; 524288]);
    assert_eq!(stderr, vec![0; 524288]);
}

#[test]
fn each_stream_enforces_its_own_exact_limit() {
    for (count, expected_error) in [(32768, false), (32769, true)] {
        for redirect in ["", ">&2"] {
            let mut fixture =
                Fixture::new(&format!("/usr/bin/head -c {count} /dev/zero {redirect}"));
            let result = fixture.capture(Duration::from_secs(2), 32768);
            if expected_error {
                assert!(result.unwrap_err().contains("tooLarge"));
            } else {
                let (stdout, stderr, status) = result.unwrap();
                assert!(status.success());
                assert_eq!(stdout.len() + stderr.len(), count);
            }
            assert!(fixture.child.try_wait().unwrap().is_some());
        }
    }
}

#[test]
fn continuous_stdout_and_sparse_stderr_cannot_starve_deadline() {
    let mut fixture = Fixture::new("( /bin/sleep 0.02; printf marker >&2 ) & /bin/cat /dev/zero");
    let started = Instant::now();
    let result = fixture.capture(Duration::from_millis(80), usize::MAX);
    assert!(result.unwrap_err().contains("timeout"));
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(fixture.child.try_wait().unwrap().is_some());
}

#[test]
fn pipe_eof_without_leader_exit_still_obeys_deadline() {
    let mut fixture = Fixture::new("exec 1>&- 2>&-; exec /bin/sleep 2");
    let started = Instant::now();
    let result = fixture.capture(Duration::from_millis(80), 1024);
    assert!(result.unwrap_err().contains("timeout"));
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(fixture.child.try_wait().unwrap().is_some());
}

#[test]
fn missing_pipe_errors_clean_up_owned_children() {
    for stdout_missing in [true, false] {
        let mut fixture = Fixture::new("exec /bin/sleep 2");
        if stdout_missing {
            drop(fixture.child.stdout.take());
        } else {
            drop(fixture.child.stderr.take());
        }
        let result = fixture.capture(Duration::from_millis(80), 1024);
        let name = if stdout_missing { "stdout" } else { "stderr" };
        assert_eq!(result.unwrap_err(), format!("herdr {name} pipe missing"));
        assert!(fixture.child.try_wait().unwrap().is_some());
    }
}
