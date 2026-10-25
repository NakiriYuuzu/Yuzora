//! The process-wide spawn lock, exercised with real processes and descriptors.
#![cfg(unix)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use yuzora_host::process_kill::{configure_new_group, spawn_std, spawn_tokio, with_spawn_lock};

/// These tests make descriptors inheritable on purpose, so one test's raw spawn
/// must not run while another test's descriptors are in that state.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long the stand-in for std's pipe setup leaves its descriptors inheritable.
const GAP: Duration = Duration::from_millis(300);
const EOF_WAIT: Duration = Duration::from_secs(2);
const HELD_FOR: Duration = Duration::from_millis(200);

/// Plays the part of std's `Stdio::piped()` on macOS: a pipe created with both
/// ends inheritable that only turns close-on-exec in a later step. It runs
/// under the spawn lock, like the std spawn it stands in for. `created` fires
/// once the pipe exists, so the caller can try to spawn inside the gap.
fn pipe_with_a_gap(created: Sender<()>) -> (OwnedFd, OwnedFd) {
    with_spawn_lock(|| {
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        created.send(()).unwrap();
        std::thread::sleep(GAP);
        for fd in fds {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    })
}

/// The pipe is readable only at EOF (nothing is ever written to it), so this
/// says whether every copy of its write end is gone.
fn reached_eof(read_end: &OwnedFd, within: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd: read_end.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(within.as_millis()).unwrap();
    unsafe { libc::poll(&mut poll, 1, millis) == 1 }
}

fn sleeper() -> Command {
    let mut command = Command::new("sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn kill(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

/// Try to spawn right after the pipe is created, then close our write end and
/// report whether the read end still never reached EOF, meaning the spawned
/// child captured the write end.
fn spawn_in_the_gap(spawn: impl FnOnce() -> Child, eof_wait: Duration) -> bool {
    let (created, wait_for_pipe) = mpsc::channel();
    let gap = std::thread::spawn(move || pipe_with_a_gap(created));
    wait_for_pipe.recv().unwrap();
    let child = spawn();
    let (read_end, write_end) = gap.join().unwrap();
    drop(write_end);
    let captured = !reached_eof(&read_end, eof_wait);
    kill(child);
    captured
}

#[test]
fn a_raw_spawn_inside_the_gap_captures_the_pipe() {
    let _serial = serial();
    // The premise of the lock; without it the tests below prove nothing.
    assert!(spawn_in_the_gap(
        || sleeper().spawn().unwrap(),
        Duration::from_millis(500)
    ));
}

#[test]
fn spawn_std_does_not_capture_a_pipe_that_is_still_being_set_up() {
    let _serial = serial();
    assert!(!spawn_in_the_gap(
        || spawn_std(&mut sleeper()).unwrap(),
        EOF_WAIT
    ));
}

#[test]
fn spawn_std_on_the_fork_path_does_not_capture_a_pipe_that_is_still_being_set_up() {
    let _serial = serial();
    assert!(!spawn_in_the_gap(
        || {
            let mut command = sleeper();
            // A pre_exec hook makes std fork instead of posix_spawn.
            configure_new_group(&mut command);
            spawn_std(&mut command).unwrap()
        },
        EOF_WAIT
    ));
}

#[test]
fn spawn_tokio_does_not_capture_a_pipe_that_is_still_being_set_up() {
    let _serial = serial();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (created, wait_for_pipe) = mpsc::channel();
    let gap = std::thread::spawn(move || pipe_with_a_gap(created));
    wait_for_pipe.recv().unwrap();
    let mut command = tokio::process::Command::new("sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = runtime.block_on(async { spawn_tokio(&mut command).unwrap() });
    let (read_end, write_end) = gap.join().unwrap();
    drop(write_end);
    let reached = reached_eof(&read_end, EOF_WAIT);
    runtime.block_on(child.kill()).unwrap();
    assert!(reached);
}

/// Hold the spawn lock on another thread, start `spawn` on a third, and check it
/// only completes after the lock is released.
fn spawn_waits_for_the_lock<T: Send + 'static>(spawn: impl FnOnce() -> T + Send + 'static) -> T {
    let (held, wait_until_held) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        with_spawn_lock(|| {
            held.send(()).unwrap();
            // Bounded so a broken lock fails the test instead of hanging it.
            let _ = released.recv_timeout(Duration::from_secs(5));
        });
    });
    wait_until_held.recv().unwrap();
    let (done, finished) = mpsc::channel();
    let spawner = std::thread::spawn(move || done.send(spawn()).unwrap());
    assert!(
        finished.recv_timeout(HELD_FOR).is_err(),
        "the spawn completed while the spawn lock was held"
    );
    release.send(()).unwrap();
    let spawned = finished.recv_timeout(Duration::from_secs(5)).unwrap();
    holder.join().unwrap();
    spawner.join().unwrap();
    spawned
}

#[test]
fn spawn_std_waits_while_the_lock_is_held() {
    let _serial = serial();
    kill(spawn_waits_for_the_lock(|| {
        spawn_std(&mut sleeper()).unwrap()
    }));
}

#[test]
fn spawn_tokio_waits_while_the_lock_is_held() {
    let _serial = serial();
    let (runtime, mut child) = spawn_waits_for_the_lock(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut command = tokio::process::Command::new("sleep");
        command
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // The runtime goes with the child so its driver outlives the spawn.
        let child = runtime.block_on(async { spawn_tokio(&mut command).unwrap() });
        (runtime, child)
    });
    runtime.block_on(child.kill()).unwrap();
}

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// `#[cfg(test)]` and `#[cfg(all(test, ..))]` keep their item for test builds
/// only; `any(.., test)` and `not(test)` still ship, so they stay in the scan.
fn is_test_only_cfg(line: &str) -> bool {
    let line = line.trim();
    line == "#[cfg(test)]" || line.starts_with("#[cfg(all(test,")
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Last line of the item that starts at `lines[start]`. An item whose first
/// line opens a block (`{`, `(`, `[`) ends at the first later line that closes
/// it at the same indent without opening again (`) -> T {` does not end a
/// multi-line signature; rustfmt keeps the indent invariant). Anything else,
/// like `mod tests;` or `use ..;`, is the one line.
fn end_of_item(lines: &[&str], start: usize) -> usize {
    let opens = |line: &str| line.trim_end().ends_with(['{', '(', '[']);
    if !opens(lines[start]) {
        return start;
    }
    (start + 1..lines.len())
        .find(|&i| {
            indent(lines[i]) == indent(lines[start])
                && lines[i].trim_start().starts_with(['}', ')', ']'])
                && !opens(lines[i])
        })
        .unwrap_or_else(|| panic!("unterminated test-only item at line {}", start + 1))
}

/// The source lines (with their 1-based numbers) outside test-only items,
/// wherever those are in the file. An out-of-line `#[cfg(test)] mod tests;`
/// removes only its own declaration, so the production code after it is still
/// scanned.
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut kept = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !is_test_only_cfg(lines[index]) {
            kept.push((index + 1, lines[index]));
            index += 1;
            continue;
        }
        let mut item = index + 1;
        while item < lines.len() && {
            let line = lines[item].trim_start();
            line.starts_with("#[") || line.starts_with("//")
        } {
            item += 1;
        }
        assert!(item < lines.len(), "test-only attribute without an item");
        index = end_of_item(&lines, item) + 1;
    }
    kept
}

