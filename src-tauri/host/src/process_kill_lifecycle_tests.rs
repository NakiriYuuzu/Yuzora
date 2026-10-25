use super::*;
use std::path::PathBuf;
use std::process::Stdio;

std::thread_local! {
    static SIGNALS: std::cell::RefCell<Option<Vec<(i32, i32)>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn record_signal(pid: i32, signal: i32) -> bool {
    SIGNALS.with(|slot| match slot.borrow_mut().as_mut() {
        Some(signals) => {
            signals.push((pid, signal));
            true
        }
        None => false,
    })
}

struct Fixture {
    _root: tempfile::TempDir,
    child: Child,
    descendant: i32,
    cleaned: bool,
}

impl Fixture {
    fn new(exit_before_call: bool) -> Self {
        let ending = if exit_before_call { "exit 0" } else { "wait" };
        // The child inherits ignored dispositions before the leader replaces
        // its own TERM handler, so there is no trap-installation race.
        let script = format!("trap '' TERM HUP; /bin/sleep 30 & descendant=$!; trap 'exit 0' TERM; printf '%s\\n' \"$descendant\" > \"$OWNED_PID\"; {ending}");
        Self::launch(&script, None)
    }

    fn launch(script: &str, child_script: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("descendant.pid");
        if let Some(script) = child_script {
            std::fs::write(root.path().join("child.sh"), script).unwrap();
        }
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .env_clear()
            .env("HOME", root.path())
            .env("TMPDIR", root.path())
            .env("OWNED_PID", &record)
            .env("OWNED_ROOT", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_new_group(&mut command);
        let mut child = command.spawn().unwrap();
        let Some(descendant) = read_pid(&record) else {
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGKILL);
            }
            reap_bounded(&mut child).unwrap();
            wait_for_process_group_exit(child.id() as i32).unwrap();
            panic!("owned descendant did not become ready");
        };
        Self {
            _root: root,
            child,
            descendant,
            cleaned: false,
        }
    }

    fn descendant_alive(&self) -> bool {
        unsafe { libc::getpgid(self.descendant) == self.child.id() as i32 }
    }

    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        // Child remembers a successful reap. Only a still-running, unreaped
        // leader pins the group here; never reacquire ownership by raw PID.
        if matches!(self.child.try_wait(), Ok(None)) {
            unsafe {
                libc::killpg(self.child.id() as i32, libc::SIGKILL);
            }
            reap_bounded(&mut self.child).unwrap();
        }
        // The old implementation already reaps the leader. Only the exact
        // recorded descendant, still in that owned group, may be signalled now.
        if self.descendant_alive() {
            unsafe {
                libc::kill(self.descendant, libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.descendant_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !self.descendant_alive(),
            "fixture cleanup must succeed even on red"
        );
        self.cleaned = true;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn read_pid(path: &PathBuf) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return Some(pid);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn leader_exit_during_term_still_escalates_for_the_remaining_group() {
    let mut fixture = Fixture::new(false);
    let result = kill_tree(&mut fixture.child);
    let remained = fixture.descendant_alive();
    fixture.cleanup();
    result.unwrap();
    assert!(
        !remained,
        "kill_tree returned success with a TERM-ignoring descendant alive"
    );
}

#[test]
fn unreaped_naturally_exited_leader_still_owns_descendant_cleanup() {
    let mut fixture = Fixture::new(true);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !leader_exited_without_reaping(fixture.child.id() as i32).unwrap() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = kill_tree(&mut fixture.child);
    let remained = fixture.descendant_alive();
    fixture.cleanup();
    result.unwrap();
    assert!(
        !remained,
        "an unreaped zombie leader must not bypass group escalation"
    );
}

#[test]
fn descendants_retain_their_term_grace_after_the_leader_exits() {
    let child_script = r#"trap '/bin/sleep 0.1; printf done > "$OWNED_ROOT/done"; exit 0' TERM
printf ready > "$OWNED_ROOT/ready"
/bin/sleep 30 & wait
"#;
    let parent = r#"trap 'exit 0' TERM
/bin/sh "$OWNED_ROOT/child.sh" & descendant=$!
while [ ! -f "$OWNED_ROOT/ready" ]; do /bin/sleep 0.01; done
printf '%s\n' "$descendant" > "$OWNED_PID"
wait
"#;
    let mut fixture = Fixture::launch(parent, Some(child_script));
    kill_tree(&mut fixture.child).unwrap();
    assert_eq!(
        std::fs::read(fixture._root.path().join("done")).unwrap(),
        b"done"
    );
    assert!(!fixture.descendant_alive());
}

#[test]
fn already_reaped_child_is_rejected_before_any_signal() {
    let observed = std::thread::spawn(|| {
        let mut command = Command::new("/usr/bin/true");
        command.env_clear();
        configure_new_group(&mut command);
        let mut child = command.spawn().unwrap();
        child.wait().unwrap();
        // Suppress real signals even if the regression returns: never create
        // a real PID-reuse hazard just to test stale-Child rejection.
        SIGNALS.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
        let result = kill_tree(&mut child);
        let signals = SIGNALS.with(|slot| slot.borrow_mut().take().unwrap());
        (result.err().and_then(|error| error.raw_os_error()), signals)
    })
    .join()
    .unwrap();
    assert_eq!(observed.0, Some(libc::ECHILD));
    assert!(observed.1.is_empty());
}

#[test]
fn direct_child_and_grouped_child_keep_distinct_termination_policies() {
    for grouped in [false, true] {
        let mut command = Command::new("/bin/sleep");
        command.arg("30").env_clear();
        if grouped {
            configure_new_group(&mut command);
        }
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        kill_tree(&mut child).unwrap();
        let elapsed = started.elapsed();
        assert!(child.try_wait().unwrap().is_some());
        if grouped {
            assert!(elapsed >= Duration::from_millis(300));
        }
        println!("GROUP_TERMINATION_POLICY grouped={grouped} observedMs={} (functional grace check; not a performance comparison)", elapsed.as_millis());
    }
}

#[test]
#[ignore = "owned 100-cycle resource probe; exact isolated invocation required"]
fn owned_group_lifecycle_probe() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.iter().any(|a| a == "--exact"));
    assert!(args.iter().any(|a| a == "--test-threads=1"));
    assert!(args
        .iter()
        .any(|a| a == "process_kill::lifecycle_tests::owned_group_lifecycle_probe"));
    let count = || {
        #[cfg(target_os = "linux")]
        let path = "/proc/self/fd";
        #[cfg(not(target_os = "linux"))]
        let path = "/dev/fd";
        std::fs::read_dir(path).unwrap().count()
    };
    let mut baseline = None;
    for cycle in 0..120 {
        let mut fixture = Fixture::new(cycle % 2 == 0);
        kill_tree(&mut fixture.child).unwrap();
        assert!(!fixture.descendant_alive());
        drop(fixture);
        if cycle == 19 {
            baseline = Some(count());
        }
        if cycle >= 20 {
            let fds = count();
            assert_eq!(Some(fds), baseline);
            println!(
                "GROUP_LIFECYCLE cycle={} ownedChildren=0 fds={fds}",
                cycle - 19
            );
        }
    }
}