#[test]
fn the_scan_drops_test_only_items_and_keeps_all_other_code() {
    let source = "\
fn production() { cmd.spawn() }
#[cfg(test)]
mod tests;
fn after_the_declaration() { cmd.output() }
#[cfg(test)]
mod inline_tests {
    fn in_the_module() { cmd.spawn() }
}
fn after_the_inline_module() { cmd.status() }
#[cfg(all(test, unix))]
#[path = \"more.rs\"]
mod more;
impl Thing {
    #[cfg(test)]
    fn only_for_tests(&self) {
        self.cmd.spawn()
    }
    #[cfg(any(unix, test))]
    fn also_production(&self) { self.cmd.spawn() }
    #[cfg(not(test))]
    fn not_a_test(&self) { self.cmd.spawn() }
}
#[cfg(test)]
fn long_signature(
    a: u8,
) -> u8 {
    cmd.spawn()
}
fn last() { cmd.status() }
";
    let kept = production_lines(source)
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n");
    for shipped in [
        "production",
        "after_the_declaration",
        "after_the_inline_module",
        "also_production",
        "not_a_test",
        "last",
    ] {
        assert!(
            kept.contains(&format!("fn {shipped}(")),
            "{shipped} was cut"
        );
    }
    for test_only in [
        "in_the_module",
        "only_for_tests",
        "long_signature",
        "more",
        "mod tests",
    ] {
        assert!(!kept.contains(test_only), "{test_only} was kept");
    }
    assert_eq!(kept.matches("cmd.spawn()").count(), 3);
}

#[test]
#[should_panic(expected = "unterminated test-only item")]
fn the_scan_fails_closed_on_a_test_only_item_it_cannot_delimit() {
    production_lines("#[cfg(test)]\nmod broken {\n    fn f() {}\n");
}

/// `.status()` is also a plain getter on HTTP responses and our own managers,
/// so it only counts when the receiver is not one of those known getters. A
/// builder chain split over lines (`.status()` alone on its line) counts.
fn calls_command_status(code: &str) -> bool {
    const GETTER_RECEIVERS: [&str; 3] = ["self", "response", "manager"];
    code.match_indices(".status()").any(|(at, _)| {
        let before = &code[..at];
        let receiver = &before[before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1)..];
        !GETTER_RECEIVERS.contains(&receiver)
    })
}

/// Whether a production source line spawns a process on its own. `pty_launch`
/// lets the PTY calls through: `PtyLaunch::start` makes them and its only
/// caller wraps it in the lock.
fn spawns_outside_the_lock(code: &str, pty_launch: bool) -> bool {
    const SPAWN_CALLS: [&str; 2] = [".spawn()", ".output()"];
    const PTY_CALLS: [&str; 2] = [".spawn_command(", ".openpty("];
    !code.starts_with("//")
        && (SPAWN_CALLS.iter().any(|call| code.contains(call))
            || calls_command_status(code)
            || (!pty_launch && PTY_CALLS.iter().any(|call| code.contains(call))))
}

#[test]
fn the_scan_recognizes_every_way_to_start_a_command() {
    for spawning in [
        "cmd.spawn()",
        ".spawn()",
        "let child = cmd.spawn().unwrap();",
        "Command::new(\"x\").output().unwrap()",
        "cmd.status()",
        ".status()",
        "Command::new(\"x\").status()?;",
        "slave.spawn_command(builder)",
        "native_pty_system().openpty(size)",
    ] {
        assert!(spawns_outside_the_lock(spawning, false), "{spawning}");
    }
    for harmless in [
        "// cmd.spawn()",
        "process_kill::spawn_std(&mut cmd)",
        "self.status()",
        "response.status().as_u16()",
        "state.manager.status()",
        "let status = state.manager.status();",
    ] {
        assert!(!spawns_outside_the_lock(harmless, false), "{harmless}");
    }
    assert!(spawns_outside_the_lock("cmd.spawn()", true));
    assert!(!spawns_outside_the_lock("slave.spawn_command(b)", true));
    assert!(!spawns_outside_the_lock("pty.openpty(size)", true));
}

/// A source scan, so it is a regression guard and not proof of behaviour: it
/// stops a new spawn site from quietly bypassing the lock. Each site's wiring
/// is proven by its own `process_kill::trace_spawns` test.
#[test]
fn production_code_spawns_only_through_the_lock() {
    let host = Path::new(env!("CARGO_MANIFEST_DIR"));
    let helpers = host.join("src/process_kill.rs");
    // `PtyLaunch::start` makes the PTY and spawns on it; its only caller wraps it.
    let pty_launch = host.join("src/herdr_native_client.rs");
    // Production lines only: the file's own tests wrap `start` the same way.
    let pty_source = std::fs::read_to_string(&pty_launch).unwrap();
    assert!(
        production_lines(&pty_source)
            .iter()
            .any(|(_, line)| line.contains("process_kill::with_spawn_lock(|| launch.start(")),
        "PtyLaunch::start must run under the spawn lock"
    );
    let mut offenders = Vec::new();
    for crate_src in ["src", "../src", "../companion/src"] {
        let dir = host.join(crate_src);
        assert!(dir.is_dir(), "{} is missing", dir.display());
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            if file == helpers || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&file).unwrap();
            for (number, line) in production_lines(&source) {
                let code = line.trim_start();
                if spawns_outside_the_lock(code, file == pty_launch) {
                    offenders.push(format!("{}:{number}: {code}", file.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "spawn through process_kill::{{spawn_std, spawn_tokio, with_spawn_lock}}:\n{}",
        offenders.join("\n")
    );
}
